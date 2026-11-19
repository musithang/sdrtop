// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Where a file goes, what it is called, and what happens when it is there.
//!
//! The other reusable half, with [`super::provenance`]. A later IQ-sample export
//! writes something completely different and asks exactly these questions.
//!
//! **Every failure names the path, and none of them loses the data silently.**
//! An export that failed quietly is worse than one that never ran: the user
//! believes they have the file. So each way this can go wrong returns a sentence
//! with the path in it, for the log to show.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Where exports go unless the user says otherwise.
///
/// `~/.local/share/sdrtop/`, the XDG data directory, because an export is data
/// the user keeps rather than configuration - the config and the log live under
/// `~/.config/sdrtop/` and these are a different kind of file. Falls back to the
/// working directory on a machine with no `HOME`, which is the only other place
/// that is certainly writable.
pub fn default_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .map(|d| d.join("sdrtop"))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `net-census-20260905-182241.csv`.
///
/// **The name carries the same instant the header does**, from the same seconds,
/// so a file cannot be called one thing and say another inside. The seconds are
/// in the name because two exports a minute apart must not collide, and a
/// collision is refused rather than resolved - see [`create`].
pub fn file_name(stem: &str, unix_secs: i64) -> String {
    format!("{}.csv", base_name(stem, unix_secs))
}

/// `iq-20260927-101500`: the name without its extension, for an export that
/// is more than one file (an IQ recording's data and its metadata).
pub fn base_name(stem: &str, unix_secs: i64) -> String {
    let t = super::provenance::iso8601(unix_secs);
    let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    let (date, time) = digits.split_at(8.min(digits.len()));
    format!("{stem}-{date}-{time}")
}

/// Bytes an unprivileged writer can still put in `dir`.
///
/// Asked before an IQ recording starts, because a limit the disk cannot hold
/// is better refused at the first byte than discovered at the last one.
pub fn free_bytes(dir: &Path) -> Result<u64, String> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes())
        .map_err(|_| format!("{} is not a path the system can open", dir.display()))?;
    let mut st = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: `c` is a valid NUL-terminated path and `st` is sized for the
    // struct statvfs writes; it is read only after the call reports success.
    let rc = unsafe { libc::statvfs(c.as_ptr(), st.as_mut_ptr()) };
    if rc != 0 {
        return Err(format!(
            "cannot ask how much room {} has: {}",
            dir.display(),
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: statvfs returned 0, so it filled the struct.
    let st = unsafe { st.assume_init() };
    // Both are 32 bits wide on 32-bit Raspberry Pi OS and 64 elsewhere.
    #[allow(clippy::unnecessary_cast)]
    Ok((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
}

/// Create the file, refusing to overwrite anything already there.
///
/// **Refusing rather than finding a free name.** An export is a deliberate act
/// and the user chose the moment; silently writing to a different name than the
/// one reported would be the same class of quiet as overwriting. The name
/// carries seconds, so a collision means two exports in one second, which is a
/// mistake worth stopping.
pub fn create(path: &Path) -> Result<File, String> {
    // Named before the open, because "no such file or directory" from the OS
    // does not say *which* of the two was missing and the user needs to know
    // whether to make a directory or fix a typo.
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        if !parent.is_dir() {
            return Err(format!("no directory to export into: {}", parent.display()));
        }
    }
    File::create_new(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => {
            format!(
                "{} exists already, refusing to overwrite it",
                path.display()
            )
        }
        _ => format!("cannot create {}: {e}", path.display()),
    })
}

/// Write a whole export, header and body, to `path`.
pub fn write(path: &Path, lines: &[String]) -> Result<(), String> {
    let mut file = create(path)?;
    for line in lines {
        writeln!(file, "{line}").map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    file.flush()
        .map_err(|e| format!("cannot finish writing {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sdrtop-export-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The name and the header agree, because they come from the same seconds.
    #[test]
    fn the_file_name_carries_the_instant_the_header_does() {
        assert_eq!(
            file_name("net-census", 1_788_632_561),
            "net-census-20260905-182241.csv"
        );
        // The same instant, both ways round.
        let header = super::super::provenance::iso8601(1_788_632_561);
        assert!(header.starts_with("2026-09-05"));
        assert!(file_name("x", 1_788_632_561).contains("20260905"));
    }

    /// A directory that exists has some room, and one that does not is named.
    #[test]
    fn free_space_is_asked_of_the_directory_itself() {
        let dir = scratch("free");
        assert!(free_bytes(&dir).unwrap() > 0);
        let gone = dir.join("nope");
        assert!(free_bytes(&gone).unwrap_err().contains("nope"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_written_file_holds_what_it_was_given() {
        let dir = scratch("write");
        let path = dir.join("out.csv");
        write(&path, &["# header".to_string(), "a,b".to_string()]).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# header\na,b\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A name that already exists is refused, and the file is untouched.**
    #[test]
    fn an_existing_file_is_never_overwritten() {
        let dir = scratch("exists");
        let path = dir.join("out.csv");
        std::fs::write(&path, "the original").unwrap();

        let err = write(&path, &["the replacement".to_string()]).unwrap_err();
        assert!(err.contains(&path.display().to_string()), "{err}");
        assert!(err.to_lowercase().contains("exist"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "the original",
            "the export overwrote a file it said it refused to"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A directory that is not there is named, not created.
    ///
    /// Creating it would be the friendly thing and the wrong one: a typo in a
    /// path would silently scatter files into directories nobody meant to make.
    #[test]
    fn a_missing_directory_is_named_rather_than_created() {
        let dir = scratch("missing");
        let path = dir.join("no-such-dir").join("out.csv");
        let err = write(&path, &["x".to_string()]).unwrap_err();
        assert!(err.contains("no-such-dir"), "{err}");
        assert!(!path.exists(), "the file was written anyway");
        assert!(
            !path.parent().unwrap().exists(),
            "the directory was created behind the user's back"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A path that cannot be written names itself.
    ///
    /// The parent is a *file*, which is an error every OS gives deterministically
    /// and which needs no permission games to provoke.
    #[test]
    fn a_path_that_cannot_be_written_names_itself() {
        let dir = scratch("notdir");
        let blocker = dir.join("a-file");
        std::fs::write(&blocker, "not a directory").unwrap();

        let path = blocker.join("out.csv");
        let err = write(&path, &["x".to_string()]).unwrap_err();
        assert!(err.contains("a-file"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// And a directory that exists but refuses writes.
    ///
    /// Skipped under a uid that ignores the permission bits, with the reason
    /// stated: a test that quietly passes because it could not run is worse than
    /// no test.
    #[test]
    fn a_read_only_directory_names_itself() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("readonly");
        let locked = dir.join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();

        let path = locked.join("out.csv");
        let result = write(&path, &["x".to_string()]);
        if std::fs::File::create(locked.join("probe")).is_ok() {
            eprintln!("skipped: this uid writes through a read-only directory");
        } else {
            let err = result.unwrap_err();
            assert!(err.contains(&path.display().to_string()), "{err}");
        }
        let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
