// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Control: what can be asked of the radio, and the answer to each ask.
//!
//! A command is an absolute setting (this frequency, this stage at this
//! gain), never a keypress. How a key gets from where the radio is to where
//! it should go (one step along a table, one place along a vendor curve, a
//! total spread across the chain) is the key's business and stays with it;
//! what reaches the engine is the result, the same thing a script would
//! send. So there is one way the radio is changed, and one answer to it.
//!
//! **Two halves, split on what they may touch.** [`plan`] decides, on plain
//! data and with no device: a stage the radio does not have, a frequency it
//! does not tune, a rate outside its range is refused with the reason
//! (POLICY rule 2), and a gain off a stage's grid is moved onto it and the
//! move is said. [`Engine::apply`] then makes the one device call with no
//! lock held, writes back what the radio answered, and replies.

use serde::{Deserialize, Serialize};

use super::Engine;
use crate::state::SdrMetrics;

/// What can be asked of the radio. See the module comment.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    /// Tune to this frequency.
    Tune { hz: u64 },
    /// Run at this sample rate (on a swept analyser, this span).
    SampleRate { hz: f64 },
    /// Set one gain stage, by the name the radio gives it.
    Gain { stage: String, db: f64 },
    /// Set every gain stage at once, front to back.
    Gains { values: Vec<f64> },
    /// Switch the tuner's own AGC.
    Agc { on: bool },
    /// Switch the front-end boost: an amplifier, or the tuner's automatic
    /// gain, whichever the radio's boost is.
    Boost { on: bool },
    /// Back to the radio's own defaults: rate, gains, frequency, boost off.
    Reset,
    /// Compute what this view in this section asks for, by the terminal's
    /// own names for both.
    SetMode { section: String, view: String },
    /// Stream from the radio, or stop.
    Stream { on: bool },
    /// Take the DC offset out of the samples the spectrum, the demodulator
    /// and the constellation see; the statistics stay on the raw samples.
    DcBlock { on: bool },
    /// Set one of the radio's own options, by its id, to one of its choices.
    DeviceOption { id: String, choice: String },
    /// Show addresses `full`, by registrant (`oui`) or `masked`, on every
    /// screen and in every export and API answer alike.
    Addresses { show: String },
    /// The pager's view: text `masked` to its kind and length, and the one
    /// address it answers to (`mine`, none for every address).
    Pager { masked: bool, mine: Option<u32> },
}

/// The highest pager address: an address is 21 bits.
pub const PAGER_ADDRESS_MAX: u32 = 0x1F_FFFF;

/// The answer to a command.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    /// Done; what the radio is now set to.
    Applied { detail: String },
    /// Not done, and why. Nothing was changed.
    Refused { reason: String },
}

/// A command in the form the radio will be given, and what had to be moved
/// to get there.
#[derive(Clone, PartialEq, Debug)]
pub struct Planned {
    pub command: Command,
    /// Said whenever the command was changed on its way to the radio.
    pub note: Option<String>,
}

impl Planned {
    fn as_asked(command: &Command) -> Self {
        Self {
            command: command.clone(),
            note: None,
        }
    }
}

