// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The C structs of SDRplay's API that sdrtop reads or writes, laid out as
//! the API's 3.15 headers lay them out.
//!
//! SDRplay's API hands its state over as structs the caller reads and writes
//! in place, so a field one offset off is not a crash but a value written
//! into the wrong field. The layout is therefore a checked fact, tied to the
//! API version it was checked against: [`super::ffi::CHECKED_VERSIONS`] lists
//! the versions whose headers produced exactly these offsets, and the backend
//! refuses any other version by name. The tests hold every offset sdrtop
//! touches twice, from one table: against the fixture library's own
//! declarations (in every test run), and against the installed headers
//! themselves, compiled by the C compiler (an ignored test, for a machine that
//! has them).
//!
//! The structs of the other RSPs (`Rsp2*`, `RspDuo*`, `RspDx*`) are here
//! whole because they sit between the fields the RSP1B uses: their sizes are
//! what puts the RSP1B's after them where the API looks.

use libc::{c_char, c_int, c_void};

/// The longest serial the API writes, NUL included (`SDRPLAY_MAX_SER_NO_LEN`).
pub const SERIAL_LEN: usize = 64;

/// How many radios `sdrplay_api_GetDevices` may describe at once
/// (`SDRPLAY_MAX_DEVICES`).
pub const MAX_DEVICES: usize = 16;

/// `sdrplay_api_DeviceT`: one radio as `sdrplay_api_GetDevices` lists it.
/// The two enums (`tuner`, `rspDuoMode`) are C `int`s.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DeviceT {
    pub ser_no: [c_char; SERIAL_LEN],
    /// Which RSP: 6 for the RSP1B (`SDRPLAY_RSP1B_ID`).
    pub hw_ver: u8,
    pub tuner: c_int,
    pub rsp_duo_mode: c_int,
    /// Whether the entry describes a radio this process may select.
    pub valid: u8,
    pub rsp_duo_sample_freq: f64,
    /// The handle, filled in by `sdrplay_api_SelectDevice`.
    pub dev: *mut c_void,
}

impl Default for DeviceT {
    fn default() -> Self {
        Self {
            ser_no: [0; SERIAL_LEN],
            hw_ver: 0,
            tuner: 0,
            rsp_duo_mode: 0,
            valid: 0,
            rsp_duo_sample_freq: 0.0,
            dev: std::ptr::null_mut(),
        }
    }
}

/// `sdrplay_api_FsFreqT`: the ADC's rate.
#[repr(C)]
pub struct FsFreqT {
    pub fs_hz: f64,
    pub sync_update: u8,
    pub re_cal: u8,
}

/// `sdrplay_api_SyncUpdateT`.
#[repr(C)]
pub struct SyncUpdateT {
    pub sample_num: u32,
    pub period: u32,
}

/// `sdrplay_api_ResetFlagsT`.
#[repr(C)]
pub struct ResetFlagsT {
    pub reset_gain_update: u8,
    pub reset_rf_update: u8,
    pub reset_fs_update: u8,
}

/// `sdrplay_api_Rsp1aParamsT`, which the RSP1B uses: the two notches.
#[repr(C)]
pub struct Rsp1aParamsT {
    /// The AM/FM notch (one switch for both, measured).
    pub rf_notch_enable: u8,
    pub rf_dab_notch_enable: u8,
}

/// `sdrplay_api_Rsp2ParamsT`.
#[repr(C)]
pub struct Rsp2ParamsT {
    pub ext_ref_output_en: u8,
}

/// `sdrplay_api_RspDuoParamsT`.
#[repr(C)]
pub struct RspDuoParamsT {
    pub ext_ref_output_en: c_int,
}

/// `sdrplay_api_RspDxParamsT`.
#[repr(C)]
pub struct RspDxParamsT {
    pub hdr_enable: u8,
    pub bias_t_enable: u8,
    pub antenna_sel: c_int,
    pub rf_notch_enable: u8,
    pub rf_dab_notch_enable: u8,
}

/// `sdrplay_api_DevParamsT`: what is not per tuner.
#[repr(C)]
pub struct DevParamsT {
    pub ppm: f64,
    pub fs_freq: FsFreqT,
    pub sync_update: SyncUpdateT,
    pub reset_flags: ResetFlagsT,
    /// `sdrplay_api_TransferModeT`: 0 isochronous (the default), 1 bulk.
    pub mode: c_int,
    /// Called an output; the API never fills it (measured: 0 after `Init`
    /// and after every rate change).
    pub samples_per_pkt: u32,
    pub rsp1a_params: Rsp1aParamsT,
    pub rsp2_params: Rsp2ParamsT,
    pub rsp_duo_params: RspDuoParamsT,
    pub rsp_dx_params: RspDxParamsT,
}

