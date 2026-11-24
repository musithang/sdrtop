// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What the engine's tests and its clients' tests share: a radio that is
//! not there.

use std::sync::{Arc, Mutex};

use crate::hardware::{DeviceCapabilities, DeviceInfo, RateSet, RxContext, SdrDevice};

/// A radio for the tests: a HackRF's capabilities, every tuning written
/// down, and a gain setter that can be told to refuse.
pub struct Bench {
    pub caps: DeviceCapabilities,
    pub tuned: Mutex<Vec<u64>>,
    pub refuse_gain: bool,
    /// Every option set, in order; `options` answers as if each was taken.
    pub options_set: Mutex<Vec<(String, String)>>,
    /// Streaming from `start_rx` to `stop_rx`, as a radio is: a stream that
    /// never says it runs reads as one that stopped by itself.
    pub streaming: std::sync::atomic::AtomicBool,
}

impl Bench {
    #[expect(
        clippy::new_without_default,
        reason = "a test radio is made on purpose, never by a `..Default::default()`"
    )]
    pub fn new() -> Self {
        Self {
            caps: crate::hardware::native::hackrf::caps(),
            tuned: Mutex::new(Vec::new()),
            refuse_gain: false,
            options_set: Mutex::new(Vec::new()),
            streaming: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl SdrDevice for Bench {
    fn capabilities(&self) -> &DeviceCapabilities {
        &self.caps
    }
    fn info(&self) -> DeviceInfo {
        DeviceInfo::default()
    }
    fn start_rx(&self, _ctx: Arc<RxContext>) -> anyhow::Result<()> {
        self.streaming
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn stop_rx(&self) -> anyhow::Result<()> {
        self.streaming
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn is_streaming(&self) -> bool {
        self.streaming.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn set_frequency(&self, hz: u64) -> anyhow::Result<u64> {
        self.tuned.lock().unwrap().push(hz);
        Ok(hz)
    }
    fn set_sample_rate(&self, hz: f64) -> anyhow::Result<RateSet> {
        Ok(RateSet::new(hz, None, 0))
    }
    fn set_lna_gain(&self, _db: u32) -> anyhow::Result<()> {
        Ok(())
    }
    fn set_option(&self, id: &str, choice: &str) -> anyhow::Result<()> {
        self.options_set
            .lock()
            .unwrap()
            .push((id.to_string(), choice.to_string()));
        Ok(())
    }
    fn options(&self) -> Vec<crate::hardware::DeviceOption> {
        let chosen = self.options_set.lock().unwrap().last().cloned();
        vec![crate::hardware::DeviceOption {
            id: "bias".into(),
            label: "Bias tee".into(),
            choices: vec!["Off".into(), "On".into()],
            selected_choice: chosen.map_or("Off".into(), |(_, c)| c),
            integer_range: None,
        }]
    }
    fn set_stage_gain(&self, _index: usize, _name: &str, _db: f64) -> anyhow::Result<()> {
        if self.refuse_gain {
            anyhow::bail!("the radio said no")
        }
        Ok(())
    }
}

/// An engine on a [`Bench`] radio, with the fixture's state.
pub fn engine_on(bench: Bench) -> (super::Engine, Arc<Bench>) {
    let device = Arc::new(bench);
    let state = Arc::new(Mutex::new(crate::state::SdrMetrics::fixture()));
    let engine = super::Engine::new(state, Arc::clone(&device) as Arc<dyn SdrDevice>);
    (engine, device)
}