/// Decide a command against the state: refuse it with the reason, or give
/// the form the radio will be set to. Never sees the device.
pub fn plan(m: &SdrMetrics, command: &Command) -> Result<Planned, String> {
    let caps = &m.caps;
    match command {
        Command::Tune { hz } => {
            if caps.tunes(*hz) {
                Ok(Planned::as_asked(command))
            } else {
                Err(format!(
                    "{:.6} MHz is not a frequency this radio tunes ({})",
                    *hz as f64 / 1e6,
                    caps.bands_text()
                ))
            }
        }
        Command::SampleRate { hz } => {
            let (lo, hi) = (caps.sample_rate_min_hz, caps.sample_rate_max_hz);
            if hz.is_finite() && (lo..=hi).contains(hz) {
                Ok(Planned::as_asked(command))
            } else {
                Err(format!(
                    "{:.3} MHz is outside this radio's {:.3} to {:.3} MHz",
                    hz / 1e6,
                    lo / 1e6,
                    hi / 1e6
                ))
            }
        }
        Command::Gain { stage, db } => {
            let stages = caps.gain.stages();
            let Some(spec) = stages.iter().find(|s| s.name == *stage) else {
                return Err(format!(
                    "this radio has no stage called {stage}; its stages are {}",
                    stage_names(&stages)
                ));
            };
            let legal = spec.snap(*db);
            Ok(Planned {
                command: Command::Gain {
                    stage: stage.clone(),
                    db: legal,
                },
                note: (legal != *db)
                    .then(|| format!("{stage}: {db} dB is not on its grid, set to {legal} dB")),
            })
        }
        Command::Gains { values } => {
            let stages = caps.gain.stages();
            if values.len() != stages.len() {
                return Err(format!(
                    "{} values for {} gain stages ({})",
                    values.len(),
                    stages.len(),
                    stage_names(&stages)
                ));
            }
            let legal: Vec<f64> = stages
                .iter()
                .zip(values)
                .map(|(spec, &db)| spec.snap(db))
                .collect();
            let moved: Vec<String> = stages
                .iter()
                .zip(values.iter().zip(&legal))
                .filter(|(_, (asked, set))| asked != set)
                .map(|(spec, (asked, set))| format!("{}: {asked} dB set to {set} dB", spec.name))
                .collect();
            Ok(Planned {
                command: Command::Gains { values: legal },
                note: (!moved.is_empty()).then(|| format!("off the grid: {}", moved.join(", "))),
            })
        }
        Command::Boost { .. } if !caps.gain.has_boost() => {
            Err("this radio has no front end boost".to_string())
        }
        Command::DeviceOption { id, choice } => {
            let Some(option) = m.device_options.iter().find(|o| o.id == *id) else {
                let ids: Vec<&str> = m.device_options.iter().map(|o| o.id.as_str()).collect();
                return Err(if ids.is_empty() {
                    format!("this radio offers no options, so not {id}")
                } else {
                    format!(
                        "this radio has no option {id}; it offers {}",
                        ids.join(", ")
                    )
                });
            };
            if option.choices.contains(choice) {
                return Ok(Planned::as_asked(command));
            }
            match option.integer_choice(choice) {
                Ok(canonical) => Ok(Planned {
                    command: Command::DeviceOption {
                        id: id.clone(),
                        choice: canonical.to_string(),
                    },
                    note: None,
                }),
                Err(_) => Err(format!(
                    "{} cannot be {choice}; it can be {}",
                    option.label,
                    option.choices.join(", ")
                )),
            }
        }
        Command::Addresses { show } => {
            if crate::state::AddressDisplay::named(show).is_some() {
                Ok(Planned::as_asked(command))
            } else {
                let names: Vec<&str> = crate::state::AddressDisplay::ALL
                    .iter()
                    .map(|m| m.label())
                    .collect();
                Err(format!(
                    "there is no address mode called {show}; the modes are {}",
                    names.join(", ")
                ))
            }
        }
        Command::Pager {
            mine: Some(ric), ..
        } if *ric > PAGER_ADDRESS_MAX => Err(format!(
            "{ric} is not a pager address: an address is 21 bits, at most {PAGER_ADDRESS_MAX}"
        )),
        Command::Pager { .. } => Ok(Planned::as_asked(command)),
        Command::Agc { .. }
        | Command::Boost { .. }
        | Command::Reset
        | Command::SetMode { .. }
        | Command::Stream { .. }
        | Command::DcBlock { .. } => Ok(Planned::as_asked(command)),
    }
}