/// `sdrplay_api_GainValuesT`: SDRplay's model of the chain, an output.
#[repr(C)]
pub struct GainValuesT {
    pub curr: f32,
    pub max: f32,
    pub min: f32,
}

/// `sdrplay_api_GainT`.
#[repr(C)]
pub struct GainT {
    /// The IF gain reduction, dB (20..59 with `min_gr` normal).
    pub gr_db: c_int,
    pub lna_state: u8,
    pub sync_update: u8,
    /// `sdrplay_api_MinGainReductionT`: 20 normal, 0 extended.
    pub min_gr: c_int,
    pub gain_vals: GainValuesT,
}

/// `sdrplay_api_RfFreqT`.
#[repr(C)]
pub struct RfFreqT {
    pub rf_hz: f64,
    pub sync_update: u8,
}

/// `sdrplay_api_DcOffsetTunerT`.
#[repr(C)]
pub struct DcOffsetTunerT {
    pub dc_cal: u8,
    pub speed_up: u8,
    pub track_time: c_int,
    pub refresh_rate_time: c_int,
}

/// `sdrplay_api_TunerParamsT`. The three enums are C `int`s holding kHz
/// (`bw_type`, `if_type`) or a mode (`lo_mode`, 1 automatic).
#[repr(C)]
pub struct TunerParamsT {
    pub bw_type: c_int,
    pub if_type: c_int,
    pub lo_mode: c_int,
    pub gain: GainT,
    pub rf_freq: RfFreqT,
    pub dc_offset_tuner: DcOffsetTunerT,
}

/// `sdrplay_api_DcOffsetT`: the API's DC and IQ corrections.
#[repr(C)]
pub struct DcOffsetT {
    pub dc_enable: u8,
    pub iq_enable: u8,
}

/// `sdrplay_api_DecimationT`.
#[repr(C)]
pub struct DecimationT {
    pub enable: u8,
    pub decimation_factor: u8,
    pub wide_band_signal: u8,
}

/// `sdrplay_api_AgcT`. `enable` is `sdrplay_api_AgcControlT`: 0 off, 1
/// 100 Hz, 2 50 Hz (the API's default), 3 5 Hz.
#[repr(C)]
pub struct AgcT {
    pub enable: c_int,
    pub set_point_dbfs: c_int,
    pub attack_ms: u16,
    pub decay_ms: u16,
    pub decay_delay_ms: u16,
    pub decay_threshold_db: u16,
    pub sync_update: c_int,
}

/// `sdrplay_api_ControlParamsT`.
#[repr(C)]
pub struct ControlParamsT {
    pub dc_offset: DcOffsetT,
    pub decimation: DecimationT,
    pub agc: AgcT,
    pub adsb_mode: c_int,
}

/// `sdrplay_api_Rsp1aTunerParamsT`, which the RSP1B uses.
#[repr(C)]
pub struct Rsp1aTunerParamsT {
    pub bias_t_enable: u8,
}

/// `sdrplay_api_Rsp2TunerParamsT`.
#[repr(C)]
pub struct Rsp2TunerParamsT {
    pub bias_t_enable: u8,
    pub am_port_sel: c_int,
    pub antenna_sel: c_int,
    pub rf_notch_enable: u8,
}

/// `sdrplay_api_RspDuo_ResetSlaveFlagsT`.
#[repr(C)]
pub struct RspDuoResetSlaveFlagsT {
    pub reset_gain_update: u8,
    pub reset_rf_update: u8,
}

/// `sdrplay_api_RspDuoTunerParamsT`.
#[repr(C)]
pub struct RspDuoTunerParamsT {
    pub bias_t_enable: u8,
    pub tuner1_am_port_sel: c_int,
    pub tuner1_am_notch_enable: u8,
    pub rf_notch_enable: u8,
    pub rf_dab_notch_enable: u8,
    pub reset_slave_flags: RspDuoResetSlaveFlagsT,
}

/// `sdrplay_api_RspDxTunerParamsT`.
#[repr(C)]
pub struct RspDxTunerParamsT {
    pub hdr_bw: c_int,
}

/// `sdrplay_api_RxChannelParamsT`: one tuner's parameters.
#[repr(C)]
pub struct RxChannelParamsT {
    pub tuner_params: TunerParamsT,
    pub ctrl_params: ControlParamsT,
    pub rsp1a_tuner_params: Rsp1aTunerParamsT,
    pub rsp2_tuner_params: Rsp2TunerParamsT,
    pub rsp_duo_tuner_params: RspDuoTunerParamsT,
    pub rsp_dx_tuner_params: RspDxTunerParamsT,
}

