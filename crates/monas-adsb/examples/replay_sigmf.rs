// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Offline characterization of 1090 MHz Airspy and RTL-SDR IQ captures.

use std::error::Error;
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::Instant;

use monas_adsb::{FrameOutcome, Receiver, StreamEvent};
use monas_dsp::sample::{decode, SampleFormat, SampleGeometry};
use serde_json::Value;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const BLOCK_PAIRS: usize = 16_384;
const ADSB_HZ: u64 = 1_090_000_000;

#[derive(Debug, PartialEq, Eq)]
struct Segment {
    file_start: u64,
    stream_start: u64,
}

struct Capture {
    rate: f64,
    geometry: SampleGeometry,
    pairs: u64,
    segments: Vec<Segment>,
}

impl Capture {
    fn from_metadata(meta: &Value, bytes: u64) -> Result<Self> {
        let global = &meta["global"];
        let format = match global["core:datatype"].as_str() {
            Some("ci16_le") => SampleFormat::Int16,
            Some("cu8") => SampleFormat::Uint8,
            _ => return Err("replay requires ci16_le or cu8".into()),
        };
        let full_scale = match (format, global.get("sdrtop:full_scale")) {
            (SampleFormat::Uint8, None) => 128.0,
            (_, Some(value)) => value
                .as_f64()
                .map(|value| value as f32)
                .filter(|scale| scale.is_finite() && *scale >= 1.0)
                .ok_or("invalid sdrtop:full_scale")?,
            _ => return Err("missing sdrtop:full_scale".into()),
        };
        let geometry = SampleGeometry {
            format,
            full_scale,
            converter_bits: None,
        };
        let stride = geometry.bytes_per_pair() as u64;
        if !bytes.is_multiple_of(stride) {
            return Err("IQ data ends with an incomplete pair".into());
        }
        let pairs = bytes / stride;
        if global.get("sdrtop:shortfall").is_some()
            || global["sdrtop:radio_drops"].as_u64().unwrap_or(0) > 0
        {
            return Err("capture contains unlocated sample loss".into());
        }
        if pairs == 0
            || global["sdrtop:pairs_written"]
                .as_u64()
                .is_some_and(|n| n != pairs)
        {
            return Err("capture size does not match metadata".into());
        }
        let rate = global["core:sample_rate"]
            .as_f64()
            .filter(|rate| rate.is_finite() && *rate > 0.0)
            .ok_or("missing or invalid sample rate")?;
        let captures = meta["captures"].as_array().ok_or("missing captures")?;
        let mut segments: Vec<Segment> = Vec::with_capacity(captures.len());
        for capture in captures {
            if capture["core:frequency"].as_u64() != Some(ADSB_HZ) {
                return Err("every segment must be tuned to 1090 MHz".into());
            }
            let file_start = capture["core:sample_start"]
                .as_u64()
                .ok_or("missing sample_start")?;
            let stream_start = capture["core:global_index"]
                .as_u64()
                .ok_or("missing global_index")?;
            if file_start >= pairs {
                return Err("segment begins outside the recording".into());
            }
            if let Some(previous) = segments.last() {
                if file_start <= previous.file_start
                    || stream_start
                        .checked_sub(previous.stream_start)
                        .is_none_or(|advance| advance < file_start - previous.file_start)
                {
                    return Err("capture segments overlap or are out of order".into());
                }
            } else if file_start != 0 {
                return Err("first segment must begin at file pair 0".into());
            }
            segments.push(Segment {
                file_start,
                stream_start,
            });
        }
        let last = segments.last().ok_or("missing capture segment")?;
        if last
            .stream_start
            .checked_add(pairs - last.file_start)
            .is_none()
        {
            return Err("stream position exceeds the sample index range".into());
        }
        Ok(Self {
            rate,
            geometry,
            pairs,
            segments,
        })
    }

    fn blocks(
        &self,
        mut reader: impl Read,
        mut visit: impl FnMut(u64, &[u8]) -> Result<()>,
    ) -> Result<()> {
        let stride = self.geometry.bytes_per_pair();
        let mut buffer = vec![0; BLOCK_PAIRS * stride];
        for (index, segment) in self.segments.iter().enumerate() {
            let end = self
                .segments
                .get(index + 1)
                .map_or(self.pairs, |next| next.file_start);
            let mut cursor = segment.file_start;
            while cursor < end {
                let pairs = (end - cursor).min(BLOCK_PAIRS as u64) as usize;
                let bytes = &mut buffer[..pairs * stride];
                reader.read_exact(bytes)?;
                visit(segment.stream_start + (cursor - segment.file_start), bytes)?;
                cursor += pairs as u64;
            }
        }
        Ok(())
    }
}

fn mean_dc(capture: &Capture, data: &Path) -> Result<(f32, f32)> {
    let (mut i_sum, mut q_sum) = (0.0, 0.0);
    let mut iq = Vec::with_capacity(BLOCK_PAIRS);
    capture.blocks(BufReader::new(File::open(data)?), |_, bytes| {
        decode(bytes, capture.geometry, BLOCK_PAIRS, &mut iq);
        for sample in &iq {
            i_sum += f64::from(sample.re);
            q_sum += f64::from(sample.im);
        }
        Ok(())
    })?;
    Ok((
        (i_sum / capture.pairs as f64) as f32,
        (q_sum / capture.pairs as f64) as f32,
    ))
}

