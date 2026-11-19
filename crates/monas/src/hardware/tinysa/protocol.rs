// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use anyhow::{bail, Context};

pub(super) const PROMPT: &[u8] = b"ch> ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Model {
    Basic,
    Zs405,
    Zs406,
    Zs407,
    UltraUnknown,
}

impl Model {
    pub(super) fn is_ultra(self) -> bool {
        self != Self::Basic
    }

    pub(super) fn maximum_hz(self) -> u64 {
        match self {
            Self::Basic => 959_000_000,
            Self::Zs405 | Self::Zs406 | Self::UltraUnknown => 6_000_000_000,
            Self::Zs407 => 7_300_000_000,
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Basic => "tinySA",
            Self::Zs405 => "tinySA Ultra ZS405",
            Self::Zs406 => "tinySA Ultra ZS406",
            Self::Zs407 => "tinySA Ultra ZS407",
            Self::UltraUnknown => "tinySA Ultra",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Identity {
    pub(super) firmware: String,
    pub(super) hardware: Option<String>,
    pub(super) board: String,
    pub(super) zero_dbm: i32,
    pub(super) model: Model,
}

pub(super) fn parse_text_frame(frame: &[u8], command: &str) -> anyhow::Result<Vec<u8>> {
    if !frame.ends_with(PROMPT) {
        bail!("tinySA response did not end at the shell prompt");
    }
    let without_prompt = &frame[..frame.len() - PROMPT.len()];
    let mut echo = command.as_bytes().to_vec();
    echo.extend_from_slice(b"\r\n");
    if !without_prompt.starts_with(&echo) {
        bail!("tinySA response did not echo the command");
    }
    let body = &without_prompt[echo.len()..];
    let token = command.split_ascii_whitespace().next().unwrap_or(command);
    let unknown = format!("{token}?");
    if String::from_utf8_lossy(body)
        .lines()
        .any(|line| line.trim() == unknown)
    {
        bail!("tinySA does not support the {token} command");
    }
    Ok(body.to_vec())
}

pub(super) fn validate_setter_body(body: &[u8], command: &str) -> anyhow::Result<()> {
    if !body.is_empty() {
        bail!(
            "tinySA {command} command returned an unexpected response: {}",
            escaped_excerpt(body)
        );
    }
    Ok(())
}

fn escaped_excerpt(body: &[u8]) -> String {
    const MAX_CHARS: usize = 120;
    let mut excerpt = String::new();
    for byte in body {
        for escaped in std::ascii::escape_default(*byte) {
            if excerpt.len() == MAX_CHARS {
                excerpt.push_str("...");
                return excerpt;
            }
            excerpt.push(char::from(escaped));
        }
    }
    excerpt
}

pub(super) fn parse_identity(
    version: &[u8],
    info: &[u8],
    help: &[u8],
    zero: &[u8],
) -> anyhow::Result<Identity> {
    let version = std::str::from_utf8(version).context("tinySA version response was not text")?;
    let info = std::str::from_utf8(info).context("tinySA info response was not text")?;
    std::str::from_utf8(help).context("tinySA help response was not text")?;
    let firmware = first_line(version)
        .filter(|line| line.to_ascii_lowercase().contains("tinysa"))
        .context("serial device did not identify itself as a tinySA")?
        .to_string();
    let combined = format!("{version}\n{info}");
    let upper = combined.to_ascii_uppercase();
    let ultra = upper.contains("TINYSA4") || upper.contains("TINYSA ULTRA");
    let model = if !ultra {
        Model::Basic
    } else if upper.contains("ZS405") || upper.contains("V0.4.5.1.1") || upper.contains("V0.4.5.1")
    {
        Model::Zs405
    } else if upper.contains("ZS406") || upper.contains("V0.4.6") {
        Model::Zs406
    } else if upper.contains("ZS407") || upper.contains("V0.5.4") || upper.contains("MAX2871") {
        Model::Zs407
    } else {
        Model::UltraUnknown
    };
    let hardware = version
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("HW Version:"))
        .map(str::to_string);
    let board = first_line(info).unwrap_or_else(|| model.name()).to_string();
    Ok(Identity {
        firmware,
        hardware,
        board,
        zero_dbm: parse_zero(zero)?,
        model,
    })
}

fn first_line(value: &str) -> Option<&str> {
    value.lines().map(str::trim).find(|line| !line.is_empty())
}

pub(super) fn parse_zero(response: &[u8]) -> anyhow::Result<i32> {
    let response = std::str::from_utf8(response).context("tinySA zero response was not text")?;
    response
        .lines()
        .map(str::trim)
        .find_map(|line| {
            let number = line
                .strip_suffix("dBm")
                .or_else(|| line.strip_suffix("DBM"))?
                .trim();
            number.parse::<i32>().ok()
        })
        .context("tinySA zero response did not contain a dBm value")
}

pub(super) fn parse_scan_frame(
    frame: &[u8],
    points: u32,
    zero_dbm: i32,
) -> anyhow::Result<Vec<f32>> {
    let expected = 1usize
        .checked_add(points as usize * 3)
        .and_then(|size| size.checked_add(1))
        .context("tinySA scan point count is too large")?;
    if frame.first() != Some(&b'{') {
        bail!("tinySA scan frame is missing its opening tag");
    }
    let mut levels = Vec::with_capacity(points as usize);
    let mut offset = 1;
    for index in 0..points {
        let Some(&tag) = frame.get(offset) else {
            bail!("tinySA scan ended after {index} of {points} records");
        };
        if tag == b'}' {
            bail!("tinySA scan closed after {index} of {points} records");
        }
        if tag != b'x' {
            bail!("tinySA scan record {index} has malformed tag 0x{tag:02x}");
        }
        let Some(bytes) = frame.get(offset + 1..offset + 3) else {
            bail!("tinySA scan record {index} ended early");
        };
        let raw = i16::from_le_bytes([bytes[0], bytes[1]]);
        levels.push(raw as f32 / 32.0 - zero_dbm as f32);
        offset += 3;
    }
    if frame.get(offset) != Some(&b'}') {
        bail!("tinySA scan frame is missing its closing tag");
    }
    if frame.len() != expected {
        bail!("tinySA scan frame has trailing data");
    }
    Ok(levels)
}

pub(super) fn scan_frequencies(start_hz: u64, stop_hz: u64, points: u32) -> Vec<u64> {
    if points == 0 || stop_hz < start_hz {
        return Vec::new();
    }
    let step = ((stop_hz - start_hz) / points as u64) as f32;
    (0..points)
        .map(|index| start_hz + (step * index as f32) as u64)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_frames_require_the_echo_and_prompt() {
        assert_eq!(
            parse_text_frame(b"version\r\ntinySA_v1.4\r\nch> ", "version").unwrap(),
            b"tinySA_v1.4\r\n"
        );
        assert!(parse_text_frame(b"tinySA_v1.4\r\nch> ", "version").is_err());
        assert!(parse_text_frame(b"version\r\ntinySA_v1.4\r\n", "version").is_err());
        assert!(parse_text_frame(b"frobnicate\r\nfrobnicate?\r\nch> ", "frobnicate").is_err());
    }

    #[test]
    fn setters_require_an_empty_response_body() {
        assert!(validate_setter_body(b"", "rbw 10").is_ok());
        for body in [
            b"usage: rbw {auto|3|10}\r\n".as_slice(),
            b"error: invalid value\r\n".as_slice(),
            b"ok\r\n".as_slice(),
            b"\r\n".as_slice(),
        ] {
            assert!(validate_setter_body(body, "rbw 10").is_err());
        }
    }

    #[test]
    fn setter_rejections_include_a_bounded_escaped_excerpt() {
        let mut body = b"error:\tinvalid\x00value\r\n".to_vec();
        body.extend(std::iter::repeat_n(b'x', 200));

        let error = validate_setter_body(&body, "rbw 10")
            .unwrap_err()
            .to_string();

        assert!(error.contains(r"error:\tinvalid\x00value\r\n"));
        assert!(error.ends_with("..."));
        assert!(error.len() < 200);
    }

    #[test]
    fn basic_identity_does_not_need_a_hardware_line() {
        let identity = parse_identity(
            b"tinySA_v1.4-1-gabc\r\n",
            b"tinySA v0.3ZS304\r\nVersion: tinySA_v1.4-1-gabc\r\n",
            b"commands: version info help zero scanraw\r\n",
            b"zero {level}\r\n128dBm\r\n",
        )
        .unwrap();
        assert_eq!(identity.model, Model::Basic);
        assert_eq!(identity.hardware, None);
        assert_eq!(identity.zero_dbm, 128);
    }

    #[test]
    fn ultra_models_are_read_from_the_info_response() {
        for (suffix, expected) in [
            ("ZS405", Model::Zs405),
            ("ZS406", Model::Zs406),
            ("ZS407", Model::Zs407),
            ("prototype", Model::UltraUnknown),
        ] {
            let info = format!("tinySA ULTRA {suffix}\r\n");
            let identity = parse_identity(
                b"tinySA4_v1.4\r\nHW Version:future\r\n",
                info.as_bytes(),
                b"commands: scanraw\r\n",
                b"174dBm\r\n",
            )
            .unwrap();
            assert_eq!(identity.model, expected);
            assert_eq!(identity.hardware.as_deref(), Some("HW Version:future"));
        }
    }

    #[test]
    fn ultra_models_fall_back_to_the_hardware_revision() {
        for (hardware, expected) in [
            ("V0.4.5.1.1", Model::Zs405),
            ("V0.4.6", Model::Zs406),
            ("V0.5.4 max2871", Model::Zs407),
        ] {
            let version = format!("tinySA4_v1.4\r\nHW Version:{hardware}\r\n");
            let identity = parse_identity(
                version.as_bytes(),
                b"tinySA ULTRA\r\n",
                b"commands: scanraw\r\n",
                b"174dBm\r\n",
            )
            .unwrap();
            assert_eq!(identity.model, expected, "{hardware}");
        }
    }

    #[test]
    fn zero_parser_accepts_signed_values() {
        assert_eq!(parse_zero(b"zero {level}\r\n174dBm\r\n").unwrap(), 174);
        assert_eq!(parse_zero(b"-174dBm\r\n").unwrap(), -174);
        assert!(parse_zero(b"zero unavailable\r\n").is_err());
    }

    #[test]
    fn scan_payload_bytes_are_never_treated_as_tags() {
        let frame = [b'{', b'x', b'{', 0, b'x', 0, b'}', b'x', b'x', 0, b'}'];
        let levels = parse_scan_frame(&frame, 3, 0).unwrap();
        assert_eq!(levels, vec![123.0 / 32.0, 32000.0 / 32.0, 120.0 / 32.0]);
    }

    #[test]
    fn negative_raw_levels_keep_their_sign() {
        let levels = parse_scan_frame(&[b'{', b'x', 0x40, 0xff, b'}'], 1, 174).unwrap();
        assert_eq!(levels, vec![-180.0]);
    }

    #[test]
    fn scan_parser_rejects_bad_tags_and_truncation() {
        assert!(parse_scan_frame(b"{q\0\0}", 1, 0)
            .unwrap_err()
            .to_string()
            .contains("malformed tag"));
        assert!(parse_scan_frame(b"{}", 1, 0)
            .unwrap_err()
            .to_string()
            .contains("closed after"));
        assert!(parse_scan_frame(b"{x\0\0", 1, 0)
            .unwrap_err()
            .to_string()
            .contains("closing tag"));
        assert!(parse_scan_frame(b"{x\0", 1, 0).is_err());
        assert!(parse_scan_frame(b"{x\0\0}extra", 1, 0)
            .unwrap_err()
            .to_string()
            .contains("trailing data"));
    }

    #[test]
    fn frequency_mapping_matches_scanraw_integer_then_float_math() {
        assert_eq!(
            scan_frequencies(100_000_003, 100_001_006, 4),
            vec![100_000_003, 100_000_253, 100_000_503, 100_000_753]
        );
        let large = scan_frequencies(6_000_000_001, 6_000_100_101, 3);
        let step = ((100_100u64 / 3) as f32) as u64;
        assert_eq!(
            large,
            vec![
                6_000_000_001,
                6_000_000_001 + step,
                6_000_000_001 + 2 * step
            ]
        );
        assert!(scan_frequencies(10, 9, 3).is_empty());
        assert!(scan_frequencies(10, 20, 0).is_empty());
    }
}
