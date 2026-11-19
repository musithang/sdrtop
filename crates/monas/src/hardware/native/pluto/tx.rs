// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The R2's transmitter at open: what state it is in, and silencing it.
//!
//! **sdrtop receives; the transmitter is silenced at open and left silent.**
//! After power-up the firmware leaves the TX LO running with 10 dB of
//! attenuation and the tone generator at zero. Measured on the R2, that idle
//! LO shows in its own receiver about 46 dB over the noise, and with both LOs
//! at the firmware's 2.4 GHz it lands on DC and clips the converter. So at
//! open both TX channels go to the chip's most attenuation and the TX LO is
//! powered down, each written and read back; a readback that disagrees refuses
//! the open, because the radio is then in a state sdrtop did not intend.
//! Nothing is restored at close: the firmware's own state returns at the
//! radio's next power-up.
//!
//! **Another program transmitting is left alone.** Whether one is can be read
//! without disturbing it (measured on the R2 with a second client): a tone
//! shows as a non-zero DDS scale, a buffer of samples as the DAC channel's
//! data source switched to DMA (`0x2` in the core's channel control register
//! 7). Then nothing on the TX side is written, and the radio is said to be
//! transmitting for someone else.
//!
//! The decisions ([`assess`], [`parse_db`]) take plain data; [`read_state`]
//! and [`silence`] are the two sequences that talk to the radio.

use super::iiod::{Client, Error, Target};

const PHY: &str = "ad9361-phy";
const DDS: &str = "cf-ad9361-dds-core-lpc";

/// The chip's most attenuation, the lowest `hardwaregain` either TX channel
/// takes.
pub const MOST_ATTENUATION_DB: f64 = -89.75;

/// The DAC core's channel control register 7 for channel `n` (the data
/// source select), with bit 31 selecting the core's own registers.
fn source_register(n: u32) -> u32 {
    0x8000_0418 + 0x40 * n
}

/// The source value that means samples come from a DMA buffer.
const SOURCE_DMA: u32 = 0x2;

/// The transmitter as read at open.
#[derive(Clone, Debug, PartialEq)]
pub struct TxState {
    pub lo_powered_down: bool,
    /// TX1 and TX2 `hardwaregain`, dB.
    pub gains_db: [f64; 2],
    /// The eight DDS tones' `scale` (TX1 I/Q F1/F2, then TX2's).
    pub scales: [f64; 8],
    /// The data source of DAC channels 0..3 (TX1 I, Q, TX2 I, Q).
    pub sources: [u32; 4],
}

/// What was found.
#[derive(Clone, Debug, PartialEq)]
pub enum Finding {
    /// Already silent: LO powered down, both channels at most attenuation.
    Silent,
    /// Nobody transmitting, but not silent: the firmware's idle state, or
    /// anything else short of a tone or a buffer.
    Idle,
    /// Another program holds the transmitter: a tone set, or a buffer of
    /// samples feeding the DAC. `radiating` is whether its LO is running.
    Other { radiating: bool, what: String },
}

/// Decides what the transmitter is doing.
pub fn assess(s: &TxState) -> Finding {
    let tones = s.scales.iter().filter(|&&v| v > 0.0).count();
    let dma = s.sources.iter().filter(|&&v| v == SOURCE_DMA).count();
    if tones > 0 || dma > 0 {
        let what = match (tones, dma) {
            (0, _) => "a buffer of samples feeding the DAC".to_string(),
            (_, 0) => format!("{tones} DDS tone{} set", if tones == 1 { "" } else { "s" }),
            _ => "DDS tones and a buffer of samples".to_string(),
        };
        return Finding::Other {
            radiating: !s.lo_powered_down,
            what,
        };
    }
    let at_most = |g: f64| (g - MOST_ATTENUATION_DB).abs() < 0.01;
    if s.lo_powered_down && s.gains_db.iter().all(|&g| at_most(g)) {
        Finding::Silent
    } else {
        Finding::Idle
    }
}

/// A gain or scale as the driver writes it (`-10.000000 dB`, `0.099976`).
pub fn parse_db(s: &str) -> Option<f64> {
    s.split_whitespace().next()?.parse().ok()
}

fn number(c: &mut Client, target: Target<'_>, attr: &str) -> Result<f64, String> {
    let v = c
        .read(target, attr)
        .map_err(|e| format!("{target} {attr}: {e}"))?;
    parse_db(&v).ok_or_else(|| format!("{target} {attr} reads {v:?}"))
}

