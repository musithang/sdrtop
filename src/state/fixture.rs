//! A metrics snapshot to render panels against.
//!
//! Nothing in the crate constructed an [`SdrMetrics`] outside `app/builder`
//! until this module existed, which is why no panel body had ever been unit
//! tested: every `render` takes one, and building one by hand is forty lines of
//! sub-state literals. So the panels were checked by driving the real binary in
//! tmux against a real radio — slow, needs hardware, and the step that produced
//! two vacuous passes during R4.
//!
//! What this gives instead: [`SdrMetrics::fixture`] for an idle radio, a handful
//! of chainable methods to make it interesting, and [`draw`] to render any panel
//! into a fixed-size buffer and read the result back as text. A split can then be
//! proved to change nothing on screen by comparing two strings.
//!
//! **Deliberately a real struct literal, not `Default`.** `SdrMetrics` has no
//! `Default` and should not get one: a snapshot with no `caps` is not a state the
//! app can ever be in, and the compiler forcing this file to name every new field
//! is the point — a field added without a decision about what it means at rest
//! would otherwise silently arrive as zero in every test.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use super::*;

impl SdrMetrics {
    /// An idle radio: a HackRF open at 100 MHz, RX not started, no FFT frame yet.
    ///
    /// This is what a panel sees a fraction of a second after launch, which is
    /// also the state most likely to be got wrong — every "waiting for RX" and
    /// `[STALE]` path runs through it.
    pub(crate) fn fixture() -> Self {
        SdrMetrics {
            radio: RadioState {
                frequency: 100_000_000,
                config_sample_rate: 10_000_000.0,
                actual_sample_rate: 0,
                bb_filter_hz: 5_000_000,
                lna_gain: 24,
                vga_gain: 30,
                amp_enabled: false,
                rx_enabled: false,
                hw_streaming: false,
                rx_start_time: None,
                bytes_since_last_poll: 0,
                last_poll_time: Instant::now(),
                current_throughput_bps: 0,
                throughput_history: VecDeque::new(),
                sample_rate_history: VecDeque::new(),
            },
            signal: SignalState::default(),
            iq: IqState {
                iq_imbalance_db: 0.0,
                dc_offset_i: 0.0,
                dc_offset_q: 0.0,
                cb_period_us: 0,
                cb_jitter_us: 0,
                jitter_history: VecDeque::new(),
                iq_amplitude_hist: [0u64; 32],
                adc_signed_hist: [0u64; 32],
                buf_fill_pct: 0.0,
                buf_fill_history: VecDeque::new(),
                phase_imbalance_deg: 0.0,
                cal: IqCalState::default(),
                irr_history: VecDeque::new(),
                constellation: VecDeque::new(),
            },
            observer: ObserverState::default(),
            spectrum: SpectrumState {
                step_hz: 100_000,
                y_min: -120.0,
                y_max: 0.0,
                hold: None,
                cursor_freq: None,
                markers: vec![],
                pending_marker: None,
                style: SpectrumStyle::default(),
            },
            waterfall: WaterfallState::new(512, crate::palette::WaterfallPalette::default()),
            system: SystemState {
                board_name: Arc::from("HackRF One"),
                serial: Arc::from("0000000000000000"),
                fw_version: Arc::from("2024.02.1"),
                board_rev: 0x02,
                usb_api_version: 0x0107,
                process_cpu_pct: 0.0,
                process_rss_mb: 0,
                cpu_history: VecDeque::new(),
            },
            timing: TimingState::default(),
            sweep: SweepState::default(),
            ui: UiState::default(),
            lab: LabState::default(),
            demod: DemodState::default(),
            caps: Arc::new(crate::hardware::hackrf::caps()),
            acc: Accumulators::default(),
        }
    }

    /// RX running, with plausible throughput and a comfortable ADC level.
    ///
    /// The peak sits inside [`ADC_COMFORT_DBFS`], so a panel that judges gain
    /// staging reads "optimal" from this and a test does not have to know the
    /// thresholds to get an uninteresting answer.
    pub(crate) fn streaming(mut self) -> Self {
        self.radio.rx_enabled = true;
        self.radio.hw_streaming = true;
        self.radio.rx_start_time = Some(Instant::now());
        self.radio.actual_sample_rate = 10_000_000;
        self.radio.current_throughput_bps = 20_000_000;
        self.signal.adc_peak_dbfs = -8.0;
        self.signal.adc_rms_dbfs = -20.0;
        self
    }

