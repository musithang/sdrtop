// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

mod discovery;
mod protocol;

use std::collections::HashSet;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError};
use serde::{Deserialize, Serialize};
use serialport::{DataBits, FlowControl, Parity, SerialPort, StopBits};

use crate::config::TinySaSettings;
use crate::hardware::{
    AcquisitionKind, DeliveryModel, DeviceCapabilities, DeviceInfo, DeviceListing, DeviceOption,
    DirectSweepConfig, GainModel, LevelUnit, PowerTrace, PowerTraceTarget, RxContext, SampleFormat,
    SampleGeometry, SdrDevice, SoftwareStack,
};

use super::traits::RateSet;
use protocol::{Identity, Model, PROMPT};

const MIN_FREQUENCY_HZ: u64 = 100_000;
const DEFAULT_FREQUENCY_HZ: u64 = 100_000_000;
const DEFAULT_SPAN_HZ: u64 = 10_000_000;
#[cfg(test)]
const DEFAULT_POINTS: u32 = 450;
const READ_TIMEOUT: Duration = Duration::from_millis(50);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const DRAIN_QUIET: Duration = Duration::from_millis(200);
const MAX_RESPONSE_BYTES: usize = 128 * 1024;
const BASIC_LOW_MAX_HZ: u64 = 350_000_000;
const BASIC_HIGH_MIN_HZ: u64 = 240_000_000;
const BASIC_HIGH_MAX_HZ: u64 = 959_000_000;

type UnitReply = Sender<anyhow::Result<()>>;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BasicInput {
    #[default]
    Low,
    High,
}

impl BasicInput {
    fn parse(value: &str) -> anyhow::Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "low" => Ok(Self::Low),
            "high" => Ok(Self::High),
            _ => bail!("tinySA input must be 'low' or 'high'"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Low => "LOW",
            Self::High => "HIGH",
        }
    }

    fn mode_command(self) -> &'static str {
        match self {
            Self::Low => "mode low input",
            Self::High => "mode high input",
        }
    }

    fn range(self) -> (u64, u64) {
        match self {
            Self::Low => (MIN_FREQUENCY_HZ, BASIC_LOW_MAX_HZ),
            Self::High => (BASIC_HIGH_MIN_HZ, BASIC_HIGH_MAX_HZ),
        }
    }
}

pub fn resolve_basic_input(explicit: Option<BasicInput>, configured: BasicInput) -> BasicInput {
    explicit.unwrap_or(configured)
}

pub fn list(selector: Option<&str>) -> Vec<DeviceListing> {
    let (path, input) = match selector {
        Some(selector) => match parse_selector(selector) {
            Ok(selection) => selection,
            Err(error) => {
                eprintln!("tinySA: invalid device selector: {error}");
                return Vec::new();
            }
        },
        None => (None, None),
    };
    if let Some(path) = path {
        let input_label = input
            .map(|input| format!(" · {} input", input.label()))
            .unwrap_or_default();
        return vec![DeviceListing {
            kind: crate::hardware::DeviceKind::TinySa,
            index: 0,
            label: format!("tinySA · {}{input_label}", path.display()),
            serial: None,
            args: None,
            path: Some(path),
            tiny_sa_input: input,
        }];
    }
    let mut devices = discovery::list();
    for device in &mut devices {
        device.tiny_sa_input = input;
        if let Some(input) = input {
            device
                .label
                .push_str(&format!(" · {} input", input.label()));
        }
    }
    devices
}

pub fn parse_selector(selector: &str) -> anyhow::Result<(Option<PathBuf>, Option<BasicInput>)> {
    let (path, input) = match selector.rsplit_once("?input=") {
        Some((path, input)) => (path, Some(BasicInput::parse(input)?)),
        None => (selector, None),
    };
    if path.contains('?') {
        bail!("tinySA selector only supports '?input=low' or '?input=high'");
    }
    Ok(((!path.is_empty()).then(|| PathBuf::from(path)), input))
}

pub struct TinySaDevice {
    caps: DeviceCapabilities,
    info: DeviceInfo,
    notes: Vec<String>,
    model: Model,
    basic_input: BasicInput,
    options: Arc<Mutex<Vec<DeviceOption>>>,
    modified_options: Arc<Mutex<HashSet<String>>>,
    command_tx: Sender<Command>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl TinySaDevice {
    pub fn open(
        path: &Path,
        basic_input: BasicInput,
        explicit_basic_input: Option<BasicInput>,
        settings: &TinySaSettings,
    ) -> anyhow::Result<Self> {
        validate_settings_shape(settings)?;
        let port = serialport::new(path.to_string_lossy(), 115_200)
            .data_bits(DataBits::Eight)
            .stop_bits(StopBits::One)
            .parity(Parity::None)
            .flow_control(FlowControl::None)
            .timeout(READ_TIMEOUT)
            .open()
            .with_context(|| format!("failed to open tinySA at {}", path.display()))?;
        let (command_tx, command_rx) = crossbeam_channel::unbounded();
        let (init_tx, init_rx) = bounded(1);
        let options = Arc::new(Mutex::new(Vec::new()));
        let modified_options = Arc::new(Mutex::new(HashSet::new()));
        let worker_options = Arc::clone(&options);
        let worker_modified_options = Arc::clone(&modified_options);
        let settings = settings.clone();
        let worker = thread::Builder::new()
            .name("tinysa-serial".to_string())
            .spawn(move || {
                worker_entry(
                    port,
                    command_rx,
                    init_tx,
                    basic_input,
                    settings,
                    worker_options,
                    worker_modified_options,
                )
            })
            .context("failed to start tinySA serial worker")?;
        let initialized = match init_rx.recv() {
            Ok(Ok(initialized)) => initialized,
            Ok(Err(error)) => {
                let _ = worker.join();
                return Err(error);
            }
            Err(_) => {
                let _ = worker.join();
                bail!("tinySA serial worker stopped during initialization");
            }
        };
        let caps = capabilities(initialized.identity.model, basic_input);
        let mut notes = initialized
            .identity
            .hardware
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        if let Some(note) =
            ignored_explicit_basic_input_note(initialized.identity.model, explicit_basic_input)
        {
            notes.push(note);
        }
        let info = DeviceInfo {
            board_name: initialized.identity.board.clone(),
            serial: path.display().to_string(),
            stack: Some(SoftwareStack {
                label: "tinysa fw ",
                value: Arc::from(initialized.identity.firmware.as_str()),
            }),
            ..DeviceInfo::default()
        };
        Ok(Self {
            caps,
            info,
            notes,
            model: initialized.identity.model,
            basic_input,
            options,
            modified_options,
            command_tx,
            worker: Mutex::new(Some(worker)),
        })
    }

    fn request<T>(
        &self,
        make_command: impl FnOnce(Sender<anyhow::Result<T>>) -> Command,
    ) -> anyhow::Result<T> {
        let (reply_tx, reply_rx) = bounded(1);
        self.command_tx
            .send(make_command(reply_tx))
            .map_err(|_| anyhow!("tinySA serial worker is not running"))?;
        reply_rx
            .recv()
            .map_err(|_| anyhow!("tinySA serial worker stopped without replying"))?
    }
}

impl SdrDevice for TinySaDevice {
    fn capabilities(&self) -> &DeviceCapabilities {
        &self.caps
    }

    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn start_rx(&self, ctx: Arc<RxContext>) -> anyhow::Result<()> {
        self.request(|reply| Command::Start(ctx, reply))
    }

    fn stop_rx(&self) -> anyhow::Result<()> {
        self.request(Command::Stop)
    }

    fn is_streaming(&self) -> bool {
        self.request(Command::IsStreaming).unwrap_or(false)
    }

    fn set_frequency(&self, hz: u64) -> anyhow::Result<()> {
        self.request(|reply| Command::SetFrequency(hz, reply))
    }

    fn set_sample_rate(&self, hz: f64) -> anyhow::Result<RateSet> {
        self.request(|reply| Command::SetSpan(hz, reply))
    }

    fn set_lna_gain(&self, _db: u32) -> anyhow::Result<()> {
        self.request(Command::NoOp)
    }

    fn set_direct_sweep(&self, config: Option<DirectSweepConfig>) -> anyhow::Result<()> {
        self.request(|reply| Command::SetDirectSweep(config, reply))
    }

    fn options(&self) -> Vec<DeviceOption> {
        self.options
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn set_option(&self, id: &str, choice: &str) -> anyhow::Result<()> {
        {
            let options = self
                .options
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            validate_option_choice(&options, id, choice)?;
        }
        self.request(|reply| Command::SetOption {
            id: id.to_string(),
            choice: choice.to_string(),
            reply,
        })
    }

    fn update_config(&self, config: &mut crate::config::AppConfig) -> anyhow::Result<()> {
        config.tinysa = persisted_settings(
            &config.tinysa,
            &self.options(),
            self.model,
            self.basic_input,
            &self
                .modified_options
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        )?;
        Ok(())
    }

    fn open_notes(&self) -> &[String] {
        &self.notes
    }
}

impl Drop for TinySaDevice {
    fn drop(&mut self) {
        let (reply_tx, reply_rx) = bounded(1);
        let _ = self.command_tx.send(Command::Shutdown(reply_tx));
        let _ = reply_rx.recv_timeout(Duration::from_secs(2));
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            let _ = worker.join();
        }
    }
}

enum Command {
    Start(Arc<RxContext>, UnitReply),
    Stop(UnitReply),
    IsStreaming(Sender<anyhow::Result<bool>>),
    SetFrequency(u64, UnitReply),
    SetSpan(f64, Sender<anyhow::Result<RateSet>>),
    NoOp(UnitReply),
    SetDirectSweep(Option<DirectSweepConfig>, UnitReply),
    SetOption {
        id: String,
        choice: String,
        reply: UnitReply,
    },
    Shutdown(UnitReply),
}

struct Initialized {
    identity: Identity,
}

struct Worker {
    port: Box<dyn SerialPort>,
    command_rx: Receiver<Command>,
    identity: Identity,
    options: Vec<DeviceOption>,
    option_state: Arc<Mutex<Vec<DeviceOption>>>,
    modified_options: Arc<Mutex<HashSet<String>>>,
    basic_input: BasicInput,
    center_hz: u64,
    span_hz: u64,
    direct_sweep: Option<DirectSweepConfig>,
    rx_context: Option<Arc<RxContext>>,
    prompt_ready: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Segment {
    start_hz: u64,
    stop_hz: u64,
    points: u32,
}

enum ScanResult {
    Complete {
        frequencies_hz: Vec<u64>,
        levels_dbm: Vec<f32>,
        effective_center_hz: u64,
        effective_span_hz: u64,
    },
    Interrupted(Command, anyhow::Result<()>),
}

enum ByteEvent {
    Byte(u8),
    Command(Command),
}

#[derive(Clone, Copy)]
enum ScanDrain {
    Records(u32),
    FrameClosed,
}

fn worker_entry(
    mut port: Box<dyn SerialPort>,
    command_rx: Receiver<Command>,
    init_tx: Sender<anyhow::Result<Initialized>>,
    basic_input: BasicInput,
    settings: TinySaSettings,
    option_state: Arc<Mutex<Vec<DeviceOption>>>,
    modified_options: Arc<Mutex<HashSet<String>>>,
) {
    let (identity, options) = match initialize(&mut *port, basic_input, &settings) {
        Ok(value) => value,
        Err(error) => {
            let _ = init_tx.send(Err(error));
            return;
        }
    };
    *option_state
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = options.clone();
    if init_tx
        .send(Ok(Initialized {
            identity: identity.clone(),
        }))
        .is_err()
    {
        return;
    }
    let center_hz = default_frequency(identity.model, basic_input);
    Worker {
        port,
        command_rx,
        identity,
        options,
        option_state,
        modified_options,
        basic_input,
        center_hz,
        span_hz: DEFAULT_SPAN_HZ,
        direct_sweep: None,
        rx_context: None,
        prompt_ready: true,
    }
    .run();
}

impl Worker {
    fn run(mut self) {
        loop {
            if self.rx_context.is_none() {
                match self.command_rx.recv() {
                    Ok(command) => {
                        if self.handle_command(command) {
                            continue;
                        }
                        return;
                    }
                    Err(_) => return,
                }
            }

            match self.scan_once() {
                Ok(ScanResult::Complete {
                    frequencies_hz,
                    levels_dbm,
                    effective_center_hz,
                    effective_span_hz,
                }) => {
                    update_normal_window(
                        self.direct_sweep,
                        &mut self.center_hz,
                        &mut self.span_hz,
                        effective_center_hz,
                        effective_span_hz,
                    );
                    if let Some(context) = &self.rx_context {
                        let target = if self.direct_sweep.is_some() {
                            PowerTraceTarget::Sweep
                        } else {
                            PowerTraceTarget::Spectrum
                        };
                        let published = context
                            .power_tx
                            .try_send(PowerTrace {
                                target,
                                generation: self
                                    .direct_sweep
                                    .map(|config| config.generation)
                                    .unwrap_or(0),
                                frequencies_hz,
                                levels_dbm,
                                rbw_hz: current_rbw_hz(&self.options),
                            })
                            .is_ok();
                        if published && target == PowerTraceTarget::Spectrum {
                            let mut metrics = context
                                .metrics
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            metrics.radio.frequency = effective_center_hz;
                            metrics.radio.config_sample_rate = effective_span_hz as f64;
                        }
                    }
                    if !self.drain_commands() {
                        return;
                    }
                }
                Ok(ScanResult::Interrupted(command, abort_result)) => {
                    if let Err(error) = abort_result {
                        let message = error.to_string();
                        self.prompt_ready = false;
                        self.stop_acquisition(&message);
                        reject_command(command, anyhow!(message));
                        return;
                    }
                    if !self.handle_command(command) || !self.drain_commands() {
                        return;
                    }
                }
                Err(error) => {
                    best_effort_abort(&mut *self.port);
                    self.prompt_ready = false;
                    self.stop_acquisition(&error.to_string());
                    return;
                }
            }
        }
    }

