// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What happened once and must not be missed by sampling: a command
//! answered, a log line, samples lost, the radio going quiet or coming back.
//!
//! A reading can be polled, because the next poll has the next value; an
//! event cannot, because between two polls it has happened and gone. So each
//! client subscribes and gets its own queue.
//!
//! **Bounded, and lossy where it has to be, the way the sample feeds are.**
//! Publishing never waits: a client whose queue is full loses the event, the
//! loss is counted against that client only, and the next event that does
//! reach it carries the count. A slow client costs itself events, never the
//! engine time; a fast one beside it loses nothing.

use std::sync::Mutex;

use crossbeam_channel::{Receiver, Sender, TrySendError};
use serde::Serialize;

use super::command::{Command, Reply};
use crate::state::LogLevel;

/// Something that happened. See the module comment.
#[derive(Clone, PartialEq, Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// A command was decided and answered, by whoever sent it.
    Applied { command: Command, reply: Reply },
    /// A line was added to the log.
    Log {
        at_epoch_secs: u64,
        level: LogLevel,
        text: String,
    },
    /// Samples went missing from the stream, split by whose loss it was.
    Gap {
        at_unix_ms: u64,
        samples: u64,
        driver: u64,
        sdrtop: u64,
    },
    /// The radio stopped delivering (`stale: true`) or started again.
    Stale { stale: bool },
}

/// One event as one client receives it.
#[derive(Clone, PartialEq, Debug, Serialize)]
pub struct Delivered {
    pub event: Event,
    /// Events this client missed, its queue full, just before this one.
    pub lost_before: u64,
}

struct Subscriber {
    tx: Sender<Delivered>,
    /// Lost since the last event that reached it.
    pending: u64,
}

/// Where events are published, and every client's queue.
#[derive(Default)]
pub struct Bus {
    subscribers: Mutex<Vec<Subscriber>>,
}

impl Bus {
    /// A new client, with room for `capacity` events it has not read yet.
    pub fn subscribe(&self, capacity: usize) -> Subscription {
        let (tx, rx) = crossbeam_channel::bounded(capacity.max(1));
        self.lock().push(Subscriber { tx, pending: 0 });
        Subscription { rx }
    }

    /// Offer an event to every client. Never waits.
    pub fn publish(&self, event: Event) {
        let mut subscribers = self.lock();
        if subscribers.is_empty() {
            return;
        }
        subscribers.retain_mut(|s| {
            match s.tx.try_send(Delivered {
                event: event.clone(),
                lost_before: s.pending,
            }) {
                Ok(()) => {
                    s.pending = 0;
                    true
                }
                Err(TrySendError::Full(_)) => {
                    s.pending += 1;
                    true
                }
                Err(TrySendError::Disconnected(_)) => false,
            }
        });
    }

    #[cfg(test)]
    fn subscribers(&self) -> usize {
        self.lock().len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Subscriber>> {
        self.subscribers.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// One client's end of the bus. Dropping it unsubscribes.
pub struct Subscription {
    rx: Receiver<Delivered>,
}

impl Subscription {
    /// The next event, if one is waiting.
    #[cfg(test)]
    pub fn try_next(&self) -> Option<Delivered> {
        self.rx.try_recv().ok()
    }

    /// The next event, waiting for it; `None` once the engine is gone.
    pub fn next_blocking(&self) -> Option<Delivered> {
        self.rx.recv().ok()
    }

    /// The next event, waiting at most `at_most` for it.
    pub fn wait(&self, at_most: std::time::Duration) -> Waited {
        match self.rx.recv_timeout(at_most) {
            Ok(delivered) => Waited::Event(delivered),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => Waited::Quiet,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Waited::Gone,
        }
    }
}

/// What waiting for an event came to.
pub enum Waited {
    Event(Delivered),
    /// Nothing happened in the time allowed.
    Quiet,
    /// The engine is gone; nothing more will.
    Gone,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(n: u64) -> Event {
        Event::Stale {
            stale: n.is_multiple_of(2),
        }
    }

    /// **A slow client never holds anything up, and is told what it missed.**
    /// Its queue fills, every publish after that returns at once and is
    /// counted against it, and the next event that reaches it says how many
    /// went before it unseen.
    #[test]
    fn a_slow_subscriber_loses_counted_events_and_never_blocks() {
        let bus = Bus::default();
        let slow = bus.subscribe(4);
        for n in 0..10 {
            bus.publish(line(n));
        }
        for n in 0..4 {
            let got = slow.try_next().expect("the four that fitted");
            assert_eq!(got.event, line(n));
            assert_eq!(got.lost_before, 0);
        }
        assert!(slow.try_next().is_none());
        bus.publish(line(10));
        let next = slow.try_next().expect("room again");
        assert_eq!(next.event, line(10));
        assert_eq!(next.lost_before, 6, "the six it was not there for");
        assert_eq!(slow.try_next(), None);
    }

    /// One client's loss is its own: another, reading as fast as events
    /// come, sees every one with nothing missed.
    #[test]
    fn a_fast_subscriber_loses_nothing_beside_a_slow_one() {
        let bus = Bus::default();
        let _slow = bus.subscribe(1);
        let fast = bus.subscribe(1);
        for n in 0..10 {
            bus.publish(line(n));
            let got = fast.try_next().expect("every one");
            assert_eq!((got.event, got.lost_before), (line(n), 0));
        }
    }

    /// A client that went away is forgotten on the next publish, rather than
    /// counted against for ever.
    #[test]
    fn a_dropped_subscriber_is_forgotten() {
        let bus = Bus::default();
        let gone = bus.subscribe(4);
        let kept = bus.subscribe(4);
        drop(gone);
        bus.publish(line(0));
        assert_eq!(bus.subscribers(), 1);
        assert!(kept.try_next().is_some());
    }

    /// With nobody listening, publishing is a lock and an empty list.
    #[test]
    fn publishing_to_nobody_is_harmless() {
        let bus = Bus::default();
        bus.publish(line(0));
        assert_eq!(bus.subscribers(), 0);
    }
}
