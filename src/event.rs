// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use crossterm::event::{self, Event, KeyEvent};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use crate::hardware::DeviceOption;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceOptionRequest {
    pub id: String,
    pub label: String,
    pub choice: String,
}

#[derive(Debug)]
pub struct DeviceOptionCompletion {
    pub result: Result<Vec<DeviceOption>, String>,
}

pub enum AppEvent {
    Key(KeyEvent),
    Tick,
    DeviceOptionComplete(DeviceOptionCompletion),
    InputError(std::io::Error),
}

pub struct EventStream {
    tx: Sender<AppEvent>,
    rx: Receiver<AppEvent>,
}

impl EventStream {
    pub fn new(tick_rate: Duration) -> Self {
        let (tx, rx) = mpsc::channel();
        let event_tx = tx.clone();
        let spawned = thread::Builder::new()
            .name("key-input".to_string())
            .spawn(move || loop {
                let has_event = match event::poll(tick_rate) {
                    Ok(has_event) => has_event,
                    Err(error) => {
                        let _ = event_tx.send(AppEvent::InputError(error));
                        break;
                    }
                };

                if !has_event {
                    if event_tx.send(AppEvent::Tick).is_err() {
                        break;
                    }
                    continue;
                }

                match event::read() {
                    Ok(Event::Key(key)) => {
                        if event_tx.send(AppEvent::Key(key)).is_err() {
                            break;
                        }
                    }
                    // Trigger an immediate redraw so preferred_height re-runs with the new width.
                    Ok(Event::Resize(..)) => {
                        if event_tx.send(AppEvent::Tick).is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        let _ = event_tx.send(AppEvent::InputError(error));
                        break;
                    }
                }
            });
        spawned.expect("cannot start the key-input thread");
        Self { tx, rx }
    }

    pub fn recv(&self) -> AppEvent {
        self.rx.recv().unwrap_or(AppEvent::Tick)
    }

    pub fn sender(&self) -> Sender<AppEvent> {
        self.tx.clone()
    }

    #[cfg(test)]
    pub fn from_channel(tx: Sender<AppEvent>, rx: Receiver<AppEvent>) -> Self {
        Self { tx, rx }
    }
}
