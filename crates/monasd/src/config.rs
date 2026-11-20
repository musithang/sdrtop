// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! monasd's block of the config file, `[monasd]`, beside the engine's, which
//! are Monas's and re-exported here so the whole file is read through one
//! module. The file is the terminal's too; monasd never writes `[monasd]`,
//! and writes the engine's blocks only.

pub use monas::config::*;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// `[monasd]`: how the engine runs with no terminal. Every field optional; a
/// command-line flag wins over the file.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Default)]
pub struct MonasdSettings {
    /// The address the API listens on, `host:port`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen: Option<String>,
    /// What a client off this machine must present. Required to listen
    /// anywhere but loopback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// The view to compute for, by its layout name, e.g. `net_survey`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Where the API's SigMF recordings are to go, once it records. Read
    /// today and used by nothing yet: no route records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recordings_dir: Option<PathBuf>,
}

impl MonasdSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// The config as monasd reads it: the engine's blocks and
/// `[monasd]`. It writes back only the engine's ([`EngineConfig::save`]):
/// `[monasd]` is the operator's, and no command changes it.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct MonasdConfig {
    #[serde(flatten)]
    pub engine: EngineConfig,
    #[serde(default, skip_serializing_if = "MonasdSettings::is_default")]
    pub monasd: MonasdSettings,
}

impl MonasdConfig {
    /// See [`load_or_default`].
    pub fn load_or_default(path: &Path) -> Self {
        load_or_default(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config with every block set, as sdrtop writes it, `[monasd]`
    /// included: what monasd reads must come back out byte for byte.
    const FULL: &str = include_str!("config/full.toml");

    fn full_copy(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("monasd-full-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, FULL).unwrap();
        path
    }

    /// **monasd reads its block and the engine's, and writes
    /// back only the engine's**: the display, the theme and the layouts it
    /// never read are not reset by its save.
    #[test]
    fn the_server_writes_back_the_full_config_unchanged() {
        let path = full_copy("monasd");
        let cfg = MonasdConfig::load_or_default(&path);
        assert_eq!(
            cfg.monasd,
            MonasdSettings {
                listen: Some("0.0.0.0:7373".into()),
                token: Some("s3cret".into()),
                mode: Some("net_survey".into()),
                recordings_dir: Some(PathBuf::from("/srv/rec")),
            }
        );
        assert_eq!(cfg.engine.tinysa.points, 900);
        cfg.engine.save(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), FULL);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// **`[monasd]` is monasd's block alone**: the struct holds it and only
    /// it, and the engine owns no block by that name.
    #[test]
    fn the_monasd_block_has_one_owner() {
        let src = include_str!("config.rs");
        let body = src
            .split_once("pub struct MonasdConfig {")
            .unwrap()
            .1
            .split_once("\n}")
            .unwrap()
            .0;
        let mut flattened = false;
        let mut blocks = vec![];
        for line in body.lines().map(str::trim) {
            if line == "#[serde(flatten)]" {
                flattened = true;
            } else if let Some((name, _)) =
                line.strip_prefix("pub ").and_then(|l| l.split_once(':'))
            {
                if !std::mem::take(&mut flattened) {
                    blocks.push(name);
                }
            }
        }
        assert_eq!(blocks, ["monasd"]);
        assert!(!ENGINE_BLOCKS.contains(&"monasd"));
    }
}