fn print_accepted(event: StreamEvent) {
    if let FrameOutcome::Accepted(frame) = event.outcome {
        print!(
            "{}\t{:.3}\t{:.6}\t",
            event.first_pair, event.preamble.start_sample, event.preamble.contrast
        );
        for byte in frame.bytes() {
            print!("{byte:02X}");
        }
        println!();
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let usage = "usage: replay_sigmf CAPTURE.sigmf-meta [--mean-dc]";
    let meta = PathBuf::from(args.next().ok_or(usage)?);
    let remove_mean = match args.next() {
        None => false,
        Some(flag) if flag == "--mean-dc" && args.next().is_none() => true,
        _ => return Err(usage.into()),
    };
    if meta.extension().is_none_or(|ext| ext != "sigmf-meta") {
        return Err(usage.into());
    }
    let data = meta.with_extension("sigmf-data");
    let bytes = fs::metadata(&data)?.len();
    let metadata = serde_json::from_reader(BufReader::new(File::open(meta)?))?;
    let capture = Capture::from_metadata(&metadata, bytes)?;
    let (dc_i, dc_q) = if remove_mean {
        mean_dc(&capture, &data)?
    } else {
        (0.0, 0.0)
    };
    let mut receiver = Receiver::new(capture.rate).map_err(|error| format!("{error:?}"))?;
    let mut power = Vec::with_capacity(BLOCK_PAIRS);
    let mut iq = Vec::with_capacity(BLOCK_PAIRS);
    println!("first_pair\tphase\tcontrast\tdf17_hex");
    let start = Instant::now();
    capture.blocks(BufReader::new(File::open(data)?), |first_pair, bytes| {
        decode(bytes, capture.geometry, BLOCK_PAIRS, &mut iq);
        power.clear();
        power.extend(
            iq.iter()
                .map(|sample| (sample.re - dc_i).powi(2) + (sample.im - dc_q).powi(2)),
        );
        receiver
            .push_power(first_pair, &power, print_accepted)
            .map_err(|error| format!("{error:?}"))?;
        Ok(())
    })?;
    receiver.finish(print_accepted);
    eprintln!(
        "{} pairs at {} pairs/s in {} segments, DC subtracted ({dc_i:.8}, {dc_q:.8}), {:.3} s replay",
        capture.pairs, capture.rate, capture.segments.len(), start.elapsed().as_secs_f64()
    );
    eprintln!("{:?}", receiver.stats());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn metadata() -> Value {
        json!({
            "global": {
                "core:datatype": "ci16_le",
                "core:sample_rate": 2_500_000,
                "sdrtop:full_scale": 16_384,
                "sdrtop:pairs_written": 3
            },
            "captures": [
                {"core:sample_start": 0, "core:global_index": 0, "core:frequency": ADSB_HZ},
                {"core:sample_start": 2, "core:global_index": 9, "core:frequency": ADSB_HZ}
            ]
        })
    }

    #[test]
    fn feeds_file_pairs_at_their_original_stream_indices() {
        let capture = Capture::from_metadata(&metadata(), 12).unwrap();
        let mut seen = Vec::new();
        let bytes = [0, 0x40, 0, 0xC0, 0, 0x20, 0, 0, 0, 0xE0, 0, 0x20];
        capture
            .blocks(bytes.as_slice(), |pair, samples| {
                let mut iq = Vec::new();
                decode(samples, capture.geometry, BLOCK_PAIRS, &mut iq);
                let iq_values: Vec<_> = iq.iter().map(|sample| (sample.re, sample.im)).collect();
                seen.push((pair, iq_values));
                Ok(())
            })
            .unwrap();
        assert_eq!(
            seen,
            vec![(0, vec![(1.0, -1.0), (0.5, 0.0)]), (9, vec![(-0.5, 0.5)])]
        );
    }

    #[test]
    fn rejects_unlocated_losses_and_overlapping_indices() {
        let mut capture = metadata();
        capture["global"]["sdrtop:radio_drops"] = json!(1);
        assert!(Capture::from_metadata(&capture, 12).is_err());
        let mut capture = metadata();
        capture["captures"][1]["core:global_index"] = json!(1);
        assert!(Capture::from_metadata(&capture, 12).is_err());
        assert!(Capture::from_metadata(&metadata(), 8).is_err());
    }

    #[test]
    fn replays_unsigned_rtl_iq_with_its_own_stride_and_zero() {
        let mut meta = metadata();
        meta["global"]["core:datatype"] = json!("cu8");
        meta["global"]
            .as_object_mut()
            .unwrap()
            .remove("sdrtop:full_scale");
        let capture = Capture::from_metadata(&meta, 6).unwrap();
        assert!(Capture::from_metadata(&meta, 5).is_err());
        let mut seen = Vec::new();
        let mut iq = Vec::new();
        capture
            .blocks([0, 255, 127, 128, 255, 0].as_slice(), |pair, bytes| {
                decode(bytes, capture.geometry, BLOCK_PAIRS, &mut iq);
                seen.push((
                    pair,
                    iq.iter()
                        .map(|sample| (sample.re, sample.im))
                        .collect::<Vec<_>>(),
                ));
                Ok(())
            })
            .unwrap();
        assert_eq!(seen[0], (0, vec![(-1.0, 1.0), (-0.5 / 127.5, 0.5 / 127.5)]));
        assert_eq!(seen[1], (9, vec![(1.0, -1.0)]));
    }
}