fn stage_names(stages: &[crate::hardware::StageSpec]) -> String {
    stages
        .iter()
        .map(|s| s.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

impl Engine {
    /// Decide, apply and answer one command. The device call is made with no
    /// lock held; the state is written only with what the radio answered.
    pub fn apply(&self, command: Command) -> Reply {
        let reply = self.decide_and_apply(&command);
        // Announced outside the state's lock: publishing never waits, but it
        // need not do its copying while every reader of the state waits.
        let events = std::sync::Arc::clone(&self.lock().events);
        events.publish(super::events::Event::Applied {
            command,
            reply: reply.clone(),
        });
        reply
    }

    fn decide_and_apply(&self, command: &Command) -> Reply {
        let planned = match plan(&self.lock(), command) {
            Ok(planned) => planned,
            Err(reason) => return Reply::Refused { reason },
        };
        let said = |detail: String| match &planned.note {
            Some(note) => format!("{detail} ({note})"),
            None => detail,
        };
        let device = &self.device();
        match planned.command.clone() {
            Command::Tune { hz } => match device.set_frequency(hz) {
                Ok(tuned) => {
                    self.lock().radio.frequency = tuned;
                    Reply::Applied {
                        detail: said(format!("tuned to {:.6} MHz", tuned as f64 / 1e6)),
                    }
                }
                Err(e) => refused(e),
            },
            Command::SampleRate { hz } => match device.set_sample_rate(hz) {
                Ok(set) => {
                    let mut m = self.lock();
                    m.radio.config_sample_rate = set.rate_hz;
                    m.radio.bb_filter_hz = set.bb_filter_hz;
                    Reply::Applied {
                        detail: said(format!("running at {:.6} MHz", set.rate_hz / 1e6)),
                    }
                }
                Err(e) => refused(e),
            },
            Command::Gain { stage, db } => {
                let (index, single) = {
                    let m = self.lock();
                    let index = m.caps.gain.stages().iter().position(|s| s.name == stage);
                    (index, m.caps.gain.is_single())
                };
                // Planned against the same capabilities, which never change
                // while a radio is open.
                let Some(index) = index else {
                    return Reply::Refused {
                        reason: format!("this radio has no stage called {stage}"),
                    };
                };
                match device.set_stage_gain(index, &stage, db) {
                    Ok(()) => {
                        let mut m = self.lock();
                        m.radio.set_stage_gain(index, db);
                        // On a single-tuner radio a manual gain switches the
                        // tuner's AGC off in hardware.
                        if single {
                            m.radio.amp_enabled = false;
                        }
                        m.lab.rf_autotrack = false;
                        Reply::Applied {
                            detail: said(format!("{stage} at {db} dB")),
                        }
                    }
                    Err(e) => refused(e),
                }
            }
            Command::Gains { values } => match device.set_stage_gains(&values) {
                Ok(()) => {
                    let mut m = self.lock();
                    m.radio.gains = values.clone();
                    m.lab.rf_autotrack = false;
                    Reply::Applied {
                        detail: said(format!("gains {values:?} dB")),
                    }
                }
                Err(e) => refused(e),
            },
            Command::Agc { on } => match device.set_tuner_agc(on) {
                Ok(()) => {
                    self.lock().radio.amp_enabled = on;
                    Reply::Applied {
                        detail: format!("tuner AGC {}", on_off(on)),
                    }
                }
                Err(e) => refused(e),
            },
            Command::Boost { on } => {
                let boost = self.lock().caps.gain.boost().cloned();
                // Asked of the mechanism, not the shape: a radio with one knob
                // and a discrete amplifier exists. A misrouted call would answer
                // `Ok` through the trait's default and switch nothing.
                let result = match boost {
                    Some(crate::hardware::Boost::GainMode) => device.set_tuner_agc(on),
                    Some(crate::hardware::Boost::Element(_)) => device.set_amp_enable(on),
                    None => Err(anyhow::anyhow!("this radio has no front end boost")),
                };
                match result {
                    Ok(()) => {
                        let mut m = self.lock();
                        m.radio.amp_enabled = on;
                        m.lab.rf_autotrack = false;
                        Reply::Applied {
                            detail: format!("boost {}", on_off(on)),
                        }
                    }
                    Err(e) => refused(e),
                }
            }
            Command::Reset => self.reset(),
            Command::Stream { on } => {
                // The rx task starts and stops the stream on its next poll.
                self.lock().radio.rx_enabled = on;
                Reply::Applied {
                    detail: format!("streaming {}", on_off(on)),
                }
            }
            Command::DcBlock { on } => {
                self.lock().iq.cal.dc_block_on = on;
                Reply::Applied {
                    detail: format!("DC block {}", on_off(on)),
                }
            }
            Command::DeviceOption { id, choice } => {
                let device = std::sync::Arc::clone(device);
                match super::option::set(
                    &id,
                    &choice,
                    |id, choice| device.set_option(id, choice),
                    || device.options(),
                ) {
                    Ok(options) => {
                        let now = options
                            .iter()
                            .find(|o| o.id == id)
                            .map_or(choice.clone(), |o| o.selected_choice.clone());
                        self.lock().device_options = std::sync::Arc::new(options);
                        let retune =
                            match super::option::retune_after(device.as_ref(), &id, &self.state) {
                                Some(Ok(tuned)) => {
                                    self.lock().radio.frequency = tuned;
                                    format!(", retuned to {:.6} MHz", tuned as f64 / 1e6)
                                }
                                Some(Err(why)) => format!(", retune refused: {why}"),
                                None => String::new(),
                            };
                        Reply::Applied {
                            detail: format!("{id} is {now}{retune}"),
                        }
                    }
                    Err(e) => refused(e),
                }
            }
            Command::Addresses { show } => {
                // Planned: the name is one of the modes.
                if let Some(mode) = crate::state::AddressDisplay::named(&show) {
                    self.lock().net.address_display = mode;
                }
                Reply::Applied {
                    detail: format!("addresses shown {show}"),
                }
            }
            Command::Pager { masked, mine } => {
                {
                    let mut m = self.lock();
                    m.pager.view.masked = masked;
                    m.pager.view.mine = mine;
                }
                Reply::Applied {
                    detail: format!(
                        "pager text {}, {}",
                        if masked { "masked" } else { "shown" },
                        mine.map_or("every address".to_string(), |r| format!("address {r} only"))
                    ),
                }
            }
            Command::SetMode { section, view } => {
                self.lock().mode = super::mode::Mode::for_view(&section, &view);
                Reply::Applied {
                    detail: format!("computing for {view}"),
                }
            }
        }
    }

    /// Back to the radio's own defaults. Every device call is made before the
    /// state is touched, and the state is written only if all of them
    /// succeeded: a half-applied reset would leave the readouts describing a
    /// radio that is not in that state.
    fn reset(&self) -> Reply {
        let caps = std::sync::Arc::clone(&self.lock().caps);
        let device = &self.device();
        let def_freq = caps.default_frequency_hz;
        let def_sr = caps.default_sample_rate_hz;
        // Every stage the radio has, where a first run would put it: one
        // whose knob starts on a vendor curve goes back onto it.
        let curve = caps.gain.default_curve();
        let gains = caps.gain.start_gains(curve, def_freq);

        // The rate is the radio's own default, so a driver has no reason to
        // round it; taking what it reports anyway keeps one account of where
        // the recorded rate comes from.
        let (sr_result, settled_sr, bb_bw) = match device.set_sample_rate(def_sr) {
            Ok(set) => (Ok(()), set.rate_hz, set.bb_filter_hz),
            Err(e) => (
                Err(e),
                def_sr,
                crate::hardware::native::hackrf::compute_bb_filter_bw(def_sr),
            ),
        };
        let mut results = vec![device.set_stage_gains(&gains)];
        // Where the radio tuned, which a receiver on a tuning grid may round.
        let mut tuned_freq = def_freq;
        results.extend([
            device.set_frequency(def_freq).map(|hz| tuned_freq = hz),
            sr_result,
            device.set_amp_enable(false),
        ]);

        let failures: Vec<String> = results
            .iter()
            .filter_map(|r| r.as_ref().err().map(|e| e.to_string()))
            .collect();
        if !failures.is_empty() {
            return Reply::Refused {
                reason: failures.join("; "),
            };
        }
        let mut m = self.lock();
        m.radio.gains = gains;
        m.radio.gain_curve = curve;
        m.radio.amp_enabled = false;
        m.lab.rf_autotrack = false;
        m.radio.frequency = tuned_freq;
        m.radio.config_sample_rate = settled_sr;
        m.radio.bb_filter_hz = bb_bw;
        Reply::Applied {
            detail: "settings reset to defaults".to_string(),
        }
    }
}

fn refused(e: anyhow::Error) -> Reply {
    Reply::Refused {
        reason: e.to_string(),
    }
}

fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testing::{engine_on, Bench};
    use crate::state::SdrMetrics;
    use std::sync::Arc;

    /// A value off the stage's grid lands on the nearest legal one, and the
    /// answer names both: a client that asked for one figure and got another
    /// is told, never left to find out from the readout.
    #[test]
    fn a_gain_by_name_lands_on_a_legal_value_and_says_so() {
        let m = SdrMetrics::fixture().rtlsdr();
        let stage = m.caps.gain.stages()[0].clone();
        let asked = (stage.table[3] + stage.table[4]) / 2.0 + 0.01;
        let legal = stage.snap(asked);
        assert_ne!(asked, legal, "the request must be off the grid");

        let planned = plan(
            &m,
            &Command::Gain {
                stage: stage.name.clone(),
                db: asked,
            },
        )
        .expect("a stage the radio has");
        assert_eq!(
            planned.command,
            Command::Gain {
                stage: stage.name.clone(),
                db: legal
            }
        );
        let note = planned.note.expect("the snap is said");
        assert!(note.contains(&format!("{asked}")), "{note}");
        assert!(note.contains(&format!("{legal}")), "{note}");
    }

    /// A value already on the grid passes untouched and says nothing.
    #[test]
    fn a_legal_gain_passes_untouched() {
        let m = SdrMetrics::fixture();
        let stage = m.caps.gain.stages()[0].clone();
        let planned = plan(
            &m,
            &Command::Gain {
                stage: stage.name.clone(),
                db: stage.min_db,
            },
        )
        .unwrap();
        assert_eq!(planned.note, None);
    }

    /// The display switches reach every reader of the state: an address
    /// mode by its name, the pager's mask and own address; a name that is no
    /// mode, or an address wider than 21 bits, is refused with why.
    #[test]
    fn the_display_switches_are_set_or_refused_with_why() {
        let (engine, _) = engine_on(Bench::new());
        let reply = engine.apply(Command::Addresses {
            show: "masked".into(),
        });
        assert!(matches!(reply, Reply::Applied { .. }), "{reply:?}");
        assert_eq!(
            engine.snapshot().net.address_display,
            crate::state::AddressDisplay::Masked
        );
        let Reply::Refused { reason } = engine.apply(Command::Addresses {
            show: "hidden".into(),
        }) else {
            panic!("an unknown mode was applied");
        };
        assert!(reason.contains("full, oui, masked"), "{reason}");

        let reply = engine.apply(Command::Pager {
            masked: true,
            mine: Some(1234),
        });
        assert!(matches!(reply, Reply::Applied { .. }), "{reply:?}");
        let view = engine.snapshot().pager.view;
        assert!(view.masked);
        assert_eq!(view.mine, Some(1234));
        let Reply::Refused { reason } = engine.apply(Command::Pager {
            masked: false,
            mine: Some(PAGER_ADDRESS_MAX + 1),
        }) else {
            panic!("a 22-bit address was applied");
        };
        assert!(reason.contains("21 bits"), "{reason}");
        assert!(
            engine.snapshot().pager.view.masked,
            "a refusal changed nothing"
        );

        // On the wire, as a client sends them.
        let json: Command =
            serde_json::from_str(r#"{"command":"pager","masked":true,"mine":null}"#).unwrap();
        assert_eq!(
            json,
            Command::Pager {
                masked: true,
                mine: None
            }
        );
    }

    #[test]
    fn a_stage_the_radio_does_not_have_is_refused_by_name() {
        let m = SdrMetrics::fixture();
        let why = plan(
            &m,
            &Command::Gain {
                stage: "IFGR".into(),
                db: 20.0,
            },
        )
        .unwrap_err();
        assert!(why.contains("IFGR"), "{why}");
        for stage in m.caps.gain.stages() {
            assert!(why.contains(&stage.name), "the stages it has: {why}");
        }
    }

    #[test]
    fn gains_for_the_wrong_number_of_stages_are_refused() {
        let m = SdrMetrics::fixture();
        let why = plan(&m, &Command::Gains { values: vec![8.0] }).unwrap_err();
        assert!(why.contains('2'), "how many it has: {why}");
    }

    #[test]
    fn a_frequency_outside_the_range_is_refused_with_the_range() {
        let m = SdrMetrics::fixture();
        let why = plan(&m, &Command::Tune { hz: 1 }).unwrap_err();
        assert!(why.contains(&m.caps.bands_text()), "{why}");
    }

    #[test]
    fn a_sample_rate_outside_the_range_is_refused_with_the_range() {
        let m = SdrMetrics::fixture();
        let why = plan(&m, &Command::SampleRate { hz: 1.0 }).unwrap_err();
        assert!(why.contains("MHz"), "{why}");
    }

    #[test]
    fn a_boost_on_a_radio_without_one_is_refused() {
        let mut m = SdrMetrics::fixture();
        Arc::make_mut(&mut m.caps).gain = crate::hardware::GainModel::new(
            vec![crate::hardware::StageSpec::ranged("TUNER", 0.0, 40.0, 1.0)],
            "TUNER",
            "TUN",
        );
        assert!(!m.caps.gain.has_boost());
        assert!(plan(&m, &Command::Boost { on: true }).is_err());
    }

    #[test]
    fn set_mode_is_always_applied() {
        let (engine, _) = engine_on(Bench::new());
        let reply = engine.apply(Command::SetMode {
            section: crate::signal::net::SECTION.into(),
            view: "net_survey".into(),
        });
        assert!(matches!(reply, Reply::Applied { .. }), "{reply:?}");
        let m = engine.snapshot();
        assert!(m.mode.feeds.net && m.mode.net.survey);
    }

    /// Two masters, the person at the keys and a script: the last command
    /// wins, both are answered, and the state holds what the radio was last
    /// told.
    #[test]
    fn the_last_of_two_commands_wins_and_both_are_answered() {
        let (engine, device) = engine_on(Bench::new());
        let first = engine.apply(Command::Tune { hz: 100_000_000 });
        let second = engine.apply(Command::Tune { hz: 200_000_000 });
        assert!(matches!(first, Reply::Applied { .. }), "{first:?}");
        assert!(matches!(second, Reply::Applied { .. }), "{second:?}");
        assert_eq!(engine.snapshot().radio.frequency, 200_000_000);
        assert_eq!(
            *device.tuned.lock().unwrap(),
            vec![100_000_000, 200_000_000]
        );
    }

    /// The radio refusing is an answer with its reason, and the state keeps
    /// describing the radio as it is, not as it was asked to be.
    #[test]
    fn a_refusal_by_the_radio_changes_nothing_and_says_why() {
        let (engine, _) = engine_on(Bench {
            refuse_gain: true,
            ..Bench::new()
        });
        let before = engine.snapshot().radio.gains.clone();
        let stage = engine.snapshot().caps.gain.stages()[0].name.clone();
        let reply = engine.apply(Command::Gain { stage, db: 16.0 });
        match reply {
            Reply::Refused { reason } => assert!(reason.contains("the radio said no"), "{reason}"),
            other => panic!("applied: {other:?}"),
        }
        assert_eq!(engine.snapshot().radio.gains, before);
    }

    /// A gain set from anywhere takes the RF bench off autotrack, as a key
    /// always has: the operator, not the tracker, now owns the chain.
    #[test]
    fn a_gain_applied_lands_in_the_state_and_stops_autotrack() {
        let (engine, _) = engine_on(Bench::new());
        engine.with_state(|m| m.lab.rf_autotrack = true);
        let stage = engine.snapshot().caps.gain.stages()[1].clone();
        let reply = engine.apply(Command::Gain {
            stage: stage.name.clone(),
            db: stage.max_db,
        });
        assert!(matches!(reply, Reply::Applied { .. }), "{reply:?}");
        let m = engine.snapshot();
        assert_eq!(m.radio.stage_gain(1), stage.max_db);
        assert!(!m.lab.rf_autotrack);
    }

    /// Every command is announced to every client, whoever sent it: the
    /// person at the keys sees what a script did, and the other way round.
    #[test]
    fn a_command_is_announced_to_every_subscriber() {
        let (engine, _) = engine_on(Bench::new());
        let watching = engine.subscribe(8);
        let reply = engine.apply(Command::Tune { hz: 100_000_000 });
        let got = watching.try_next().expect("announced");
        assert_eq!(
            got.event,
            crate::engine::events::Event::Applied {
                command: Command::Tune { hz: 100_000_000 },
                reply,
            }
        );
    }

    fn with_an_option(m: &mut SdrMetrics) {
        m.device_options = Arc::new(vec![crate::hardware::DeviceOption {
            id: "bias".into(),
            label: "Bias tee".into(),
            choices: vec!["Off".into(), "On".into()],
            selected_choice: "Off".into(),
            integer_range: None,
        }]);
    }

    /// An option the radio does not offer, or a choice it does not list, is
    /// refused with what it does offer.
    #[test]
    fn a_device_option_the_radio_does_not_offer_is_refused_with_what_it_does() {
        let mut m = SdrMetrics::fixture();
        with_an_option(&mut m);
        let why = plan(
            &m,
            &Command::DeviceOption {
                id: "lna_boost".into(),
                choice: "On".into(),
            },
        )
        .unwrap_err();
        assert!(why.contains("lna_boost") && why.contains("bias"), "{why}");
        let why = plan(
            &m,
            &Command::DeviceOption {
                id: "bias".into(),
                choice: "Maybe".into(),
            },
        )
        .unwrap_err();
        assert!(why.contains("Off") && why.contains("On"), "{why}");
        assert!(plan(
            &m,
            &Command::DeviceOption {
                id: "bias".into(),
                choice: "On".into()
            }
        )
        .is_ok());
    }

    /// An option set through the engine leaves the options as the radio
    /// reports them afterwards.
    #[test]
    fn a_device_option_applied_is_the_radios_word_afterwards() {
        let (engine, device) = engine_on(Bench::new());
        engine.with_state(with_an_option);
        let reply = engine.apply(Command::DeviceOption {
            id: "bias".into(),
            choice: "On".into(),
        });
        assert!(matches!(reply, Reply::Applied { .. }), "{reply:?}");
        assert_eq!(
            *device.options_set.lock().unwrap(),
            vec![("bias".to_string(), "On".to_string())]
        );
        assert_eq!(engine.snapshot().device_options[0].selected_choice, "On");
    }

    #[test]
    fn streaming_and_the_dc_block_land_in_the_state() {
        let (engine, _) = engine_on(Bench::new());
        engine.apply(Command::Stream { on: true });
        engine.apply(Command::DcBlock { on: true });
        let m = engine.snapshot();
        assert!(m.radio.rx_enabled && m.iq.cal.dc_block_on);
        engine.apply(Command::Stream { on: false });
        assert!(!engine.snapshot().radio.rx_enabled);
    }
}