/// Reads the transmitter's state.
pub fn read_state(c: &mut Client) -> Result<TxState, String> {
    let pd = number(c, Target::Output(PHY, "altvoltage1"), "powerdown")?;
    let mut gains_db = [0.0; 2];
    for (i, g) in gains_db.iter_mut().enumerate() {
        *g = number(
            c,
            Target::Output(PHY, &format!("voltage{i}")),
            "hardwaregain",
        )?;
    }
    let mut scales = [0.0; 8];
    for (i, s) in scales.iter_mut().enumerate() {
        *s = number(c, Target::Output(DDS, &format!("altvoltage{i}")), "scale")?;
    }
    let mut sources = [0; 4];
    for (i, s) in sources.iter_mut().enumerate() {
        *s = c
            .register(DDS, source_register(i as u32))
            .map_err(|e: Error| format!("DAC channel {i}'s source: {e}"))?;
    }
    Ok(TxState {
        lo_powered_down: pd != 0.0,
        gains_db,
        scales,
        sources,
    })
}

/// Both channels to most attenuation, then the LO powered down; each read
/// back. Attenuation first, so the LO never runs unattenuated in between.
pub fn silence(c: &mut Client) -> Result<(), String> {
    for i in 0..2 {
        let ch = format!("voltage{i}");
        let t = Target::Output(PHY, &ch);
        c.write(t, "hardwaregain", &format!("{MOST_ATTENUATION_DB}"))
            .map_err(|e| format!("TX{} attenuation: {e}", i + 1))?;
        let back = number(c, t, "hardwaregain")?;
        if (back - MOST_ATTENUATION_DB).abs() > 0.01 {
            return Err(format!(
                "TX{} reads {back} dB after being set to {MOST_ATTENUATION_DB} dB",
                i + 1
            ));
        }
    }
    let lo = Target::Output(PHY, "altvoltage1");
    c.write(lo, "powerdown", "1")
        .map_err(|e| format!("TX LO powerdown: {e}"))?;
    if number(c, lo, "powerdown")? != 1.0 {
        return Err("the TX LO still runs after being powered down".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn firmware_default() -> TxState {
        // What the R2 reads after power-up.
        TxState {
            lo_powered_down: false,
            gains_db: [-10.0, -10.0],
            scales: [0.0; 8],
            sources: [0; 4],
        }
    }

    #[test]
    fn the_firmware_default_is_idle_and_wants_silencing() {
        assert_eq!(assess(&firmware_default()), Finding::Idle);
    }

    #[test]
    fn silent_is_the_lo_down_and_both_channels_at_most_attenuation() {
        let s = TxState {
            lo_powered_down: true,
            gains_db: [-89.75, -89.75],
            ..firmware_default()
        };
        assert_eq!(assess(&s), Finding::Silent);
        let one_left = TxState {
            gains_db: [-89.75, -10.0],
            ..s
        };
        assert_eq!(assess(&one_left), Finding::Idle);
    }

    /// The two ways another program's transmission showed on the R2.
    #[test]
    fn a_tone_or_a_buffer_is_another_program_transmitting() {
        let mut tone = firmware_default();
        tone.scales[0] = 0.099976;
        tone.scales[2] = 0.099976;
        assert_eq!(
            assess(&tone),
            Finding::Other {
                radiating: true,
                what: "2 DDS tones set".into()
            }
        );
        let mut buffer = firmware_default();
        buffer.sources = [SOURCE_DMA, SOURCE_DMA, 0, 0];
        buffer.lo_powered_down = true;
        assert_eq!(
            assess(&buffer),
            Finding::Other {
                radiating: false,
                what: "a buffer of samples feeding the DAC".into()
            }
        );
    }

    #[test]
    fn the_drivers_numbers_parse() {
        assert_eq!(parse_db("-10.000000 dB"), Some(-10.0));
        assert_eq!(parse_db("-89.750000 dB"), Some(-89.75));
        assert_eq!(parse_db("0.099976"), Some(0.099976));
        assert_eq!(parse_db("1"), Some(1.0));
        assert_eq!(parse_db(""), None);
        assert_eq!(parse_db("dB"), None);
    }

    #[test]
    fn the_source_registers_are_the_ones_read_on_the_bench() {
        assert_eq!(source_register(0), 0x8000_0418);
        assert_eq!(source_register(1), 0x8000_0458);
    }
}
