// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The R2 is shared: what other programs change on it, noticed.
//!
//! `iiod` serves every client that connects, and any of them may retune the
//! radio, change its rate or gain, or start transmitting; nothing in the
//! protocol tells the others. So while sdrtop streams, a connection of its
//! own reads the settings back once a second and compares them with what
//! sdrtop last set ([`Expected`]). A difference is another program's doing:
//! it is queued for the app ([`OutsideChange`]), and the radio's value becomes
//! the expectation. sdrtop never writes its own value back over it.
//!
//! **sdrtop's own writes are never mistaken for someone else's.** The
//! device's setters hold [`Expected`] for the whole of a write and its
//! readback, and the watcher holds it for the whole of a comparison, so the
//! watcher only ever sees the radio before or after one of sdrtop's changes.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use super::iiod::{Client, Target, Timeouts};
use super::tx::{self, Finding};
use crate::hardware::OutsideChange;

const PHY: &str = "ad9361-phy";

/// What sdrtop last set, or last found another program had set.
#[derive(Clone, Debug, PartialEq)]
pub struct Expected {
    /// The gain setting as a position from the band's bottom.
    pub position: f64,
    /// The band's bottom `hardwaregain`, as the radio reports it.
    pub bottom_db: i32,
    pub lo_hz: u64,
    pub rate_hz: f64,
    pub bw_hz: u32,
    pub agc_on: bool,
    /// Another program transmitting, in a few words.
    pub other_tx: Option<String>,
}

/// What a transmitter reading means for the header, in a few words.
///
/// Only what leaves the radio counts: a tone or a buffer set up behind a
/// powered-down LO radiates nothing, and is not "transmitting".
pub fn transmitting(f: &Finding, lo_powered_down: bool) -> Option<String> {
    match f {
        Finding::Other {
            what,
            radiating: true,
        } => Some(what.clone()),
        // Powered up by someone after sdrtop silenced it: a carrier leaks
        // whether or not a signal is set.
        Finding::Idle if !lo_powered_down => Some("its TX LO powered up, no signal set".into()),
        _ => None,
    }
}

/// One comparison: the radio as read against what was expected. Returns the
/// changes, oldest first, and leaves `e` holding the radio's values.
pub fn compare(e: &mut Expected, now: &Expected) -> Vec<OutsideChange> {
    let mut out = Vec::new();
    if now.lo_hz != e.lo_hz {
        out.push(OutsideChange::Frequency(now.lo_hz));
    }
    if (now.rate_hz - e.rate_hz).abs() > 0.5 || now.bw_hz != e.bw_hz {
        out.push(OutsideChange::Rate {
            rate_hz: now.rate_hz,
            bb_filter_hz: now.bw_hz,
        });
    }
    if now.agc_on != e.agc_on {
        out.push(OutsideChange::Agc(now.agc_on));
    }
    // Under the AGC the gain moves by itself; only a manual one is a setting.
    if !now.agc_on && (now.position - e.position).abs() > 0.01 {
        out.push(OutsideChange::Stage(0, now.position));
    }
    if now.other_tx != e.other_tx {
        out.push(OutsideChange::Transmitting(now.other_tx.clone()));
    }
    *e = now.clone();
    out
}

fn number(c: &mut Client, t: Target<'_>, attr: &str) -> Option<f64> {
    tx::parse_db(&c.read(t, attr).ok()?)
}

/// The radio's settings as they stand.
fn read(c: &mut Client) -> Option<Expected> {
    let rx = Target::Input(PHY, "voltage0");
    let lo_hz = number(c, Target::Output(PHY, "altvoltage0"), "frequency")? as u64;
    let rate_hz = number(c, rx, "sampling_frequency")?;
    let bw_hz = number(c, rx, "rf_bandwidth")? as u32;
    let agc_on = c.read(rx, "gain_control_mode").ok()? != "manual";
    let avail = c.read(rx, "hardwaregain_available").ok()?;
    let bottom_db = avail
        .trim_start_matches('[')
        .split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()?
        .round() as i32;
    let hw = number(c, rx, "hardwaregain")?;
    let state = tx::read_state(c).ok()?;
    Some(Expected {
        position: hw - f64::from(bottom_db),
        bottom_db,
        lo_hz,
        rate_hz,
        bw_hz,
        agc_on,
        other_tx: transmitting(&tx::assess(&state), state.lo_powered_down),
    })
}