    fn drain_commands(&mut self) -> bool {
        while let Ok(command) = self.command_rx.try_recv() {
            if !self.handle_command(command) {
                return false;
            }
        }
        true
    }

    fn handle_command(&mut self, command: Command) -> bool {
        match command {
            Command::Start(context, reply) => {
                let result = if !self.prompt_ready {
                    Err(anyhow!(
                        "tinySA serial state is unknown; restart sdrtop, reconnecting the analyzer first if needed"
                    ))
                } else if self.rx_context.is_some() {
                    Err(anyhow!("tinySA acquisition is already running"))
                } else {
                    self.rx_context = Some(context);
                    Ok(())
                };
                let _ = reply.send(result);
            }
            Command::Stop(reply) => {
                self.rx_context = None;
                let _ = reply.send(Ok(()));
            }
            Command::IsStreaming(reply) => {
                let _ = reply.send(Ok(self.rx_context.is_some()));
            }
            Command::SetFrequency(hz, reply) => {
                let (minimum, maximum) = frequency_range(self.identity.model, self.basic_input);
                self.center_hz = hz.clamp(minimum, maximum);
                let _ = reply.send(Ok(()));
            }
            Command::SetSpan(hz, reply) => {
                let (minimum, maximum) = frequency_range(self.identity.model, self.basic_input);
                let maximum = maximum - minimum;
                let result = selected_points(&self.options).and_then(|points| {
                    normalize_span(hz, maximum, points).map(|span_hz| {
                        self.span_hz = span_hz;
                        RateSet::new(
                            hz,
                            Some(self.span_hz as f64),
                            current_rbw_hz(&self.options).unwrap_or(0),
                        )
                    })
                });
                let _ = reply.send(result);
            }
            Command::NoOp(reply) => {
                let _ = reply.send(Ok(()));
            }
            Command::SetDirectSweep(config, reply) => {
                let result = selected_points(&self.options).and_then(|points| {
                    validate_direct_sweep(config, self.identity.model, self.basic_input, points)
                        .map(|()| {
                            self.direct_sweep = config;
                        })
                });
                let _ = reply.send(result);
            }
            Command::SetOption { id, choice, reply } => {
                let result = self.apply_option(&id, &choice);
                let _ = reply.send(result);
            }
            Command::Shutdown(reply) => {
                self.rx_context = None;
                let _ = reply.send(Ok(()));
                return false;
            }
        }
        true
    }

    fn stop_acquisition(&mut self, message: &str) {
        if let Some(context) = self.rx_context.take() {
            let mut metrics = context
                .metrics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            metrics.radio.hw_streaming = false;
            metrics.push_log(format!("tinySA scan error: {message}"));
        }
    }

    fn apply_option(&mut self, id: &str, choice: &str) -> anyhow::Result<()> {
        if !self.prompt_ready {
            bail!(
                "tinySA serial state is unknown; restart sdrtop, reconnecting the analyzer first if needed"
            );
        }
        let prepared = prepare_option_update(
            &self.options,
            self.identity.model,
            id,
            choice,
            self.span_hz,
            self.direct_sweep,
        )?;
        if let Err(error) =
            execute_option_update(&mut self.options, prepared, id, choice, |command| {
                send_setter_command(&mut *self.port, command)
            })
        {
            self.stop_acquisition(&error.to_string());
            let recovery = recover_option_state(
                &mut *self.port,
                self.identity.model,
                self.basic_input,
                &self.options,
            );
            self.prompt_ready = recovery.is_ok();
            return Err(option_update_failure(error, recovery));
        }
        self.modified_options
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(id.to_string());
        *self
            .option_state
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = self.options.clone();
        Ok(())
    }