/// `sdrplay_api_DeviceParamsT`: what `sdrplay_api_GetDeviceParams` points
/// at, owned by the API for as long as the radio is selected.
#[repr(C)]
pub struct DeviceParamsT {
    pub dev_params: *mut DevParamsT,
    pub rx_channel_a: *mut RxChannelParamsT,
    pub rx_channel_b: *mut RxChannelParamsT,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// Every size and offset sdrtop relies on: the C expression that reads it,
    /// and ours.
    fn entries() -> Vec<(&'static str, usize)> {
        macro_rules! s {
            ($c:literal, $t:ty) => {
                (concat!("sizeof(", $c, ")"), size_of::<$t>())
            };
        }
        macro_rules! o {
            ($c:literal, $cf:literal, $t:ty, $f:ident) => {
                (concat!("offsetof(", $c, ", ", $cf, ")"), offset_of!($t, $f))
            };
        }
        vec![
            s!("sdrplay_api_DeviceT", DeviceT),
            o!("sdrplay_api_DeviceT", "hwVer", DeviceT, hw_ver),
            o!("sdrplay_api_DeviceT", "tuner", DeviceT, tuner),
            o!("sdrplay_api_DeviceT", "rspDuoMode", DeviceT, rsp_duo_mode),
            o!("sdrplay_api_DeviceT", "valid", DeviceT, valid),
            o!(
                "sdrplay_api_DeviceT",
                "rspDuoSampleFreq",
                DeviceT,
                rsp_duo_sample_freq
            ),
            o!("sdrplay_api_DeviceT", "dev", DeviceT, dev),
            s!("sdrplay_api_DeviceParamsT", DeviceParamsT),
            o!(
                "sdrplay_api_DeviceParamsT",
                "rxChannelA",
                DeviceParamsT,
                rx_channel_a
            ),
            o!(
                "sdrplay_api_DeviceParamsT",
                "rxChannelB",
                DeviceParamsT,
                rx_channel_b
            ),
            s!("sdrplay_api_DevParamsT", DevParamsT),
            o!("sdrplay_api_DevParamsT", "fsFreq", DevParamsT, fs_freq),
            o!(
                "sdrplay_api_DevParamsT",
                "syncUpdate",
                DevParamsT,
                sync_update
            ),
            o!(
                "sdrplay_api_DevParamsT",
                "resetFlags",
                DevParamsT,
                reset_flags
            ),
            o!("sdrplay_api_DevParamsT", "mode", DevParamsT, mode),
            o!(
                "sdrplay_api_DevParamsT",
                "samplesPerPkt",
                DevParamsT,
                samples_per_pkt
            ),
            o!(
                "sdrplay_api_DevParamsT",
                "rsp1aParams",
                DevParamsT,
                rsp1a_params
            ),
            o!(
                "sdrplay_api_DevParamsT",
                "rsp2Params",
                DevParamsT,
                rsp2_params
            ),
            o!(
                "sdrplay_api_DevParamsT",
                "rspDuoParams",
                DevParamsT,
                rsp_duo_params
            ),
            o!(
                "sdrplay_api_DevParamsT",
                "rspDxParams",
                DevParamsT,
                rsp_dx_params
            ),
            s!("sdrplay_api_FsFreqT", FsFreqT),
            s!("sdrplay_api_Rsp1aParamsT", Rsp1aParamsT),
            o!(
                "sdrplay_api_Rsp1aParamsT",
                "rfDabNotchEnable",
                Rsp1aParamsT,
                rf_dab_notch_enable
            ),
            s!("sdrplay_api_RspDxParamsT", RspDxParamsT),
            s!("sdrplay_api_RxChannelParamsT", RxChannelParamsT),
            o!(
                "sdrplay_api_RxChannelParamsT",
                "ctrlParams",
                RxChannelParamsT,
                ctrl_params
            ),
            o!(
                "sdrplay_api_RxChannelParamsT",
                "rsp1aTunerParams",
                RxChannelParamsT,
                rsp1a_tuner_params
            ),
            o!(
                "sdrplay_api_RxChannelParamsT",
                "rsp2TunerParams",
                RxChannelParamsT,
                rsp2_tuner_params
            ),
            o!(
                "sdrplay_api_RxChannelParamsT",
                "rspDuoTunerParams",
                RxChannelParamsT,
                rsp_duo_tuner_params
            ),
            o!(
                "sdrplay_api_RxChannelParamsT",
                "rspDxTunerParams",
                RxChannelParamsT,
                rsp_dx_tuner_params
            ),
            s!("sdrplay_api_TunerParamsT", TunerParamsT),
            o!("sdrplay_api_TunerParamsT", "ifType", TunerParamsT, if_type),
            o!("sdrplay_api_TunerParamsT", "loMode", TunerParamsT, lo_mode),
            o!("sdrplay_api_TunerParamsT", "gain", TunerParamsT, gain),
            o!("sdrplay_api_TunerParamsT", "rfFreq", TunerParamsT, rf_freq),
            o!(
                "sdrplay_api_TunerParamsT",
                "dcOffsetTuner",
                TunerParamsT,
                dc_offset_tuner
            ),
            s!("sdrplay_api_GainT", GainT),
            o!("sdrplay_api_GainT", "LNAstate", GainT, lna_state),
            o!("sdrplay_api_GainT", "minGr", GainT, min_gr),
            o!("sdrplay_api_GainT", "gainVals", GainT, gain_vals),
            s!("sdrplay_api_RfFreqT", RfFreqT),
            s!("sdrplay_api_DcOffsetTunerT", DcOffsetTunerT),
            s!("sdrplay_api_ControlParamsT", ControlParamsT),
            o!(
                "sdrplay_api_ControlParamsT",
                "decimation",
                ControlParamsT,
                decimation
            ),
            o!("sdrplay_api_ControlParamsT", "agc", ControlParamsT, agc),
            o!(
                "sdrplay_api_ControlParamsT",
                "adsbMode",
                ControlParamsT,
                adsb_mode
            ),
            s!("sdrplay_api_AgcT", AgcT),
            o!("sdrplay_api_AgcT", "setPoint_dBfs", AgcT, set_point_dbfs),
            o!("sdrplay_api_AgcT", "syncUpdate", AgcT, sync_update),
            s!("sdrplay_api_Rsp2TunerParamsT", Rsp2TunerParamsT),
            s!("sdrplay_api_RspDuoTunerParamsT", RspDuoTunerParamsT),
            s!("sdrplay_api_RspDxTunerParamsT", RspDxTunerParamsT),
        ]
    }