/// The watcher: a thread and its connection while sdrtop streams.
#[derive(Default)]
pub struct Watch {
    active: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
    changes: Arc<Mutex<Vec<OutsideChange>>>,
}

impl Watch {
    /// Starts reading back every `period`. A watcher that cannot connect
    /// simply does not run: the stream says when the radio is gone.
    pub fn start(
        &self,
        address: &str,
        t: Timeouts,
        period: Duration,
        expected: Arc<Mutex<Expected>>,
        stream_rate: Arc<AtomicU64>,
    ) {
        self.stop();
        let Ok(mut c) = Client::connect(address, t) else {
            return;
        };
        self.active.store(true, Ordering::SeqCst);
        let active = Arc::clone(&self.active);
        let changes = Arc::clone(&self.changes);
        let spawned = std::thread::Builder::new()
            .name("pluto-watch".into())
            .spawn(move || {
                while active.load(Ordering::SeqCst) {
                    std::thread::sleep(period);
                    let mut e = expected.lock().unwrap_or_else(|p| p.into_inner());
                    let Some(now) = read(&mut c) else { break };
                    let found = compare(&mut e, &now);
                    if found
                        .iter()
                        .any(|f| matches!(f, OutsideChange::Rate { .. }))
                    {
                        stream_rate.store(now.rate_hz.to_bits(), Ordering::Relaxed);
                    }
                    drop(e);
                    if !found.is_empty() {
                        changes
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .extend(found);
                    }
                }
                c.exit();
            });
        if let Ok(h) = spawned {
            *self.thread.lock().unwrap_or_else(|p| p.into_inner()) = Some(h);
        }
    }

    pub fn stop(&self) {
        self.active.store(false, Ordering::SeqCst);
        if let Some(h) = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = h.join();
        }
    }

    /// What was found since last asked.
    pub fn take(&self) -> Vec<OutsideChange> {
        std::mem::take(&mut *self.changes.lock().unwrap_or_else(|p| p.into_inner()))
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Expected {
        Expected {
            position: 20.0,
            bottom_db: -1,
            lo_hz: 433_500_000,
            rate_hz: 3e6,
            bw_hz: 3_000_000,
            agc_on: false,
            other_tx: None,
        }
    }

    #[test]
    fn nothing_changed_is_nothing_said() {
        let mut e = base();
        assert!(compare(&mut e, &base()).is_empty());
    }

    /// Each kind of change found, in order, and the radio's values kept.
    #[test]
    fn another_programs_changes_are_found_and_kept() {
        let mut e = base();
        let now = Expected {
            lo_hz: 145_500_000,
            rate_hz: 4e6,
            bw_hz: 4_000_000,
            position: 30.0,
            other_tx: Some("1 DDS tone set".into()),
            ..base()
        };
        assert_eq!(
            compare(&mut e, &now),
            [
                OutsideChange::Frequency(145_500_000),
                OutsideChange::Rate {
                    rate_hz: 4e6,
                    bb_filter_hz: 4_000_000
                },
                OutsideChange::Stage(0, 30.0),
                OutsideChange::Transmitting(Some("1 DDS tone set".into())),
            ]
        );
        assert_eq!(e, now, "the radio's word is kept");
        assert!(compare(&mut e, &now).is_empty(), "said once");
    }

    /// Under the AGC the gain moves by itself; that is not another program.
    #[test]
    fn the_agcs_own_moves_are_not_changes() {
        let mut e = Expected {
            agc_on: true,
            ..base()
        };
        let now = Expected {
            agc_on: true,
            position: 55.0,
            ..base()
        };
        assert!(compare(&mut e, &now).is_empty());
    }

    #[test]
    fn a_signal_behind_a_powered_down_lo_is_not_transmitting() {
        let set_up = Finding::Other {
            radiating: false,
            what: "1 DDS tone set".into(),
        };
        assert_eq!(transmitting(&set_up, true), None);
        let on_air = Finding::Other {
            radiating: true,
            what: "1 DDS tone set".into(),
        };
        assert_eq!(
            transmitting(&on_air, false).as_deref(),
            Some("1 DDS tone set")
        );
    }

    #[test]
    fn a_tx_lo_powered_up_by_someone_leaks_and_is_said() {
        assert_eq!(
            transmitting(&Finding::Idle, false).as_deref(),
            Some("its TX LO powered up, no signal set")
        );
        assert_eq!(transmitting(&Finding::Idle, true), None);
        assert_eq!(transmitting(&Finding::Silent, true), None);
    }
}
