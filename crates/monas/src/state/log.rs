// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The log: what happened, in words, as it happened.
//!
//! The engine's, not the screen's: a worker, a task and a command all say
//! what they did here, and a headless instance has the same log with nothing
//! drawing it. The log panel and the command rail's foot are two of its
//! readers.

use std::sync::Arc;

/// How many lines are kept; the oldest goes first.
pub const LOG_MAX_ENTRIES: usize = 100;

/// Severity of a log line, used by the log panel to pick a status lamp + colour.
/// Derived from the message text (see [`LogLevel::infer`]) so the ~86 existing
/// `push_log` call sites keep working unchanged.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Info,
    Ok,
    Warn,
    Error,
}

impl LogLevel {
    /// Classify a message by keyword. Order matters: hard failures win over the
    /// "warning" prefix, which wins over success words. Everything else is Info.
    pub fn infer(msg: &str) -> LogLevel {
        let m = msg.to_ascii_lowercase();
        let has = |needles: &[&str]| needles.iter().any(|n| m.contains(n));
        if has(&["error", "fail", "unable", "in use", "could not"]) {
            LogLevel::Error
        } else if has(&["warning", "warn", "no marker", "unexpectedly"]) {
            LogLevel::Warn
        } else if has(&["started", "connected", "enabled", "success"]) {
            LogLevel::Ok
        } else {
            LogLevel::Info
        }
    }
}

/// One structured log line: when it happened (Unix epoch seconds, captured at
/// push time so the timestamp is fixed), its severity, and the message text.
#[derive(Clone)]
pub struct LogEntry {
    pub at_epoch_secs: u64,
    pub level: LogLevel,
    pub text: Arc<str>,
}

/// Append a line, stamped now, dropping the oldest past [`LOG_MAX_ENTRIES`].
/// The line as appended.
pub fn push(log: &mut std::collections::VecDeque<LogEntry>, msg: impl Into<String>) -> LogEntry {
    let text = msg.into();
    let level = LogLevel::infer(&text);
    let at_epoch_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if log.len() >= LOG_MAX_ENTRIES {
        log.pop_front();
    }
    let entry = LogEntry {
        at_epoch_secs,
        level,
        text: Arc::from(text),
    };
    log.push_back(entry.clone());
    entry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_log_captures_level_and_time() {
        let mut m = crate::state::SdrMetrics::fixture();
        m.push_log("Tune error: nope");
        let e = m.log.back().unwrap();
        assert_eq!(e.level, LogLevel::Error);
        assert_eq!(e.text.as_ref(), "Tune error: nope");
        assert!(e.at_epoch_secs > 0, "timestamp captured");
    }

    #[test]
    fn infer_flags_failures_as_error() {
        for msg in [
            "Tune error: out of range",
            "Error stopping RX: timeout",
            "Startup: failed to set gain",
            "Device is in use by another process",
            "Reset error: denied",
        ] {
            assert_eq!(LogLevel::infer(msg), LogLevel::Error, "{msg:?}");
        }
    }

    #[test]
    fn infer_flags_warnings() {
        for msg in [
            "WARNING: Streaming stopped unexpectedly — press [Space]",
            "No marker near cursor — place one with [M] first",
        ] {
            assert_eq!(LogLevel::infer(msg), LogLevel::Warn, "{msg:?}");
        }
    }

    #[test]
    fn infer_flags_success_events() {
        assert_eq!(LogLevel::infer("RX streaming started"), LogLevel::Ok);
        assert_eq!(
            LogLevel::infer("Connected: HackRF One | Serial: …"),
            LogLevel::Ok
        );
    }

    #[test]
    fn infer_defaults_to_info() {
        for msg in [
            "Step → 100 kHz",
            "Freq zoom: ×4",
            "Preset: spectrum",
            "RX streaming stopped",
        ] {
            assert_eq!(LogLevel::infer(msg), LogLevel::Info, "{msg:?}");
        }
    }

    /// Every log line is also an event, so a client with no log panel hears
    /// what the log says, as it says it.
    #[test]
    fn a_log_line_is_an_event() {
        let mut m = crate::state::SdrMetrics::fixture();
        let watching = m.events.subscribe(4);
        m.push_log("Tune error: nope");
        match watching.try_next().expect("published").event {
            crate::engine::events::Event::Log { level, text, .. } => {
                assert_eq!(level, LogLevel::Error);
                assert_eq!(text, "Tune error: nope");
            }
            other => panic!("{other:?}"),
        }
    }
}