    /// Compiles a probe that prints every entry's C figure, with `include`
    /// as the header, and returns the figures with what the header says the
    /// API's version is.
    fn probe(include: &str, extra_args: &[&str]) -> (String, Vec<usize>) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sdrtop-sdrplay-probe-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("probe.c");
        let binary = dir.join("probe");
        let mut c = format!(
            "#include <stdio.h>\n#include <stddef.h>\n#include {include}\nint main(void) {{\n    printf(\"%.2f\\n\", (double)SDRPLAY_API_VERSION);\n"
        );
        for (expr, _) in entries() {
            c.push_str(&format!("    printf(\"%zu\\n\", {expr});\n"));
        }
        c.push_str("    return 0;\n}\n");
        std::fs::write(&source, c).unwrap();
        let built = std::process::Command::new("cc")
            .args(["-std=c11", "-Wall", "-Werror"])
            .args(extra_args)
            .arg("-o")
            .arg(&binary)
            .arg(&source)
            .output()
            .expect("a C compiler");
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let out = std::process::Command::new(&binary).output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let text = String::from_utf8(out.stdout).unwrap();
        let mut lines = text.lines();
        let version = lines.next().unwrap().to_string();
        (version, lines.map(|l| l.parse().unwrap()).collect())
    }

    fn ours() -> Vec<usize> {
        entries().into_iter().map(|(_, n)| n).collect()
    }

    /// The fixture library's declarations of the API's structs and ours lay
    /// every field sdrtop touches out alike.
    #[test]
    fn the_structs_match_the_fixtures_declarations() {
        let header = concat!(env!("CARGO_MANIFEST_DIR"), "/src/hardware/native/fixtures");
        let (version, c) = probe("\"sdrplay.h\"", &["-I", header]);
        assert_eq!(version, "3.15");
        assert_eq!(c, ours());
    }

    /// Against the API's own headers, compiled: the version they declare is
    /// one sdrtop checked, and every size and offset in the table is theirs.
    /// Needs SDRplay's API installed (its headers under the compiler's include
    /// path), which no package manager provides, so it is never run by CI.
    #[test]
    #[ignore = "requires SDRplay's API headers (sdrplay_api.h) installed"]
    fn the_installed_headers_lay_the_structs_out_as_we_do() {
        let (version, c) = probe("<sdrplay_api.h>", &[]);
        assert!(
            super::super::ffi::CHECKED_VERSIONS.contains(&version.as_str()),
            "the installed headers are {version}, not a version sdrtop checked"
        );
        assert_eq!(c, ours());
    }
}
