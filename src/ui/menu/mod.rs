//! The menu: the app's launcher and its key reference.
//!
//! **Not a panel.** Full-screen UI in the family of [`crate::ui::overlay`] and
//! [`crate::ui::device_selector`], drawn outside the layout engine, into a
//! `Rect` the caller supplies: the whole screen at startup, a centred box over
//! a dimmed deck during a session. One function, two callers.
//!
//! Split by what each part draws, the way `panels/core/spectrum/` is:
//!
//! - [`model`]: presets to sections. The only part with logic, and the only part
//!   that never touches ratatui.
//!
//! The parts below `model` arrive with the screen itself; this module is the
//! orchestrator that will resolve the frame, carve the columns and call each of
//! them once.

// The engine holds a `Menu` now, but its accessors are still only reached from
// tests, so `Section`, `Entry` and the lookups on `Menu` are not yet live code.
// This is a binary crate, where `pub` does not keep an uncalled item alive, and
// CI runs clippy with `-D warnings`. Delete this the moment the menu renders.
#[allow(dead_code)]
pub mod model;
