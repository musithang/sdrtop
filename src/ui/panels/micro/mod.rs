//! The compact field views cycled by `0`.
//!
//! [`entry`] is the overview the cycle starts on; the rest each strip one concern
//! down to a single glance. All of them adapt across three width modes so they
//! stay readable from an 80x24 SSH session down to a 40-column framebuffer.

pub mod entry;
pub mod gain;
pub mod health;
pub mod signal;
pub mod sweep;
