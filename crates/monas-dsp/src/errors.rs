// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The Controller's error codes, which both Bluetooths' link layers send:
//! a classic LMP_DETACH or LMP_NOT_ACCEPTED and an LE LL_TERMINATE_IND or
//! LL_REJECT_IND each carry one.
//!
//! **Core 5.4 Vol 1 Part F, Table 1.1, "List of possible error codes"**,
//! transcribed mechanically from the SIG's HTML copy on 2026-10-01, the
//! reserved and previously used codes as the table words them. A test holds
//! the table to running 0x00 to 0x47 once each.

/// Table 1.1: each code and its name.
pub const ERRORS: &[(u8, &str)] = &[
    (0x00, "Success"),
    (0x01, "Unknown HCI Command"),
    (0x02, "Unknown Connection Identifier"),
    (0x03, "Hardware Failure"),
    (0x04, "Page Timeout"),
    (0x05, "Authentication Failure"),
    (0x06, "PIN or Key Missing"),
    (0x07, "Memory Capacity Exceeded"),
    (0x08, "Connection Timeout"),
    (0x09, "Connection Limit Exceeded"),
    (0x0a, "Synchronous Connection Limit To A Device Exceeded"),
    (0x0b, "Connection Already Exists"),
    (0x0c, "Command Disallowed"),
    (0x0d, "Connection Rejected due to Limited Resources"),
    (0x0e, "Connection Rejected Due To Security Reasons"),
    (0x0f, "Connection Rejected due to Unacceptable BD_ADDR"),
    (0x10, "Connection Accept Timeout Exceeded"),
    (0x11, "Unsupported Feature or Parameter Value"),
    (0x12, "Invalid HCI Command Parameters"),
    (0x13, "Remote User Terminated Connection"),
    (
        0x14,
        "Remote Device Terminated Connection due to Low Resources",
    ),
    (0x15, "Remote Device Terminated Connection due to Power Off"),
    (0x16, "Connection Terminated By Local Host"),
    (0x17, "Repeated Attempts"),
    (0x18, "Pairing Not Allowed"),
    (0x19, "Unknown LMP PDU"),
    (0x1a, "Unsupported Remote Feature"),
    (0x1b, "SCO Offset Rejected"),
    (0x1c, "SCO Interval Rejected"),
    (0x1d, "SCO Air Mode Rejected"),
    (0x1e, "Invalid LMP Parameters / Invalid LL Parameters"),
    (0x1f, "Unspecified Error"),
    (
        0x20,
        "Unsupported LMP Parameter Value / Unsupported LL Parameter Value",
    ),
    (0x21, "Role Change Not Allowed"),
    (0x22, "LMP Response Timeout / LL Response Timeout"),
    (
        0x23,
        "LMP Error Transaction Collision / LL Procedure Collision",
    ),
    (0x24, "LMP PDU Not Allowed"),
    (0x25, "Encryption Mode Not Acceptable"),
    (0x26, "Link Key cannot be Changed"),
    (0x27, "Requested QoS Not Supported"),
    (0x28, "Instant Passed"),
    (0x29, "Pairing With Unit Key Not Supported"),
    (0x2a, "Different Transaction Collision"),
    (0x2b, "Reserved for future use"),
    (0x2c, "QoS Unacceptable Parameter"),
    (0x2d, "QoS Rejected"),
    (0x2e, "Channel Classification Not Supported"),
    (0x2f, "Insufficient Security"),
    (0x30, "Parameter Out Of Mandatory Range"),
    (0x31, "Reserved for future use"),
    (0x32, "Role Switch Pending"),
    (0x33, "Reserved for future use"),
    (0x34, "Reserved Slot Violation"),
    (0x35, "Role Switch Failed"),
    (0x36, "Extended Inquiry Response Too Large"),
    (0x37, "Secure Simple Pairing Not Supported By Host"),
    (0x38, "Host Busy - Pairing"),
    (0x39, "Connection Rejected due to No Suitable Channel Found"),
    (0x3a, "Controller Busy"),
    (0x3b, "Unacceptable Connection Parameters"),
    (0x3c, "Advertising Timeout"),
    (0x3d, "Connection Terminated due to MIC Failure"),
    (
        0x3e,
        "Connection Failed to be Established / Synchronization Timeout",
    ),
    (0x3f, "Previously used"),
    (
        0x40,
        "Coarse Clock Adjustment Rejected but Will Try to Adjust Using Clock Dragging",
    ),
    (0x41, "Type0 Submap Not Defined"),
    (0x42, "Unknown Advertising Identifier"),
    (0x43, "Limit Reached"),
    (0x44, "Operation Cancelled by Host"),
    (0x45, "Packet Too Long"),
    (0x46, "Too Late"),
    (0x47, "Too Early"),
];

/// A code's name and number, `"Remote User Terminated Connection (0x13)"`,
/// or what it is when the table has no such code.
pub fn error_name(code: u8) -> String {
    match ERRORS.iter().find(|e| e.0 == code) {
        Some((_, name)) => format!("{name} (0x{code:02x})"),
        None => format!("error 0x{code:02x}: not in Vol 1 Part F Table 1.1"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every code of Table 1.1 once, in order, 0x00 to 0x47.
    #[test]
    fn the_table_runs_from_success_to_too_early() {
        let codes: Vec<u8> = ERRORS.iter().map(|e| e.0).collect();
        assert_eq!(codes, (0x00..=0x47).collect::<Vec<u8>>());
        assert_eq!(ERRORS[0].1, "Success");
        assert_eq!(ERRORS[0x47].1, "Too Early");
    }

    #[test]
    fn a_code_reads_as_its_name_and_number() {
        assert_eq!(error_name(0x13), "Remote User Terminated Connection (0x13)");
        assert_eq!(
            error_name(0x55),
            "error 0x55: not in Vol 1 Part F Table 1.1"
        );
    }
}