    /// A fresh FFT frame with a carrier `offset_hz` from centre, standing
    /// `snr_db` above the noise floor.
    ///
    /// `Instant::now()` on purpose: a frame is only as good as its age, and every
    /// `Staleness::FftAge` panel reads exactly that.
    pub(crate) fn with_carrier(mut self, offset_hz: f64, snr_db: f32) -> Self {
        const N: usize = 256;
        let noise_floor = -100.0_f32;
        let sample_rate = self.radio.config_sample_rate;
        let mut bins = vec![noise_floor; N];
        let bin = (N as f64 / 2.0 + offset_hz / (sample_rate / N as f64)).round();
        if (0.0..N as f64).contains(&bin) {
            bins[bin as usize] = noise_floor + snr_db;
        }
        let bins = Arc::new(bins);
        self.waterfall.last_fft = Some(FftFrame {
            bins_dbfs: Arc::clone(&bins),
            peak_hold: Arc::clone(&bins),
            noise_floor,
            center_freq_hz: self.radio.frequency,
            sample_rate,
            timestamp: Instant::now(),
            peak_to_nf_db: snr_db,
            channel_power_dbfs: noise_floor + snr_db,
            occupied_bw_hz: 150_000,
            enbw_hz: sample_rate / N as f64,
        });
        self.signal.peak_to_nf_db = snr_db;
        self.signal.channel_power_dbfs = noise_floor + snr_db;
        self.signal.occupied_bw_hz = 150_000;
        self.waterfall
            .buffer
            .rows
            .push_front((Instant::now(), bins));
        self
    }

    /// Age the newest FFT frame past [`crate::ui::panel::FFT_STALE_MS`], so the
    /// staleness paths can be rendered without a test sleeping.
    pub(crate) fn with_stale_fft(mut self) -> Self {
        if let Some(fr) = self.waterfall.last_fft.as_mut() {
            fr.timestamp = Instant::now()
                - std::time::Duration::from_millis(crate::ui::panel::FFT_STALE_MS as u64 + 50);
        }
        self
    }
}

impl SdrMetrics {
    /// A completed sweep across `start_hz..stop_hz`, with a bump partway along so
    /// the plot has a shape rather than a flat line.
    pub(crate) fn with_sweep(mut self, start_hz: u64, stop_hz: u64) -> Self {
        const N: usize = 64;
        let span = (stop_hz - start_hz) as f64;
        let freq_hz: Vec<u64> = (0..N)
            .map(|i| start_hz + (span * i as f64 / N as f64) as u64)
            .collect();
        let peak_dbfs: Vec<f32> = (0..N)
            .map(|i| if i == N / 3 { -30.0 } else { -95.0 })
            .collect();
        let mean_dbfs: Vec<f32> = peak_dbfs.iter().map(|v| v - 8.0).collect();
        self.sweep.config = SweepConfig {
            start_hz,
            stop_hz,
            step_hz: (span / N as f64) as u64,
            dwell_ms: 20,
        };
        self.sweep.active = true;
        self.sweep.current_hz = start_hz;
        self.sweep.positions_total = N;
        self.sweep.positions_done = N;
        self.sweep.cycle_count = 3;
        self.sweep.cycle_duration_ms = 1_400;
        self.sweep.current_frame = Some(Arc::new(SweepFrame {
            start_hz,
            stop_hz,
            freq_hz,
            peak_dbfs,
            mean_dbfs,
            timestamp: Instant::now(),
            cycle_count: 3,
            cycle_duration_ms: 1_400,
        }));
        self
    }
}

impl SdrMetrics {
    /// A plausible timing snapshot: callbacks arriving on time, a budget derived
    /// from the expected period, and a deviation series to plot.
    ///
    /// `jitter_ratio` scales the deviations against the deadline budget, so a
    /// test can ask for "comfortably inside" (0.3) or "well over" (2.5) without
    /// knowing the thresholds.
    pub(crate) fn with_timing(mut self, jitter_ratio: f64) -> Self {
        let expected = 4_096u64;
        let budget = (expected as f64 * 0.15).round() as u64;
        let peak = (budget as f64 * jitter_ratio).round() as i32;
        let deviations: Vec<i32> = (0..320)
            .map(|i| {
                let phase = (i % 16) as f64 / 16.0 * std::f64::consts::TAU;
                (peak as f64 * phase.sin()).round() as i32
            })
            .collect();
        let abs: Vec<u64> = deviations.iter().map(|d| d.unsigned_abs() as u64).collect();
        let mut sorted = abs.clone();
        sorted.sort_unstable();
        let pick = |q: f64| sorted[((sorted.len() as f64 * q) as usize).min(sorted.len() - 1)];
        self.timing = TimingState {
            cb_period_us: expected,
            cb_period_expected: expected,
            cb_period_delta_ppm: 12,
            cb_jitter_us: (peak as u64) / 3,
            jitter_max_us: peak as u64,
            jitter_session_max_us: peak as u64,
            sr_delta_ppm: 18,
            throughput_mean_mbps: 38.1,
            throughput_std_mbps: 0.4,
            timing_quality: TimingQuality::classify(pick(0.99), expected, 18, 0),
            late_callbacks: abs.iter().filter(|&&d| d > budget).count() as u32,
            late_window: abs.len() as u32,
            dev_p95_us: pick(0.95),
            dev_p99_us: pick(0.99),
            dev_peak_us: sorted[sorted.len() - 1],
            cb_deviations_us: deviations,
            deadline_budget_us: budget,
        };
        self.iq.cb_period_us = expected;
        self.iq.cb_jitter_us = self.timing.cb_jitter_us;
        self
    }
}

