//! The SoapySDR backend: one API, and behind it most of the radios sdrtop does
//! not have a driver for.
//!
//! Split on the seam that makes it testable with no radio and no libSoapySDR,
//! which is what CI has: **query, then interpret, then apply.**
//!
//! - [`api`]: the dlopen'd symbol table. The only file with `unsafe extern`
//!   declarations for this library.
//! - [`args`]: device arguments and labels. Safe and pure.
//! - [`caps`]: driver answers to [`crate::hardware::DeviceCapabilities`]. Safe
//!   and pure, and where the actual thinking is.
//! - [`device`]: `SoapyDevice`. Orchestration: the order things happen in.
//! - [`stream`]: the RX stream and its owned read thread.
//!
//! The parts that decide anything take plain Rust data and return plain Rust
//! data. They never see a device pointer, which is why the interesting logic can
//! be checked by a machine with nothing plugged into it.

pub mod api;
pub mod args;
pub mod caps;
pub mod device;
pub mod stream;
