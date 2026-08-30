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

// Nothing calls the model yet: the engine picks it up next, and this crate is a
// binary, so `pub` does not make an unused item reachable the way it would in a
// library. CI runs clippy with `-D warnings`, so without this the checkpoint
// that adds a tested module and the one that wires it in could not be separate
// commits. Delete this line the moment `LayoutEngine` holds a `Menu`.
#[allow(dead_code)]
pub mod model;