/// Render one panel into a `width × height` buffer and return it as text lines,
/// trailing spaces trimmed.
///
/// Goes through [`PanelRegistry::render_panel`] rather than calling
/// `Panel::render` directly, so the frame, the nameplate and the `[STALE]` tag
/// are part of what is compared — those are as much the panel's output as its
/// body is, and R3 moved them out of the panel precisely so they could not drift.
pub(crate) fn draw(
    panel: impl crate::ui::panel::Panel + 'static,
    width: u16,
    height: u16,
    state: &SdrMetrics,
) -> Vec<String> {
    let name = panel.name();
    let mut registry = crate::ui::PanelRegistry::new();
    registry.register(panel);
    let theme = crate::Theme::sdr();

    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).expect("test backend never fails");
    terminal
        .draw(|f| {
            let area = f.size();
            registry.render_panel(name, f, area, state, &theme, false);
        })
        .expect("test backend never fails");

    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer.get(x, y).symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture has to be a state the app can actually be in, or every test
    /// written against it is testing a fiction.
    #[test]
    fn the_idle_fixture_is_a_coherent_idle_radio() {
        let m = SdrMetrics::fixture();
        assert!(!m.radio.hw_streaming);
        assert!(m.radio.rx_start_time.is_none());
        assert_eq!(
            m.radio.actual_sample_rate, 0,
            "nothing has been measured yet"
        );
        assert!(m.waterfall.last_fft.is_none());
        // The tuning is legal for the capabilities the fixture claims.
        assert!((m.caps.freq_min_hz..=m.caps.freq_max_hz).contains(&m.radio.frequency));
        let (lna, vga) = m.caps.gain.clamp_gains(m.radio.lna_gain, m.radio.vga_gain);
        assert_eq!(
            (lna, vga),
            (m.radio.lna_gain, m.radio.vga_gain),
            "the fixture's gains must be ones this device can be set to"
        );
    }

    #[test]
    fn a_carrier_lands_where_it_was_asked_for() {
        let m = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        let fr = m.waterfall.last_fft.as_ref().unwrap();
        let n = fr.bins_dbfs.len();
        let peak = fr
            .bins_dbfs
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();
        assert_eq!(peak, n / 2, "a zero offset is the centre bin");
        assert!((fr.bins_dbfs[peak] - fr.noise_floor - 40.0).abs() < 1e-4);

        // And an offset one moves off centre, in the right direction.
        let m = SdrMetrics::fixture()
            .streaming()
            .with_carrier(1_000_000.0, 40.0);
        let fr = m.waterfall.last_fft.as_ref().unwrap();
        let peak = fr
            .bins_dbfs
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();
        assert!(
            peak > fr.bins_dbfs.len() / 2,
            "a positive offset sits right of centre"
        );
    }

    /// `with_stale_fft` has to actually reach the rule the panels use, not merely
    /// set a number that looks old.
    #[test]
    fn with_stale_fft_reaches_the_declared_staleness_rule() {
        use crate::ui::panel::Staleness;
        let fresh = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        assert!(!Staleness::FftAge.resolve(&fresh));
        assert!(Staleness::FftAge.resolve(&fresh.with_stale_fft()));
    }

    /// The harness must return something before any test believes an empty diff.
    /// (R4's verification recipe learned this the hard way, twice.)
    #[test]
    fn draw_returns_a_frame_with_the_panel_in_it() {
        let lines = draw(
            crate::ui::SignalMetricsPanel,
            60,
            10,
            &SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0),
        );
        assert_eq!(lines.len(), 10, "one string per row");
        let joined = lines.join("\n");
        assert!(
            joined.contains("Signal") || joined.contains("SIGNAL"),
            "the nameplate should be in the buffer:\n{joined}"
        );
        assert!(
            joined.chars().any(|c| c.is_ascii_digit()),
            "a rendered panel with a carrier should show some number:\n{joined}"
        );
    }

    /// The point of the harness: the same state renders the same buffer, so a
    /// difference in a split is a real difference and not frame-to-frame noise.
    #[test]
    fn the_same_state_renders_identically_twice() {
        let m = SdrMetrics::fixture().streaming().with_carrier(0.0, 40.0);
        assert_eq!(
            draw(crate::ui::SignalMetricsPanel, 60, 10, &m),
            draw(crate::ui::SignalMetricsPanel, 60, 10, &m)
        );
    }
}