    fn scan_once(&mut self) -> anyhow::Result<ScanResult> {
        let (minimum_hz, maximum_hz) = frequency_range(self.identity.model, self.basic_input);
        let (start_hz, stop_hz) = self
            .direct_sweep
            .map(|config| (config.start_hz, config.stop_hz))
            .unwrap_or_else(|| {
                centered_window(self.center_hz, self.span_hz, minimum_hz, maximum_hz)
            });
        let points = selected_points(&self.options)?;
        let scan_span_hz = stop_hz - start_hz;
        if scan_span_hz < points as u64 {
            bail!("tinySA scan span is too narrow for {points} points");
        }
        let mut frequencies_hz = Vec::with_capacity(points as usize);
        let mut levels_dbm = Vec::with_capacity(points as usize);
        let segment = Segment {
            start_hz,
            stop_hz,
            points,
        };
        let inactivity_timeout =
            scan_inactivity_timeout(segment, self.identity.model, &self.options)?;
        match scan_segment(
            &mut *self.port,
            &self.command_rx,
            segment,
            self.identity.zero_dbm,
            inactivity_timeout,
        )? {
            ScanResult::Complete {
                frequencies_hz: segment_frequencies,
                levels_dbm: segment_levels,
                ..
            } => {
                frequencies_hz.extend(segment_frequencies);
                levels_dbm.extend(segment_levels);
            }
            interrupted => return Ok(interrupted),
        }
        let target = if self.direct_sweep.is_some() {
            PowerTraceTarget::Sweep
        } else {
            PowerTraceTarget::Spectrum
        };
        let frequencies_hz = trace_frequencies(frequencies_hz, target)?;
        let (effective_center_hz, effective_span_hz) = effective_window(start_hz, stop_hz);
        Ok(ScanResult::Complete {
            frequencies_hz,
            levels_dbm,
            effective_center_hz,
            effective_span_hz,
        })
    }
}

fn option_update_failure(
    setter_error: anyhow::Error,
    recovery: anyhow::Result<()>,
) -> anyhow::Error {
    match recovery {
        Ok(()) => anyhow!(
            "{setter_error:#}; previous accepted tinySA controls were restored and acquisition was stopped"
        ),
        Err(recovery_error) => anyhow!(
            "{setter_error:#}; acquisition was stopped; failed to restore tinySA controls: {recovery_error:#}"
        ),
    }
}

fn initialize(
    port: &mut dyn SerialPort,
    basic_input: BasicInput,
    settings: &TinySaSettings,
) -> anyhow::Result<(Identity, Vec<DeviceOption>)> {
    best_effort_abort(port);
    drain_startup(port)?;
    let mut last_error = None;
    let mut version = None;
    for _ in 0..3 {
        match send_text_command(port, "version") {
            Ok(response)
                if String::from_utf8_lossy(&response)
                    .to_ascii_lowercase()
                    .contains("tinysa") =>
            {
                version = Some(response);
                break;
            }
            Ok(_) => last_error = Some(anyhow!("serial device did not identify as a tinySA")),
            Err(error) => last_error = Some(error),
        }
        let _ = drain_startup(port);
    }
    let version = version
        .ok_or_else(|| last_error.unwrap_or_else(|| anyhow!("tinySA version probe failed")))?;
    send_setter_command(port, "output off")?;
    let info = send_text_command(port, "info")?;
    let help = send_text_command(port, "help")?;
    let zero = send_text_command(port, "zero")?;
    let identity = protocol::parse_identity(&version, &info, &help, &zero)?;
    let (options, saved_commands) = startup_options(identity.model, basic_input, settings)?;
    send_setter_command(port, input_mode_command(identity.model, basic_input))?;
    send_setter_command(port, "abort on")?;
    for command in baseline_commands(identity.model, basic_input) {
        send_setter_command(port, &command)?;
    }
    for command in saved_commands {
        send_setter_command(port, &command)?;
    }
    Ok((identity, options))
}

fn best_effort_abort(port: &mut dyn SerialPort) {
    let _ = port.write_all(b"abort\r");
    let _ = port.flush();
}

fn input_mode_command(model: Model, basic_input: BasicInput) -> &'static str {
    if model.is_ultra() {
        "mode input"
    } else {
        basic_input.mode_command()
    }
}

fn ignored_explicit_basic_input_note(
    model: Model,
    explicit_basic_input: Option<BasicInput>,
) -> Option<String> {
    explicit_basic_input.and_then(|input| {
        model.is_ultra().then(|| {
            format!(
                "tinySA Ultra uses automatic input selection; explicit Basic {} input selection was ignored",
                input.label()
            )
        })
    })
}

fn send_text_command(port: &mut dyn SerialPort, command: &str) -> anyhow::Result<Vec<u8>> {
    port.write_all(command.as_bytes())
        .with_context(|| format!("failed to write tinySA {command} command"))?;
    port.write_all(b"\r")
        .with_context(|| format!("failed to terminate tinySA {command} command"))?;
    port.flush()
        .with_context(|| format!("failed to flush tinySA {command} command"))?;
    let frame = read_until_prompt(port, RESPONSE_TIMEOUT)?;
    protocol::parse_text_frame(&frame, command)
}

fn send_setter_command(port: &mut dyn SerialPort, command: &str) -> anyhow::Result<()> {
    let body = send_text_command(port, command)?;
    protocol::validate_setter_body(&body, command)
}

fn read_until_prompt(
    port: &mut dyn SerialPort,
    inactivity_timeout: Duration,
) -> anyhow::Result<Vec<u8>> {
    let mut response = Vec::new();
    let mut last_byte = Instant::now();
    loop {
        let mut byte = [0u8; 1];
        match port.read(&mut byte) {
            Ok(1) => {
                response.push(byte[0]);
                last_byte = Instant::now();
                if response.ends_with(PROMPT) {
                    return Ok(response);
                }
                if response.len() > MAX_RESPONSE_BYTES {
                    bail!("tinySA response exceeded {MAX_RESPONSE_BYTES} bytes");
                }
            }
            Ok(0) => bail!("tinySA disconnected while returning a response"),
            Ok(_) => unreachable!(),
            Err(error) if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {
                if last_byte.elapsed() >= inactivity_timeout {
                    bail!("timed out waiting for the tinySA shell prompt");
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error).context("failed to read tinySA response"),
        }
    }
}

fn drain_startup(port: &mut dyn SerialPort) -> anyhow::Result<()> {
    let started = Instant::now();
    let mut last_byte = Instant::now();
    let mut buffer = [0u8; 256];
    loop {
        match port.read(&mut buffer) {
            Ok(count) if count > 0 => last_byte = Instant::now(),
            Ok(_) => return Ok(()),
            Err(error) if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {
                if last_byte.elapsed() >= DRAIN_QUIET {
                    return Ok(());
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error).context("failed to drain tinySA startup output"),
        }
        if started.elapsed() >= RESPONSE_TIMEOUT {
            bail!("tinySA startup output did not become idle");
        }
    }
}

fn scan_segment(
    port: &mut dyn SerialPort,
    command_rx: &Receiver<Command>,
    segment: Segment,
    zero_dbm: i32,
    inactivity_timeout: Duration,
) -> anyhow::Result<ScanResult> {
    let command = format!(
        "scanraw {} {} {} 1",
        segment.start_hz, segment.stop_hz, segment.points
    );
    if let Some(command) = poll_scan_command(command_rx)? {
        return Ok(ScanResult::Interrupted(command, Ok(())));
    }
    port.write_all(command.as_bytes())
        .context("failed to write tinySA scan command")?;
    port.write_all(b"\r")
        .context("failed to terminate tinySA scan command")?;
    port.flush()
        .context("failed to flush tinySA scan command")?;
    let mut expected = command.into_bytes();
    expected.extend_from_slice(b"\r\n{");
    let mut frame = Vec::with_capacity(2 + segment.points as usize * 3);
    for (index, expected_byte) in expected.into_iter().enumerate() {
        let byte = next_port_byte(port, inactivity_timeout)?;
        if byte != expected_byte {
            bail!(
                "tinySA scan prelude byte {index} was 0x{byte:02x}, expected 0x{expected_byte:02x}"
            );
        }
    }
    frame.push(b'{');
    for index in 0..segment.points {
        let tag = match next_scan_byte(port, command_rx, inactivity_timeout)? {
            ByteEvent::Byte(byte) => byte,
            ByteEvent::Command(command) => {
                return Ok(ScanResult::Interrupted(
                    command,
                    abort_active_scan(
                        port,
                        ScanDrain::Records(segment.points - index),
                        inactivity_timeout,
                    ),
                ))
            }
        };
        if tag == b'}' {
            bail!(
                "tinySA scan closed after {index} of {} records",
                segment.points
            );
        }
        if tag != b'x' {
            bail!("tinySA scan record {index} has malformed tag 0x{tag:02x}");
        }
        frame.push(tag);
        for _ in 0..2 {
            frame.push(next_port_byte(port, inactivity_timeout)?);
        }
    }
    let close = match next_scan_byte(port, command_rx, inactivity_timeout)? {
        ByteEvent::Byte(byte) => byte,
        ByteEvent::Command(command) => {
            return Ok(ScanResult::Interrupted(
                command,
                abort_active_scan(port, ScanDrain::Records(0), inactivity_timeout),
            ))
        }
    };
    frame.push(close);
    let levels_dbm = protocol::parse_scan_frame(&frame, segment.points, zero_dbm)?;
    for expected_byte in PROMPT {
        match next_scan_byte(port, command_rx, inactivity_timeout)? {
            ByteEvent::Byte(byte) if byte == *expected_byte => {}
            ByteEvent::Byte(byte) => bail!(
                "tinySA emitted text after a scan frame: 0x{byte:02x} before the shell prompt"
            ),
            ByteEvent::Command(command) => {
                return Ok(ScanResult::Interrupted(
                    command,
                    abort_active_scan(port, ScanDrain::FrameClosed, inactivity_timeout),
                ))
            }
        }
    }
    Ok(ScanResult::Complete {
        frequencies_hz: protocol::scan_frequencies(
            segment.start_hz,
            segment.stop_hz,
            segment.points,
        ),
        levels_dbm,
        effective_center_hz: window_center(segment.start_hz, segment.stop_hz),
        effective_span_hz: segment.stop_hz - segment.start_hz,
    })
}

fn next_scan_byte(
    port: &mut dyn SerialPort,
    command_rx: &Receiver<Command>,
    inactivity_timeout: Duration,
) -> anyhow::Result<ByteEvent> {
    let last_byte = Instant::now();
    loop {
        if let Some(command) = poll_scan_command(command_rx)? {
            return Ok(ByteEvent::Command(command));
        }
        let mut byte = [0u8; 1];
        match port.read(&mut byte) {
            Ok(1) => return Ok(ByteEvent::Byte(byte[0])),
            Ok(0) => bail!("tinySA disconnected during a scan"),
            Ok(_) => unreachable!(),
            Err(error) if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {
                if last_byte.elapsed() >= inactivity_timeout {
                    bail!("tinySA scan timed out");
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error).context("failed to read tinySA scan"),
        }
    }
}

fn poll_scan_command(command_rx: &Receiver<Command>) -> anyhow::Result<Option<Command>> {
    loop {
        match command_rx.try_recv() {
            Ok(Command::IsStreaming(reply)) => {
                let _ = reply.send(Ok(true));
            }
            Ok(command) => return Ok(Some(command)),
            Err(TryRecvError::Disconnected) => bail!("tinySA command channel disconnected"),
            Err(TryRecvError::Empty) => return Ok(None),
        }
    }
}

fn next_port_byte(port: &mut dyn SerialPort, inactivity_timeout: Duration) -> anyhow::Result<u8> {
    let last_byte = Instant::now();
    loop {
        let mut byte = [0u8; 1];
        match port.read(&mut byte) {
            Ok(1) => return Ok(byte[0]),
            Ok(0) => bail!("tinySA disconnected during a scan"),
            Ok(_) => unreachable!(),
            Err(error) if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {
                if last_byte.elapsed() >= inactivity_timeout {
                    bail!("tinySA scan timed out");
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error).context("failed to read tinySA scan"),
        }
    }
}

fn abort_active_scan<P>(
    port: &mut P,
    progress: ScanDrain,
    inactivity_timeout: Duration,
) -> anyhow::Result<()>
where
    P: Read + Write + ?Sized,
{
    const ABORT_REPLY: &[u8] = b"abort\r\nch> ";

    port.write_all(b"abort\r")
        .context("failed to send tinySA scan abort")?;
    port.flush().context("failed to flush tinySA scan abort")?;
    let mut records = match progress {
        ScanDrain::Records(records) => Some(records),
        ScanDrain::FrameClosed => None,
    };
    let mut abort_reply_at = 0;
    let mut bytes_read = 0usize;
    let mut last_byte = Instant::now();

    while records.is_some() || abort_reply_at < ABORT_REPLY.len() {
        let byte = read_abort_byte(port, inactivity_timeout, &mut last_byte, &mut bytes_read)?;
        if let Some(remaining) = records {
            match byte {
                b'x' if remaining > 0 => {
                    read_abort_byte(port, inactivity_timeout, &mut last_byte, &mut bytes_read)?;
                    read_abort_byte(port, inactivity_timeout, &mut last_byte, &mut bytes_read)?;
                    records = Some(remaining - 1);
                }
                b'}' => records = None,
                b'a' => {
                    for expected in &ABORT_REPLY[1..] {
                        let byte = read_abort_byte(
                            port,
                            inactivity_timeout,
                            &mut last_byte,
                            &mut bytes_read,
                        )?;
                        if byte != *expected {
                            bail!("tinySA abort acknowledgement was malformed");
                        }
                    }
                    abort_reply_at = ABORT_REPLY.len();
                }
                _ => bail!("tinySA aborted scan frame was malformed"),
            }
        } else if byte == ABORT_REPLY[abort_reply_at] {
            abort_reply_at += 1;
        } else {
            abort_reply_at = usize::from(byte == ABORT_REPLY[0]);
        }
    }
    Ok(())
}

fn read_abort_byte<P>(
    port: &mut P,
    inactivity_timeout: Duration,
    last_byte: &mut Instant,
    bytes_read: &mut usize,
) -> anyhow::Result<u8>
where
    P: Read + ?Sized,
{
    loop {
        let mut byte = [0u8; 1];
        match port.read(&mut byte) {
            Ok(1) => {
                *last_byte = Instant::now();
                *bytes_read += 1;
                if *bytes_read > MAX_RESPONSE_BYTES {
                    bail!("tinySA abort drain exceeded {MAX_RESPONSE_BYTES} bytes");
                }
                return Ok(byte[0]);
            }
            Ok(0) => bail!("tinySA disconnected while aborting a scan"),
            Ok(_) => unreachable!(),
            Err(error) if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {
                if last_byte.elapsed() >= inactivity_timeout {
                    bail!("timed out draining the tinySA aborted scan");
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error).context("failed to drain tinySA aborted scan"),
        }
    }
}

fn reject_command(command: Command, error: anyhow::Error) {
    let message = error.to_string();
    match command {
        Command::Start(_, reply)
        | Command::Stop(reply)
        | Command::SetFrequency(_, reply)
        | Command::NoOp(reply)
        | Command::SetDirectSweep(_, reply)
        | Command::SetOption { reply, .. }
        | Command::Shutdown(reply) => {
            let _ = reply.send(Err(anyhow!(message)));
        }
        Command::IsStreaming(reply) => {
            let _ = reply.send(Err(anyhow!(message)));
        }
        Command::SetSpan(_, reply) => {
            let _ = reply.send(Err(anyhow!(message)));
        }
    }
}

fn validate_direct_sweep(
    config: Option<DirectSweepConfig>,
    model: Model,
    basic_input: BasicInput,
    points: u32,
) -> anyhow::Result<()> {
    let Some(config) = config else {
        return Ok(());
    };
    let (minimum_hz, maximum_hz) = frequency_range(model, basic_input);
    if config.start_hz < minimum_hz
        || config.stop_hz > maximum_hz
        || config.start_hz >= config.stop_hz
    {
        bail!(
            "tinySA sweep must be within {}..{} Hz with start below stop",
            minimum_hz,
            maximum_hz
        );
    }
    if config.stop_hz - config.start_hz < points as u64 {
        bail!("tinySA sweep span is too narrow for {points} points");
    }
    Ok(())
}

fn capabilities(model: Model, basic_input: BasicInput) -> DeviceCapabilities {
    let (minimum_hz, maximum_hz) = frequency_range(model, basic_input);
    DeviceCapabilities {
        acquisition: AcquisitionKind::PowerTrace,
        sample_rate_is_span: true,
        level_unit: LevelUnit::Dbm,
        level_min_db: -120.0,
        level_max_db: 20.0,
        trace_stale_ms: 5_000,
        freq_min_hz: minimum_hz,
        freq_max_hz: maximum_hz,
        sample_rate_min_hz: allowed_points()[0] as f64,
        sample_rate_max_hz: (maximum_hz - minimum_hz) as f64,
        default_frequency_hz: default_frequency(model, basic_input),
        default_sample_rate_hz: DEFAULT_SPAN_HZ as f64,
        sample_geometry: SampleGeometry {
            format: SampleFormat::Int8,
            full_scale: 1.0,
        },
        gain: GainModel::new(Vec::new(), "RF", "RF").with_gauge_fallback(0),
        samples_per_transfer: 0,
        has_bb_filter: true,
        friis_applicable: false,
        delivery: DeliveryModel::Pull,
    }
}

fn frequency_range(model: Model, basic_input: BasicInput) -> (u64, u64) {
    if model.is_ultra() {
        (MIN_FREQUENCY_HZ, model.maximum_hz())
    } else {
        basic_input.range()
    }
}

fn default_frequency(model: Model, basic_input: BasicInput) -> u64 {
    let (minimum_hz, maximum_hz) = frequency_range(model, basic_input);
    DEFAULT_FREQUENCY_HZ.clamp(minimum_hz, maximum_hz)
}

fn centered_window(center_hz: u64, span_hz: u64, minimum_hz: u64, maximum_hz: u64) -> (u64, u64) {
    let span_hz = span_hz.min(maximum_hz - minimum_hz);
    let mut start_hz = center_hz.saturating_sub(span_hz / 2);
    let mut stop_hz = start_hz.saturating_add(span_hz);
    if start_hz < minimum_hz {
        start_hz = minimum_hz;
        stop_hz = minimum_hz + span_hz;
    }

    if stop_hz > maximum_hz {
        stop_hz = maximum_hz;
        start_hz = maximum_hz - span_hz;
    }
    (start_hz, stop_hz)
}

fn window_center(start_hz: u64, stop_hz: u64) -> u64 {
    start_hz + (stop_hz - start_hz) / 2
}

fn effective_window(start_hz: u64, stop_hz: u64) -> (u64, u64) {
    (window_center(start_hz, stop_hz), stop_hz - start_hz)
}

fn update_normal_window(
    direct_sweep: Option<DirectSweepConfig>,
    center_hz: &mut u64,
    span_hz: &mut u64,
    effective_center_hz: u64,
    effective_span_hz: u64,
) {
    if direct_sweep.is_none() {
        *center_hz = effective_center_hz;
        *span_hz = effective_span_hz;
    }
}

fn normalize_span(hz: f64, maximum_hz: u64, points: u32) -> anyhow::Result<u64> {
    if !hz.is_finite() || hz <= 0.0 {
        bail!("tinySA span must be a positive finite value");
    }
    Ok((hz.round() as u64).clamp(points as u64, maximum_hz))
}

fn display_frequencies(measured_hz: &[u64]) -> anyhow::Result<Vec<u64>> {
    let first = *measured_hz
        .first()
        .context("tinySA scan returned no frequencies")?;
    let last = *measured_hz
        .last()
        .context("tinySA scan returned no frequencies")?;
    if measured_hz.windows(2).any(|pair| pair[1] <= pair[0]) {
        bail!("tinySA scan returned duplicate or descending frequencies");
    }
    let intervals = measured_hz.len().saturating_sub(1) as u64;
    let span = last - first;
    if intervals == 0 || span < intervals {
        bail!(
            "tinySA scan span is too narrow for {} points",
            measured_hz.len()
        );
    }
    Ok((0..measured_hz.len())
        .map(|index| {
            let offset = (span as u128 * index as u128 + intervals as u128 / 2) / intervals as u128;
            first + offset as u64
        })
        .collect())
}

fn trace_frequencies(measured_hz: Vec<u64>, target: PowerTraceTarget) -> anyhow::Result<Vec<u64>> {
    if measured_hz.is_empty() {
        bail!("tinySA scan returned no frequencies");
    }
    if measured_hz.windows(2).any(|pair| pair[1] <= pair[0]) {
        bail!("tinySA scan returned duplicate or descending frequencies");
    }
    match target {
        PowerTraceTarget::Spectrum => display_frequencies(&measured_hz),
        PowerTraceTarget::Sweep => Ok(measured_hz),
    }
}

fn validate_settings_shape(settings: &TinySaSettings) -> anyhow::Result<()> {
    if !allowed_points().contains(&settings.points) {
        bail!(
            "[tinysa].points has invalid value {}; allowed values: 64, 128, 290, 450, 900, 1800",
            settings.points
        );
    }
    if !(-100..=100).contains(&settings.ext_gain_db) {
        bail!(
            "[tinysa].ext_gain_db has invalid value {}; allowed range: -100..=100 dB",
            settings.ext_gain_db
        );
    }
    Ok(())
}

fn persisted_settings(
    loaded: &TinySaSettings,
    options: &[DeviceOption],
    model: Model,
    basic_input: BasicInput,
    modified_options: &HashSet<String>,
) -> anyhow::Result<TinySaSettings> {
    let mut settings = loaded.clone();
    for option in options {
        let value = option.selected_choice.as_str();
        validate_option_choice(options, &option.id, value)?;
        if model == Model::Basic
            && matches!(option.id.as_str(), "rbw" | "spur")
            && !modified_options.contains(&option.id)
        {
            continue;
        }
        match option.id.as_str() {
            "points" => {
                settings.points = value.parse().context("tinySA point setting is invalid")?;
            }
            "rbw" => settings.rbw = value.to_string(),
            "attenuation" => settings.attenuation = value.to_string(),
            "high_attenuation" => {
                settings.high_attenuation = match value {
                    "off" => false,
                    "on" => true,
                    _ => bail!("tinySA HIGH attenuation setting is invalid"),
                };
            }
            "lna" => {
                settings.lna = match value {
                    "off" => false,
                    "on" => true,
                    _ => bail!("tinySA LNA setting is invalid"),
                };
            }
            "spur" => settings.spur = value.to_string(),
            "ext_gain" => {
                settings.ext_gain_db = value
                    .parse()
                    .context("tinySA external gain setting is invalid")?;
            }
            id => bail!("unknown tinySA option {id}"),
        }
    }
    if model == Model::Basic {
        settings.basic_input = basic_input;
    }
    validate_settings_shape(&settings)?;
    Ok(settings)
}

fn startup_options(
    model: Model,
    basic_input: BasicInput,
    settings: &TinySaSettings,
) -> anyhow::Result<(Vec<DeviceOption>, Vec<String>)> {
    validate_settings_shape(settings)?;
    let mut options = option_definitions(model, basic_input);
    let rbw = if model == Model::Basic && matches!(settings.rbw.as_str(), "0.2" | "1" | "850") {
        "auto"
    } else {
        &settings.rbw
    };
    let spur = if model == Model::Basic && settings.spur == "auto" {
        "on"
    } else {
        &settings.spur
    };

    let mut values = vec![
        ("points", settings.points.to_string()),
        ("rbw", rbw.to_string()),
    ];
    if model == Model::Basic && basic_input == BasicInput::High {
        values.push((
            "high_attenuation",
            if settings.high_attenuation {
                "on"
            } else {
                "off"
            }
            .to_string(),
        ));
    } else {
        let attenuation = if model.is_ultra() && settings.lna {
            "0"
        } else {
            &settings.attenuation
        };
        values.push(("attenuation", attenuation.to_string()));
    }
    if model.is_ultra() {
        values.push(("lna", if settings.lna { "on" } else { "off" }.to_string()));
    }
    values.extend([
        ("spur", spur.to_string()),
        ("ext_gain", settings.ext_gain_db.to_string()),
    ]);

    let mut commands = Vec::new();
    for (id, choice) in values {
        let option_index = validate_setting_choice(&options, id, &choice)?;
        commands.extend(option_commands(model, &options, id, &choice)?);
        options[option_index].selected_choice = choice;
    }
    Ok((options, commands))
}

fn baseline_commands(model: Model, basic_input: BasicInput) -> Vec<String> {
    if model.is_ultra() {
        strings(&[
            "ultra on",
            "ultra auto",
            "rbw auto",
            "lna off",
            "attenuate auto",
            "spur auto",
            "ext_gain 0",
        ])
    } else if basic_input == BasicInput::High {
        let mut commands = vec!["rbw auto".to_string()];
        commands.extend(high_attenuation_commands("off"));
        commands.extend(strings(&["spur on", "ext_gain 0"]));
        commands
    } else {
        strings(&["rbw auto", "attenuate auto", "spur on", "ext_gain 0"])
    }
}

fn option_definitions(model: Model, basic_input: BasicInput) -> Vec<DeviceOption> {
    let mut options = vec![
        option(
            "points",
            "Points",
            allowed_points().map(|value| value.to_string()).to_vec(),
        ),
        option(
            "rbw",
            "RBW (kHz)",
            if model.is_ultra() {
                strings(&[
                    "auto", "0.2", "1", "3", "10", "30", "100", "300", "600", "850",
                ])
            } else {
                strings(&["auto", "3", "10", "30", "100", "300", "600"])
            },
        ),
    ];
    if model == Model::Basic && basic_input == BasicInput::High {
        options.push(option(
            "high_attenuation",
            "Coarse attenuation",
            strings(&["off", "on"]),
        ));
    } else {
        options.push(integer_option(
            "attenuation",
            "Attenuation (dB)",
            std::iter::once("auto".to_string())
                .chain((0..=31).map(|value| value.to_string()))
                .collect(),
            0..=31,
        ));
    }
    if model.is_ultra() {
        options.push(option("lna", "LNA", strings(&["off", "on"])));
    }
    options.push(option(
        "spur",
        "Spur removal",
        if model.is_ultra() {
            strings(&["off", "on", "auto"])
        } else {
            strings(&["off", "on"])
        },
    ));
    options.push(integer_option(
        "ext_gain",
        "External gain (dB)",
        (-100..=100).map(|value| value.to_string()).collect(),
        -100..=100,
    ));
    options
}

fn option(id: &str, label: &str, choices: Vec<String>) -> DeviceOption {
    DeviceOption {
        id: id.to_string(),
        label: label.to_string(),
        selected_choice: choices.first().cloned().unwrap_or_default(),
        choices,
        integer_range: None,
    }
}

fn integer_option(
    id: &str,
    label: &str,
    choices: Vec<String>,
    range: std::ops::RangeInclusive<i32>,
) -> DeviceOption {
    DeviceOption {
        integer_range: Some(range),
        ..option(id, label, choices)
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn allowed_points() -> [u32; 6] {
    [64, 128, 290, 450, 900, 1800]
}

fn validate_option_choice(
    options: &[DeviceOption],
    id: &str,
    choice: &str,
) -> anyhow::Result<usize> {
    let option_index = options
        .iter()
        .position(|option| option.id == id)
        .with_context(|| format!("unknown tinySA option {id}"))?;
    if !options[option_index]
        .choices
        .iter()
        .any(|candidate| candidate == choice)
    {
        bail!("invalid choice {choice:?} for tinySA option {id}");
    }
    Ok(option_index)
}

fn validate_setting_choice(
    options: &[DeviceOption],
    id: &str,
    choice: &str,
) -> anyhow::Result<usize> {
    let option_index = options
        .iter()
        .position(|option| option.id == id)
        .with_context(|| format!("unknown tinySA option {id}"))?;
    if options[option_index]
        .choices
        .iter()
        .any(|candidate| candidate == choice)
    {
        return Ok(option_index);
    }
    let key = match id {
        "ext_gain" => "ext_gain_db",
        other => other,
    };
    let allowed = match id {
        "ext_gain" => "-100..=100".to_string(),
        "attenuation" => "auto or 0..=31".to_string(),
        _ => options[option_index].choices.join(", "),
    };
    bail!("[tinysa].{key} has invalid value {choice:?}; allowed values: {allowed}");
}

fn selected_option_value<'a>(options: &'a [DeviceOption], id: &str) -> Option<&'a str> {
    options
        .iter()
        .find(|option| option.id == id)
        .map(|option| option.selected_choice.as_str())
}

fn set_selected_option(options: &mut [DeviceOption], id: &str, choice: &str) -> anyhow::Result<()> {
    let option_index = validate_option_choice(options, id, choice)?;
    options[option_index].selected_choice = choice.to_string();
    Ok(())
}

fn option_commands(
    model: Model,
    options: &[DeviceOption],
    id: &str,
    choice: &str,
) -> anyhow::Result<Vec<String>> {
    validate_option_choice(options, id, choice)?;
    let commands = match id {
        "points" => Vec::new(),
        "rbw" => vec![format!("rbw {choice}")],
        "attenuation" => manual_attenuation_commands(choice),
        "high_attenuation" => high_attenuation_commands(choice),
        "lna" if model.is_ultra() => vec![format!("lna {choice}")],
        "spur" => vec![format!("spur {choice}")],
        "ext_gain" => vec![format!("ext_gain {choice}")],
        _ => bail!("unknown tinySA option {id}"),
    };
    Ok(commands)
}

fn manual_attenuation_commands(choice: &str) -> Vec<String> {
    if choice == "auto" {
        return vec!["attenuate auto".to_string()];
    }
    let target: u8 = choice
        .parse()
        .expect("validated tinySA attenuation choice must be numeric");
    // Use a high attenuation transition because firmware ignores an equal numeric value
    let transition = if target == 31 { 30 } else { 31 };
    vec![
        format!("attenuate {transition}"),
        format!("attenuate {target}"),
    ]
}

fn high_attenuation_commands(choice: &str) -> Vec<String> {
    let mut commands = vec!["attenuate 1".to_string(), "attenuate 0".to_string()];
    if choice == "on" {
        commands.push("attenuate 1".to_string());
    }
    commands
}

struct PreparedOption {
    option_index: usize,
    commands: Vec<String>,
}

fn prepare_option_update(
    options: &[DeviceOption],
    model: Model,
    id: &str,
    choice: &str,
    span_hz: u64,
    direct_sweep: Option<DirectSweepConfig>,
) -> anyhow::Result<PreparedOption> {
    let option_index = validate_option_choice(options, id, choice)?;
    if id == "attenuation" && choice != "0" && selected_option_value(options, "lna") == Some("on") {
        bail!("turn the tinySA LNA off before changing attenuation");
    }
    if id == "points" {
        let points: u32 = choice.parse().context("tinySA point setting is invalid")?;
        if span_hz < points as u64 {
            bail!("tinySA span is too narrow for {points} points");
        }
        if direct_sweep.is_some_and(|config| config.stop_hz - config.start_hz < points as u64) {
            bail!("tinySA sweep span is too narrow for {points} points");
        }
    }
    let lna_is_on = selected_option_value(options, "lna") == Some("on");
    let mut commands = Vec::new();
    if id == "lna" && choice == "on" {
        commands.extend(manual_attenuation_commands("0"));
    }
    if id == "attenuation" && lna_is_on {
        commands.push("lna off".to_string());
        commands.extend(option_commands(model, options, id, choice)?);
        commands.push("lna on".to_string());
    } else {
        commands.extend(option_commands(model, options, id, choice)?);
    }
    Ok(PreparedOption {
        option_index,
        commands,
    })
}

fn commit_option_update(
    options: &mut [DeviceOption],
    option_index: usize,
    id: &str,
    choice: &str,
) -> anyhow::Result<()> {
    options[option_index].selected_choice = choice.to_string();
    if id == "lna" && choice == "on" {
        set_selected_option(options, "attenuation", "0")?;
    }
    Ok(())
}

fn execute_option_update(
    options: &mut [DeviceOption],
    prepared: PreparedOption,
    id: &str,
    choice: &str,
    mut send: impl FnMut(&str) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    for command in &prepared.commands {
        send(command)?;
    }
    commit_option_update(options, prepared.option_index, id, choice)
}

fn selected_points(options: &[DeviceOption]) -> anyhow::Result<u32> {
    selected_option_value(options, "points")
        .context("tinySA point setting is missing")?
        .parse()
        .context("tinySA point setting is invalid")
}

fn current_rbw_hz(options: &[DeviceOption]) -> Option<u32> {
    let value = selected_option_value(options, "rbw")?;
    if value == "auto" {
        return None;
    }
    value
        .parse::<f64>()
        .ok()
        .map(|khz| (khz * 1_000.0).round() as u32)
}

fn basic_option_commands(options: &[DeviceOption]) -> anyhow::Result<Vec<String>> {
    let mut commands = Vec::new();
    let attenuation_id = if options.iter().any(|option| option.id == "high_attenuation") {
        "high_attenuation"
    } else {
        "attenuation"
    };
    for id in ["rbw", attenuation_id, "spur", "ext_gain"] {
        let choice = selected_option_value(options, id)
            .with_context(|| format!("tinySA {id} is missing"))?;
        commands.extend(option_commands(Model::Basic, options, id, choice)?);
    }
    Ok(commands)
}

fn restore_option_commands(model: Model, options: &[DeviceOption]) -> anyhow::Result<Vec<String>> {
    if !model.is_ultra() {
        return basic_option_commands(options);
    }

    let mut commands = vec!["lna off".to_string()];
    for id in ["rbw", "attenuation"] {
        let choice = selected_option_value(options, id)
            .with_context(|| format!("tinySA {id} is missing"))?;
        commands.extend(option_commands(model, options, id, choice)?);
    }
    if selected_option_value(options, "lna") == Some("on") {
        commands.push("lna on".to_string());
    }
    for id in ["spur", "ext_gain"] {
        let choice = selected_option_value(options, id)
            .with_context(|| format!("tinySA {id} is missing"))?;
        commands.extend(option_commands(model, options, id, choice)?);
    }
    Ok(commands)
}

fn recover_option_state(
    port: &mut dyn SerialPort,
    model: Model,
    basic_input: BasicInput,
    options: &[DeviceOption],
) -> anyhow::Result<()> {
    best_effort_abort(port);
    drain_startup(port)?;
    send_setter_command(port, input_mode_command(model, basic_input))?;
    send_setter_command(port, "abort on")?;
    for command in baseline_commands(model, basic_input) {
        send_setter_command(port, &command)?;
    }
    for command in restore_option_commands(model, options)? {
        send_setter_command(port, &command)?;
    }
    Ok(())
}

fn scan_inactivity_timeout(
    segment: Segment,
    model: Model,
    options: &[DeviceOption],
) -> anyhow::Result<Duration> {
    let rbw_setting = selected_option_value(options, "rbw").context("tinySA RBW is missing")?;
    let span_hz = segment.stop_hz.saturating_sub(segment.start_hz) as f64;
    let (minimum_rbw, maximum_rbw) = if model.is_ultra() {
        (0.2, 850.0)
    } else {
        (3.0, 600.0)
    };
    let rbw_khz = if rbw_setting == "auto" {
        (span_hz * 7e-6).clamp(minimum_rbw, maximum_rbw)
    } else {
        rbw_setting
            .parse::<f64>()
            .context("tinySA RBW setting is invalid")?
    };
    let points = segment.points.max(1) as f64;
    let mut total_seconds = (span_hz / 20_000.0) / rbw_khz.powi(2) + points / 500.0;
    let spur = selected_option_value(options, "spur").context("tinySA spur setting is missing")?;
    if (spur == "on" && segment.stop_hz > 800_000_000) || spur == "auto" {
        total_seconds *= 2.0;
    }
    let block_seconds = total_seconds * 20.0 / points + 1.0;
    Ok(Duration::from_secs_f64(
        block_seconds.max(RESPONSE_TIMEOUT.as_secs_f64()),
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io;

    use super::*;

    enum ReadStep {
        Bytes(VecDeque<u8>),
        Delay(Duration),
        FrameClose,
    }

    struct ScriptedPeer {
        reads: VecDeque<ReadStep>,
        writes: Vec<u8>,
        frame_closed: bool,
    }

    impl ScriptedPeer {
        fn new(steps: impl IntoIterator<Item = ReadStep>) -> Self {
            Self {
                reads: steps.into_iter().collect(),
                writes: Vec::new(),
                frame_closed: false,
            }
        }
    }

    impl Read for ScriptedPeer {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            loop {
                match self.reads.front_mut() {
                    Some(ReadStep::Bytes(bytes)) => {
                        let Some(byte) = bytes.pop_front() else {
                            self.reads.pop_front();
                            continue;
                        };
                        buffer[0] = byte;
                        return Ok(1);
                    }
                    Some(ReadStep::Delay(duration)) => {
                        let duration = *duration;
                        self.reads.pop_front();
                        std::thread::sleep(duration);
                        return Err(io::Error::new(ErrorKind::TimedOut, "scripted delay"));
                    }
                    Some(ReadStep::FrameClose) => {
                        self.reads.pop_front();
                        self.frame_closed = true;
                        buffer[0] = b'}';
                        return Ok(1);
                    }
                    None => return Err(io::Error::new(ErrorKind::TimedOut, "script exhausted")),
                }
            }
        }
    }

    impl Write for ScriptedPeer {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if buffer == b"next\r" {
                assert!(
                    self.frame_closed,
                    "next request preceded the old frame close"
                );
            }
            self.writes.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn bytes(value: &[u8]) -> ReadStep {
        ReadStep::Bytes(value.iter().copied().collect())
    }

    #[cfg(unix)]
    #[test]
    fn unknown_serial_state_rejects_start_and_options_without_device_writes() {
        let (worker_port, peer_port) = serialport::TTYPort::pair().unwrap();
        let (_command_tx, command_rx) = crossbeam_channel::unbounded();
        let options = option_definitions(Model::Basic, BasicInput::Low);
        let mut worker = Worker {
            port: Box::new(worker_port),
            command_rx,
            identity: Identity {
                firmware: "test".into(),
                hardware: None,
                board: "test".into(),
                zero_dbm: 0,
                model: Model::Basic,
            },
            option_state: Arc::new(Mutex::new(options.clone())),
            modified_options: Arc::new(Mutex::new(HashSet::new())),
            options,
            basic_input: BasicInput::Low,
            center_hz: default_frequency(Model::Basic, BasicInput::Low),
            span_hz: DEFAULT_SPAN_HZ,
            direct_sweep: None,
            rx_context: None,
            prompt_ready: false,
        };
        let state = Arc::new(Mutex::new(crate::state::SdrMetrics::fixture()));
        let (sample_tx, _) = crossbeam_channel::bounded(1);
        let (demod_tx, _) = crossbeam_channel::bounded(1);
        let (net_tx, _) = crossbeam_channel::bounded(1);
        let (power_tx, _) = crossbeam_channel::bounded(1);
        let context = Arc::new(RxContext {
            metrics: state,
            sample_tx,
            fft_feed: crate::hardware::FeedHealth::default(),
            demod_tx,
            net_tx,
            net_feed: crate::hardware::FeedHealth::default(),
            power_tx,
            geometry: capabilities(Model::Basic, BasicInput::Low).sample_geometry,
            blocks_seen: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        });

        let (start_tx, start_rx) = bounded(1);
        assert!(worker.handle_command(Command::Start(context, start_tx)));
        let start_error = start_rx.recv().unwrap().unwrap_err().to_string();
        assert!(start_error.contains("restart sdrtop"));
        assert!(worker.rx_context.is_none());

        let (option_tx, option_rx) = bounded(1);
        assert!(worker.handle_command(Command::SetOption {
            id: "spur".into(),
            choice: "off".into(),
            reply: option_tx,
        }));
        let option_error = option_rx.recv().unwrap().unwrap_err().to_string();
        assert!(option_error.contains("restart sdrtop"));
        assert_eq!(peer_port.bytes_to_read().unwrap(), 0);

        let (shutdown_tx, shutdown_rx) = bounded(1);
        assert!(!worker.handle_command(Command::Shutdown(shutdown_tx)));
        shutdown_rx.recv().unwrap().unwrap();
    }

    #[test]
    fn cancellation_waits_for_the_delayed_frame_close_before_the_next_request() {
        let mut peer = ScriptedPeer::new([
            bytes(b"x}xxch"),
            bytes(b"abort\r\nch> "),
            ReadStep::Delay(Duration::from_millis(225)),
            ReadStep::FrameClose,
        ]);
        let started = Instant::now();

        abort_active_scan(&mut peer, ScanDrain::Records(2), Duration::from_secs(1)).unwrap();
        peer.write_all(b"next\r").unwrap();

        assert!(started.elapsed() >= Duration::from_millis(200));
        assert_eq!(peer.writes, b"abort\rnext\r");
    }

    #[test]
    fn cancellation_after_the_frame_waits_only_for_the_abort_reply() {
        let mut peer = ScriptedPeer::new([
            bytes(PROMPT),
            ReadStep::Delay(Duration::from_millis(225)),
            bytes(b"abort\r\nch> "),
        ]);
        let started = Instant::now();

        abort_active_scan(&mut peer, ScanDrain::FrameClosed, Duration::from_secs(1)).unwrap();

        assert!(started.elapsed() >= Duration::from_millis(200));
        assert_eq!(peer.writes, b"abort\r");
    }

    #[test]
    fn cancellation_times_out_when_the_active_frame_never_closes() {
        let mut peer = ScriptedPeer::new([bytes(b"abort\r\nch> ")]);

        assert!(
            abort_active_scan(&mut peer, ScanDrain::Records(1), Duration::from_millis(10),)
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
    }

    #[test]
    fn basic_inputs_expose_only_their_physical_connector_range() {
        assert_eq!(
            frequency_range(Model::Basic, BasicInput::Low),
            (100_000, 350_000_000)
        );
        assert_eq!(
            frequency_range(Model::Basic, BasicInput::High),
            (240_000_000, 959_000_000)
        );
        assert_eq!(
            default_frequency(Model::Basic, BasicInput::High),
            240_000_000
        );
    }

    #[test]
    fn option_definitions_match_each_model() {
        let basic = option_definitions(Model::Basic, BasicInput::Low);
        assert_eq!(
            basic
                .iter()
                .find(|option| option.id == "points")
                .unwrap()
                .choices,
            ["64", "128", "290", "450", "900", "1800"]
        );
        assert_eq!(
            basic
                .iter()
                .find(|option| option.id == "rbw")
                .unwrap()
                .choices,
            ["auto", "3", "10", "30", "100", "300", "600"]
        );
        let low_attenuation = basic
            .iter()
            .find(|option| option.id == "attenuation")
            .unwrap();
        assert_eq!(low_attenuation.label, "Attenuation (dB)");
        assert_eq!(low_attenuation.choices.first().unwrap(), "auto");
        assert_eq!(low_attenuation.choices.last().unwrap(), "31");
        assert_eq!(low_attenuation.integer_range, Some(0..=31));
        assert!(basic
            .iter()
            .filter(|option| option.id != "attenuation" && option.id != "ext_gain")
            .all(|option| option.integer_range.is_none()));
        assert!(!basic.iter().any(|option| option.id == "lna"));
        assert_eq!(
            basic
                .iter()
                .find(|option| option.id == "spur")
                .unwrap()
                .choices,
            ["off", "on"]
        );

        let high = option_definitions(Model::Basic, BasicInput::High);
        let high_attenuation = high
            .iter()
            .find(|option| option.id == "high_attenuation")
            .unwrap();
        assert_eq!(high_attenuation.label, "Coarse attenuation");
        assert_eq!(high_attenuation.choices, ["off", "on"]);
        assert!(high_attenuation.integer_range.is_none());
        assert!(!high.iter().any(|option| option.id == "attenuation"));

        let ultra = option_definitions(Model::Zs407, BasicInput::Low);
        assert_eq!(
            ultra
                .iter()
                .find(|option| option.id == "rbw")
                .unwrap()
                .choices,
            ["auto", "0.2", "1", "3", "10", "30", "100", "300", "600", "850"]
        );
        assert!(ultra.iter().any(|option| option.id == "lna"));
        assert!(!ultra.iter().any(|option| option.id == "lna2"));
        assert!(!ultra.iter().any(|option| option.id == "agc"));
        let external_gain = ultra.iter().find(|option| option.id == "ext_gain").unwrap();
        assert_eq!(external_gain.choices.len(), 201);
        assert_eq!(external_gain.integer_range, Some(-100..=100));
    }

    #[test]
    fn startup_sets_a_safe_baseline_before_saved_values() {
        assert_eq!(
            baseline_commands(Model::Zs407, BasicInput::Low),
            [
                "ultra on",
                "ultra auto",
                "rbw auto",
                "lna off",
                "attenuate auto",
                "spur auto",
                "ext_gain 0",
            ]
        );
        assert_eq!(
            baseline_commands(Model::Basic, BasicInput::Low),
            ["rbw auto", "attenuate auto", "spur on", "ext_gain 0"]
        );
        assert_eq!(
            baseline_commands(Model::Basic, BasicInput::High),
            [
                "rbw auto",
                "attenuate 1",
                "attenuate 0",
                "spur on",
                "ext_gain 0",
            ]
        );

        let settings = TinySaSettings {
            basic_input: BasicInput::Low,
            points: 900,
            rbw: "0.2".into(),
            attenuation: "12".into(),
            high_attenuation: false,
            lna: true,
            spur: "off".into(),
            ext_gain_db: -7,
        };
        let (options, commands) =
            startup_options(Model::Zs407, BasicInput::Low, &settings).unwrap();
        assert_eq!(
            commands,
            [
                "rbw 0.2",
                "attenuate 31",
                "attenuate 0",
                "lna on",
                "spur off",
                "ext_gain -7",
            ]
        );
        assert_eq!(selected_points(&options).unwrap(), 900);
        assert_eq!(selected_option_value(&options, "attenuation"), Some("0"));
        assert_eq!(selected_option_value(&options, "lna"), Some("on"));
    }

    #[test]
    fn basic_saved_values_normalize_only_documented_choices() {
        for rbw in ["0.2", "1", "850"] {
            let settings = TinySaSettings {
                rbw: rbw.into(),
                spur: "auto".into(),
                ..TinySaSettings::default()
            };
            let (options, commands) =
                startup_options(Model::Basic, BasicInput::Low, &settings).unwrap();
            assert_eq!(selected_option_value(&options, "rbw"), Some("auto"));
            assert_eq!(selected_option_value(&options, "spur"), Some("on"));
            assert!(commands.iter().any(|command| command == "rbw auto"));
            assert!(commands.iter().any(|command| command == "spur on"));
        }

        assert!(startup_options(
            Model::Basic,
            BasicInput::Low,
            &TinySaSettings {
                attenuation: "32".into(),
                ..TinySaSettings::default()
            },
        )
        .is_err());
        assert!(startup_options(
            Model::Basic,
            BasicInput::Low,
            &TinySaSettings {
                rbw: "2".into(),
                ..TinySaSettings::default()
            },
        )
        .is_err());
        assert!(startup_options(
            Model::Basic,
            BasicInput::Low,
            &TinySaSettings {
                spur: "maybe".into(),
                ..TinySaSettings::default()
            },
        )
        .is_err());
        assert!(validate_settings_shape(&TinySaSettings {
            points: 451,
            ..TinySaSettings::default()
        })
        .is_err());
        assert!(validate_settings_shape(&TinySaSettings {
            ext_gain_db: 101,
            ..TinySaSettings::default()
        })
        .is_err());
    }

    #[test]
    fn option_commands_are_whitelisted_after_choice_validation() {
        let basic = option_definitions(Model::Basic, BasicInput::Low);
        assert_eq!(
            option_commands(Model::Basic, &basic, "points", "450").unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            option_commands(Model::Basic, &basic, "rbw", "10").unwrap(),
            ["rbw 10"]
        );
        assert!(option_commands(Model::Basic, &basic, "rbw", "10; reset").is_err());
        assert!(option_commands(Model::Basic, &basic, "lna", "on").is_err());
        assert!(option_commands(Model::Basic, &basic, "missing", "on").is_err());

        let high = option_definitions(Model::Basic, BasicInput::High);
        assert_eq!(
            option_commands(Model::Basic, &high, "high_attenuation", "off").unwrap(),
            ["attenuate 1", "attenuate 0"]
        );
        assert_eq!(
            option_commands(Model::Basic, &high, "high_attenuation", "on").unwrap(),
            ["attenuate 1", "attenuate 0", "attenuate 1"]
        );
        assert!(option_commands(Model::Basic, &high, "high_attenuation", "auto").is_err());
    }

    #[test]
    fn manual_attenuation_plans_clear_firmware_auto_mode() {
        struct FirmwareAttenuation {
            value: u8,
            automatic: bool,
            high_input: bool,
        }

        impl FirmwareAttenuation {
            fn apply(&mut self, command: &str) {
                let choice = command.strip_prefix("attenuate ").unwrap();
                if choice == "auto" {
                    self.value = if self.high_input { 0 } else { 30 };
                    self.automatic = true;
                    return;
                }
                let value = choice.parse().unwrap();
                if self.value == value {
                    return;
                }
                self.value = value;
                if !self.high_input || value == 0 {
                    self.automatic = false;
                }
            }
        }

        for (high_input, target, commands) in [
            (false, 30, manual_attenuation_commands("30")),
            (false, 31, manual_attenuation_commands("31")),
            (true, 0, high_attenuation_commands("off")),
            (true, 1, high_attenuation_commands("on")),
        ] {
            let mut firmware = FirmwareAttenuation {
                value: if high_input { 0 } else { 30 },
                automatic: true,
                high_input,
            };
            for command in commands {
                firmware.apply(&command);
            }
            assert_eq!(firmware.value, target);
            assert!(!firmware.automatic);
        }
    }

    #[test]
    fn basic_low_startup_and_recovery_force_manual_attenuation() {
        let settings = TinySaSettings {
            attenuation: "30".into(),
            ..TinySaSettings::default()
        };
        let (options, startup) = startup_options(Model::Basic, BasicInput::Low, &settings).unwrap();

        assert_eq!(
            startup,
            [
                "rbw auto",
                "attenuate 31",
                "attenuate 30",
                "spur on",
                "ext_gain 0",
            ]
        );
        assert_eq!(
            restore_option_commands(Model::Basic, &options).unwrap(),
            startup
        );
    }

    #[test]
    fn lna_forces_zero_attenuation_and_blocks_other_values() {
        let (mut options, _) =
            startup_options(Model::Zs407, BasicInput::Low, &TinySaSettings::default()).unwrap();
        set_selected_option(&mut options, "attenuation", "12").unwrap();
        let prepared =
            prepare_option_update(&options, Model::Zs407, "lna", "on", 10_000, None).unwrap();
        let mut commands = Vec::new();
        execute_option_update(&mut options, prepared, "lna", "on", |command| {
            commands.push(command.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!(commands, ["attenuate 31", "attenuate 0", "lna on"]);
        assert_eq!(selected_option_value(&options, "attenuation"), Some("0"));
        assert!(
            prepare_option_update(&options, Model::Zs407, "attenuation", "auto", 10_000, None,)
                .is_err()
        );
        assert!(
            prepare_option_update(&options, Model::Zs407, "attenuation", "1", 10_000, None,)
                .is_err()
        );
        assert!(
            prepare_option_update(&options, Model::Zs407, "attenuation", "0", 10_000, None,)
                .is_ok()
        );

        let prepared =
            prepare_option_update(&options, Model::Zs407, "attenuation", "0", 10_000, None)
                .unwrap();
        assert_eq!(
            prepared.commands,
            ["lna off", "attenuate 31", "attenuate 0", "lna on",]
        );
    }

    #[test]
    fn failed_setter_commands_do_not_change_option_state() {
        let (mut options, _) =
            startup_options(Model::Zs407, BasicInput::Low, &TinySaSettings::default()).unwrap();
        let before = options.clone();
        let prepared =
            prepare_option_update(&options, Model::Zs407, "rbw", "30", 10_000, None).unwrap();
        let result = execute_option_update(&mut options, prepared, "rbw", "30", |_| {
            bail!("injected serial failure")
        });
        assert!(result.is_err());
        assert_eq!(options, before);
    }

    #[test]
    fn failed_lna_activation_does_not_publish_dependent_state() {
        let (mut options, _) =
            startup_options(Model::Zs407, BasicInput::Low, &TinySaSettings::default()).unwrap();
        set_selected_option(&mut options, "attenuation", "12").unwrap();
        let before = options.clone();
        let prepared =
            prepare_option_update(&options, Model::Zs407, "lna", "on", 10_000, None).unwrap();
        let mut commands = Vec::new();
        let result = execute_option_update(&mut options, prepared, "lna", "on", |command| {
            commands.push(command.to_string());
            if command == "lna on" {
                bail!("injected serial failure");
            }
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(commands, ["attenuate 31", "attenuate 0", "lna on"]);
        assert_eq!(options, before);
    }

    #[test]
    fn restoring_ultra_controls_reestablishes_cached_dependencies() {
        let settings = TinySaSettings {
            rbw: "30".into(),
            attenuation: "12".into(),
            spur: "off".into(),
            ext_gain_db: -7,
            ..TinySaSettings::default()
        };
        let (options, _) = startup_options(Model::Zs407, BasicInput::Low, &settings).unwrap();
        assert_eq!(
            restore_option_commands(Model::Zs407, &options).unwrap(),
            [
                "lna off",
                "rbw 30",
                "attenuate 31",
                "attenuate 12",
                "spur off",
                "ext_gain -7",
            ]
        );

        let lna_settings = TinySaSettings {
            lna: true,
            ..settings
        };
        let (options, _) = startup_options(Model::Zs407, BasicInput::Low, &lna_settings).unwrap();
        assert_eq!(
            restore_option_commands(Model::Zs407, &options).unwrap(),
            [
                "lna off",
                "rbw 30",
                "attenuate 31",
                "attenuate 0",
                "lna on",
                "spur off",
                "ext_gain -7",
            ]
        );
    }

    #[test]
    fn ultra_persistence_uses_authoritative_dependent_settings() {
        let loaded = TinySaSettings {
            basic_input: BasicInput::High,
            attenuation: "12".into(),
            lna: true,
            ..TinySaSettings::default()
        };
        let (mut options, _) =
            startup_options(Model::Zs407, BasicInput::Low, &TinySaSettings::default()).unwrap();
        for (id, choice) in [
            ("points", "900"),
            ("rbw", "0.2"),
            ("attenuation", "0"),
            ("lna", "on"),
            ("spur", "off"),
            ("ext_gain", "-12"),
        ] {
            set_selected_option(&mut options, id, choice).unwrap();
        }

        let modified = HashSet::from([
            "points".to_string(),
            "rbw".to_string(),
            "attenuation".to_string(),
            "lna".to_string(),
            "spur".to_string(),
            "ext_gain".to_string(),
        ]);
        let saved = persisted_settings(&loaded, &options, Model::Zs407, BasicInput::Low, &modified)
            .unwrap();

        assert_eq!(saved.points, 900);
        assert_eq!(saved.rbw, "0.2");
        assert_eq!(saved.attenuation, "0");
        assert!(saved.lna);
        assert_eq!(saved.basic_input, BasicInput::High);
        assert_eq!(saved.spur, "off");
        assert_eq!(saved.ext_gain_db, -12);
    }

    #[test]
    fn config_hook_owns_the_resolved_basic_input_and_option_snapshot() {
        let loaded = TinySaSettings {
            basic_input: BasicInput::Low,
            attenuation: "12".into(),
            high_attenuation: true,
            lna: true,
            ..TinySaSettings::default()
        };
        let (mut options, _) = startup_options(Model::Basic, BasicInput::High, &loaded).unwrap();
        set_selected_option(&mut options, "high_attenuation", "off").unwrap();
        let (command_tx, command_rx) = crossbeam_channel::unbounded();
        drop(command_rx);
        let device = TinySaDevice {
            caps: capabilities(Model::Basic, BasicInput::High),
            info: DeviceInfo::default(),
            notes: Vec::new(),
            model: Model::Basic,
            basic_input: BasicInput::High,
            options: Arc::new(Mutex::new(options)),
            modified_options: Arc::new(Mutex::new(HashSet::from(["high_attenuation".to_string()]))),
            command_tx,
            worker: Mutex::new(None),
        };
        let mut config = crate::config::AppConfig {
            tinysa: loaded,
            ..crate::config::AppConfig::default()
        };

        device.update_config(&mut config).unwrap();

        assert_eq!(config.tinysa.basic_input, BasicInput::High);
        assert_eq!(config.tinysa.attenuation, "12");
        assert!(!config.tinysa.high_attenuation);
        assert!(config.tinysa.lna);
    }

    #[test]
    fn low_and_high_attenuation_persist_independently() {
        let loaded = TinySaSettings {
            basic_input: BasicInput::Low,
            attenuation: "12".into(),
            high_attenuation: true,
            ..TinySaSettings::default()
        };

        let (mut low_options, _) = startup_options(Model::Basic, BasicInput::Low, &loaded).unwrap();
        set_selected_option(&mut low_options, "attenuation", "7").unwrap();
        let low_saved = persisted_settings(
            &loaded,
            &low_options,
            Model::Basic,
            BasicInput::Low,
            &HashSet::from(["attenuation".to_string()]),
        )
        .unwrap();
        assert_eq!(low_saved.basic_input, BasicInput::Low);
        assert_eq!(low_saved.attenuation, "7");
        assert!(low_saved.high_attenuation);

        let (mut high_options, _) =
            startup_options(Model::Basic, BasicInput::High, &loaded).unwrap();
        set_selected_option(&mut high_options, "high_attenuation", "off").unwrap();
        let high_saved = persisted_settings(
            &loaded,
            &high_options,
            Model::Basic,
            BasicInput::High,
            &HashSet::from(["high_attenuation".to_string()]),
        )
        .unwrap();
        assert_eq!(high_saved.basic_input, BasicInput::High);
        assert_eq!(high_saved.attenuation, "12");
        assert!(!high_saved.high_attenuation);
    }

    #[test]
    fn basic_normalization_persists_only_after_successful_operator_updates() {
        for rbw in ["0.2", "1", "850"] {
            let loaded = TinySaSettings {
                rbw: rbw.into(),
                spur: "auto".into(),
                ..TinySaSettings::default()
            };
            let (options, _) = startup_options(Model::Basic, BasicInput::Low, &loaded).unwrap();
            assert_eq!(selected_option_value(&options, "rbw"), Some("auto"));
            assert_eq!(selected_option_value(&options, "spur"), Some("on"));

            let unchanged = persisted_settings(
                &loaded,
                &options,
                Model::Basic,
                BasicInput::Low,
                &HashSet::new(),
            )
            .unwrap();
            assert_eq!(unchanged.rbw, rbw);
            assert_eq!(unchanged.spur, "auto");

            let modified = HashSet::from(["rbw".to_string(), "spur".to_string()]);
            let changed =
                persisted_settings(&loaded, &options, Model::Basic, BasicInput::Low, &modified)
                    .unwrap();
            assert_eq!(changed.rbw, "auto");
            assert_eq!(changed.spur, "on");
        }
    }

    #[test]
    fn option_failure_reports_successful_and_failed_recovery() {
        let recovered =
            option_update_failure(anyhow!("setter rejected: invalid\\x00response"), Ok(()))
                .to_string();
        assert!(recovered.contains("setter rejected: invalid\\x00response"));
        assert!(recovered.contains("previous accepted tinySA controls were restored"));
        assert!(recovered.contains("acquisition was stopped"));

        let failed = option_update_failure(
            anyhow!("setter rejected: invalid response"),
            Err(anyhow!("recovery command was rejected")),
        )
        .to_string();
        assert!(failed.contains("setter rejected: invalid response"));
        assert!(failed.contains("failed to restore tinySA controls"));
        assert!(failed.contains("recovery command was rejected"));
        assert!(failed.contains("acquisition was stopped"));
    }

    #[test]
    fn high_recovery_restores_the_coarse_attenuation_choice() {
        let settings = TinySaSettings {
            high_attenuation: true,
            ..TinySaSettings::default()
        };
        let (options, commands) =
            startup_options(Model::Basic, BasicInput::High, &settings).unwrap();
        assert!(commands.iter().any(|command| command == "attenuate 1"));
        assert_eq!(
            restore_option_commands(Model::Basic, &options).unwrap(),
            [
                "rbw auto",
                "attenuate 1",
                "attenuate 0",
                "attenuate 1",
                "spur on",
                "ext_gain 0",
            ]
        );
    }

    #[test]
    fn basic_persistence_preserves_ultra_settings() {
        let loaded = TinySaSettings {
            lna: true,
            ..TinySaSettings::default()
        };
        let options = option_definitions(Model::Basic, BasicInput::Low);
        let saved = persisted_settings(
            &loaded,
            &options,
            Model::Basic,
            BasicInput::Low,
            &HashSet::new(),
        )
        .unwrap();

        assert!(saved.lna);
    }

    #[test]
    fn startup_errors_name_the_config_key_value_and_allowed_values() {
        let points = validate_settings_shape(&TinySaSettings {
            points: 451,
            ..TinySaSettings::default()
        })
        .unwrap_err()
        .to_string();
        assert!(points.contains("[tinysa].points"));
        assert!(points.contains("451"));
        assert!(points.contains("64, 128, 290, 450, 900, 1800"));

        let rbw = startup_options(
            Model::Basic,
            BasicInput::Low,
            &TinySaSettings {
                rbw: "2".into(),
                ..TinySaSettings::default()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(rbw.contains("[tinysa].rbw"));
        assert!(rbw.contains("\"2\""));
        assert!(rbw.contains("auto, 3, 10, 30, 100, 300, 600"));
    }

    #[test]
    fn persistence_rejects_malformed_option_state() {
        for id in ["points", "ext_gain"] {
            let mut options = option_definitions(Model::Zs407, BasicInput::Low);
            options
                .iter_mut()
                .find(|option| option.id == id)
                .unwrap()
                .selected_choice = "invalid".into();

            assert!(persisted_settings(
                &TinySaSettings::default(),
                &options,
                Model::Zs407,
                BasicInput::Low,
                &HashSet::new(),
            )
            .is_err());
        }
    }

    #[test]
    fn rejected_set_option_commands_receive_one_reply() {
        let (reply, replies) = bounded(1);
        reject_command(
            Command::SetOption {
                id: "rbw".into(),
                choice: "30".into(),
                reply,
            },
            anyhow!("injected abort failure"),
        );
        assert!(replies.recv().unwrap().is_err());
        assert!(matches!(
            replies.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn centered_windows_keep_the_requested_span_inside_the_model_range() {
        let low_edge = centered_window(100_000, 10_000_000, 100_000, 960_000_000);
        assert_eq!(low_edge, (100_000, 10_100_000));
        assert_eq!(low_edge.0 + (low_edge.1 - low_edge.0) / 2, 5_100_000);
        assert_eq!(
            centered_window(960_000_000, 10_000_000, 100_000, 960_000_000),
            (950_000_000, 960_000_000)
        );
    }

    #[test]
    fn a_span_too_narrow_for_the_point_count_is_clamped() {
        assert_eq!(
            normalize_span(1.0, 959_900_000, DEFAULT_POINTS).unwrap(),
            DEFAULT_POINTS as u64
        );
        assert_eq!(normalize_span(899.0, 959_900_000, 900).unwrap(), 900);
        assert_eq!(normalize_span(900.0, 959_900_000, 900).unwrap(), 900);
        let frequencies =
            protocol::scan_frequencies(100_000, 100_000 + DEFAULT_POINTS as u64, DEFAULT_POINTS);
        assert!(frequencies.windows(2).all(|pair| pair[1] > pair[0]));
    }

    #[test]
    fn firmware_frequencies_use_an_endpoint_preserving_display_grid() {
        let measured = protocol::scan_frequencies(100_000_000, 200_000_000, 450);
        assert!(
            measured.windows(2).map(|pair| pair[1] - pair[0]).min()
                != measured.windows(2).map(|pair| pair[1] - pair[0]).max()
        );
        let displayed = display_frequencies(&measured).unwrap();
        assert_eq!(displayed.first(), measured.first());
        assert_eq!(displayed.last(), measured.last());
        let span = displayed.last().unwrap() - displayed.first().unwrap();
        let intervals = displayed.len() as u64 - 1;
        let lower_step = span / intervals;
        let upper_step = span.div_ceil(intervals);
        assert!(displayed.windows(2).all(|pair| {
            let step = pair[1] - pair[0];
            step == lower_step || step == upper_step
        }));
    }

    #[test]
    fn display_grid_rejects_duplicate_firmware_frequencies() {
        assert!(display_frequencies(&[100_000, 100_000, 100_001]).is_err());
        assert!(display_frequencies(&[100_002, 100_001, 100_000]).is_err());
    }

    #[test]
    fn native_sweeps_keep_raw_firmware_frequencies() {
        let measured = protocol::scan_frequencies(100_000_000, 200_000_000, 450);
        let sweep =
            trace_frequencies(measured.clone(), PowerTraceTarget::Sweep).expect("sweep axis");
        let spectrum =
            trace_frequencies(measured.clone(), PowerTraceTarget::Spectrum).expect("spectrum axis");

        assert_eq!(sweep, measured);
        assert_ne!(spectrum, measured);
        for target in [PowerTraceTarget::Spectrum, PowerTraceTarget::Sweep] {
            assert!(trace_frequencies(vec![100_000, 100_000, 100_001], target).is_err());
            assert!(trace_frequencies(vec![100_002, 100_001, 100_000], target).is_err());
        }
    }

    #[test]
    fn an_edge_shift_becomes_the_next_scan_center() {
        let (start, stop) = centered_window(100_000, 10_000_000, 100_000, 960_000_000);
        let effective_center = window_center(start, stop);
        assert_eq!(effective_center, 5_100_000);
        assert_eq!(
            centered_window(effective_center, 1_000_000, 100_000, 960_000_000),
            (4_600_000, 5_600_000)
        );
    }

    #[test]
    fn completed_windows_update_only_normal_tuning() {
        let (start_hz, stop_hz) =
            centered_window(100_000, 10_000_000, MIN_FREQUENCY_HZ, 960_000_000);
        let mut center_hz = 100_000;
        let mut span_hz = 10_000_000;
        let (effective_center_hz, effective_span_hz) = effective_window(start_hz, stop_hz);
        update_normal_window(
            None,
            &mut center_hz,
            &mut span_hz,
            effective_center_hz,
            effective_span_hz,
        );
        assert_eq!(center_hz, 5_100_000);
        assert_eq!(span_hz, 10_000_000);

        update_normal_window(
            Some(DirectSweepConfig {
                start_hz: 88_000_000,
                stop_hz: 108_000_000,
                generation: 1,
            }),
            &mut center_hz,
            &mut span_hz,
            98_000_000,
            20_000_000,
        );
        assert_eq!(center_hz, 5_100_000);
        assert_eq!(span_hz, 10_000_000);
    }

    #[test]
    fn direct_sweeps_accept_only_ordered_in_range_limits() {
        let valid = DirectSweepConfig {
            start_hz: 88_000_000,
            stop_hz: 108_000_000,
            generation: 7,
        };
        assert!(validate_direct_sweep(Some(valid), Model::Basic, BasicInput::Low, 450).is_ok());
        assert!(validate_direct_sweep(None, Model::Basic, BasicInput::Low, 450).is_ok());
        assert!(validate_direct_sweep(
            Some(DirectSweepConfig {
                start_hz: valid.stop_hz,
                stop_hz: valid.start_hz,
                ..valid
            }),
            Model::Basic,
            BasicInput::Low,
            450,
        )
        .is_err());
        assert!(validate_direct_sweep(
            Some(DirectSweepConfig {
                stop_hz: BASIC_LOW_MAX_HZ + 1,
                ..valid
            }),
            Model::Basic,
            BasicInput::Low,
            450,
        )
        .is_err());
        assert!(validate_direct_sweep(
            Some(DirectSweepConfig {
                start_hz: BASIC_HIGH_MIN_HZ,
                stop_hz: BASIC_HIGH_MAX_HZ,
                ..valid
            }),
            Model::Basic,
            BasicInput::High,
            450,
        )
        .is_ok());
        assert!(validate_direct_sweep(
            Some(DirectSweepConfig {
                start_hz: MIN_FREQUENCY_HZ,
                stop_hz: BASIC_HIGH_MAX_HZ,
                ..valid
            }),
            Model::Basic,
            BasicInput::High,
            450,
        )
        .is_err());
        assert!(validate_direct_sweep(
            Some(DirectSweepConfig {
                start_hz: MIN_FREQUENCY_HZ,
                stop_hz: BASIC_HIGH_MAX_HZ,
                ..valid
            }),
            Model::Basic,
            BasicInput::Low,
            450,
        )
        .is_err());
        let exact = DirectSweepConfig {
            start_hz: 100_000,
            stop_hz: 100_450,
            generation: 1,
        };
        assert!(validate_direct_sweep(Some(exact), Model::Basic, BasicInput::Low, 450).is_ok());
        assert!(validate_direct_sweep(Some(exact), Model::Basic, BasicInput::Low, 900).is_err());
    }

    #[test]
    fn point_updates_fit_both_normal_and_direct_spans() {
        let (options, _) =
            startup_options(Model::Basic, BasicInput::Low, &TinySaSettings::default()).unwrap();
        assert_eq!(
            capabilities(Model::Basic, BasicInput::Low).sample_rate_min_hz,
            64.0
        );
        assert!(prepare_option_update(&options, Model::Basic, "points", "900", 900, None).is_ok());
        assert!(prepare_option_update(&options, Model::Basic, "points", "900", 899, None).is_err());
        let direct = Some(DirectSweepConfig {
            start_hz: 100_000,
            stop_hz: 100_899,
            generation: 1,
        });
        assert!(
            prepare_option_update(&options, Model::Basic, "points", "900", 10_000, direct).is_err()
        );
        assert!(
            prepare_option_update(&options, Model::Basic, "points", "450", 450, direct).is_ok()
        );
    }

    #[test]
    fn unknown_ultra_uses_the_conservative_zs405_range() {
        assert_eq!(Model::UltraUnknown.maximum_hz(), 6_000_000_000);
    }

    #[test]
    fn startup_forces_every_model_into_input_mode() {
        assert_eq!(
            input_mode_command(Model::Basic, BasicInput::Low),
            "mode low input"
        );
        assert_eq!(
            input_mode_command(Model::Basic, BasicInput::High),
            "mode high input"
        );
        for model in [
            Model::Zs405,
            Model::Zs406,
            Model::Zs407,
            Model::UltraUnknown,
        ] {
            assert_eq!(input_mode_command(model, BasicInput::High), "mode input");
        }
    }

    #[test]
    fn selector_keeps_the_path_and_basic_input_separate() {
        assert_eq!(
            parse_selector("/dev/ttyACM2").unwrap(),
            (Some(PathBuf::from("/dev/ttyACM2")), None)
        );
        assert_eq!(
            parse_selector("/dev/ttyACM2?input=high").unwrap(),
            (Some(PathBuf::from("/dev/ttyACM2")), Some(BasicInput::High))
        );
        assert_eq!(
            parse_selector("?input=high").unwrap(),
            (None, Some(BasicInput::High))
        );
        assert!(parse_selector("/dev/ttyACM2?input=other").is_err());
    }

    #[test]
    fn basic_input_resolution_uses_config_until_the_cli_overrides_it() {
        assert_eq!(
            resolve_basic_input(None, BasicInput::High),
            BasicInput::High
        );
        assert_eq!(
            resolve_basic_input(Some(BasicInput::Low), BasicInput::High),
            BasicInput::Low
        );
        assert_eq!(
            resolve_basic_input(Some(BasicInput::High), BasicInput::Low),
            BasicInput::High
        );
    }

    #[test]
    fn ultra_ignores_basic_input_and_warns_only_for_an_explicit_selector() {
        let settings = TinySaSettings {
            basic_input: BasicInput::High,
            ..TinySaSettings::default()
        };
        let (options, _) = startup_options(Model::Zs407, settings.basic_input, &settings).unwrap();
        let saved = persisted_settings(
            &settings,
            &options,
            Model::Zs407,
            settings.basic_input,
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(saved.basic_input, BasicInput::High);
        assert_eq!(
            input_mode_command(Model::Zs407, BasicInput::High),
            "mode input"
        );
        assert!(ignored_explicit_basic_input_note(Model::Zs407, None).is_none());
        for input in [BasicInput::Low, BasicInput::High] {
            let note = ignored_explicit_basic_input_note(Model::Zs407, Some(input)).unwrap();
            assert!(note.contains(&format!(
                "explicit Basic {} input selection was ignored",
                input.label()
            )));
        }
        assert!(ignored_explicit_basic_input_note(Model::Basic, Some(BasicInput::High)).is_none());
    }

    #[test]
    fn only_an_explicit_selector_overrides_the_basic_input() {
        let bare = list(Some("/dev/ttyACM2"));
        assert_eq!(bare.len(), 1);
        assert_eq!(bare[0].tiny_sa_input, None);

        let high = list(Some("/dev/ttyACM2?input=high"));
        assert_eq!(high.len(), 1);
        assert_eq!(high[0].tiny_sa_input, Some(BasicInput::High));
    }

    #[test]
    fn basic_capabilities_match_the_selected_input() {
        let low = capabilities(Model::Basic, BasicInput::Low);
        assert_eq!((low.freq_min_hz, low.freq_max_hz), (100_000, 350_000_000));
        let high = capabilities(Model::Basic, BasicInput::High);
        assert_eq!(
            (high.freq_min_hz, high.freq_max_hz),
            (240_000_000, 959_000_000)
        );
    }

    #[test]
    fn narrow_rbw_expands_the_scan_deadline() {
        let settings = TinySaSettings {
            rbw: "0.2".into(),
            ..TinySaSettings::default()
        };
        let (options, _) = startup_options(Model::Zs405, BasicInput::Low, &settings).unwrap();
        let segment = Segment {
            start_hz: 400_000_000,
            stop_hz: 500_000_000,
            points: 450,
        };
        assert_eq!(current_rbw_hz(&options), Some(200));
        let timeout = scan_inactivity_timeout(segment, Model::Zs405, &options).unwrap();
        assert!(timeout > Duration::from_secs(120), "{timeout:?}");
        let (automatic, _) =
            startup_options(Model::Zs405, BasicInput::Low, &TinySaSettings::default()).unwrap();
        assert_eq!(current_rbw_hz(&automatic), None);
    }

    #[test]
    fn explicit_rbw_updates_the_next_trace_metadata_and_timeout() {
        let (mut options, _) =
            startup_options(Model::Zs405, BasicInput::Low, &TinySaSettings::default()).unwrap();
        let segment = Segment {
            start_hz: 400_000_000,
            stop_hz: 500_000_000,
            points: 450,
        };
        let automatic_timeout = scan_inactivity_timeout(segment, Model::Zs405, &options).unwrap();
        let prepared =
            prepare_option_update(&options, Model::Zs405, "rbw", "0.2", 10_000, None).unwrap();
        execute_option_update(&mut options, prepared, "rbw", "0.2", |_| Ok(())).unwrap();
        assert_eq!(current_rbw_hz(&options), Some(200));
        assert!(
            scan_inactivity_timeout(segment, Model::Zs405, &options).unwrap() > automatic_timeout
        );
    }

    #[cfg(test)]
    mod hardware_tests {
        use super::*;

        #[test]
        #[ignore = "requires SDRTOP_TINYSA_TEST to name a connected serial port"]
        fn connected_device_streams_spectrum_frames() {
            let path = std::env::var("SDRTOP_TINYSA_TEST").expect("SDRTOP_TINYSA_TEST is not set");
            let device = TinySaDevice::open(
                Path::new(&path),
                BasicInput::Low,
                None,
                &TinySaSettings::default(),
            )
            .unwrap();
            assert!(device
                .info()
                .board_name
                .to_ascii_lowercase()
                .contains("tinysa"));

            let state = Arc::new(Mutex::new(crate::state::SdrMetrics::fixture()));
            let (sample_tx, _) = crossbeam_channel::bounded(1);
            let (demod_tx, _) = crossbeam_channel::bounded(1);
            let (net_tx, _) = crossbeam_channel::bounded(1);
            let (power_tx, power_rx) = crossbeam_channel::bounded(4);
            let context = Arc::new(RxContext {
                metrics: Arc::clone(&state),
                sample_tx,
                fft_feed: crate::hardware::FeedHealth::default(),
                demod_tx,
                net_tx,
                net_feed: crate::hardware::FeedHealth::default(),
                power_tx,
                geometry: device.capabilities().sample_geometry,
                blocks_seen: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            });

            device.start_rx(context).unwrap();
            let spectrum = power_rx.recv_timeout(Duration::from_secs(15)).unwrap();
            assert_eq!(spectrum.target, PowerTraceTarget::Spectrum);
            assert_eq!(spectrum.frequencies_hz.len(), spectrum.levels_dbm.len());
            assert!(!spectrum.frequencies_hz.is_empty());
            assert!(device.options().iter().any(|option| option.id == "rbw"));
            device.set_option("points", "64").unwrap();
            device.stop_rx().unwrap();
            assert!(!device.is_streaming());
        }
    }
}
