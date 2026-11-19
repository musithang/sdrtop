// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

use std::path::{Path, PathBuf};

/// The repository's root when this is built from a checkout, where the
/// workspace's `target/` and `.git` are; the package's own directory when it
/// is built alone (from crates.io), as it always was.
fn root() -> PathBuf {
    let here = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));
    let workspace = here.ancestors().nth(2).filter(|up| {
        std::fs::read_to_string(up.join("Cargo.toml")).is_ok_and(|t| t.contains("[workspace]"))
    });
    workspace.map(Path::to_path_buf).unwrap_or(here)
}

/// Where the generated man page lands: `target/man` under [`root`].
///
/// A fixed path under `target/`, not `OUT_DIR`: `packaging/build-tarball.sh` has
/// to copy this file into the release tarball, and `OUT_DIR` carries a build
/// hash that nothing can predict.
fn man_dir() -> PathBuf {
    root().join("target/man")
}

fn main() {
    emit_version();
    generate_man_page();
}

/// Render `monasd.1` from the CLI definition.
///
/// `include!` rather than a shared crate: this is a binary-only package, so
/// `src/cli.rs` is not importable from a build script any other way. It is why
/// that file has to stay self-contained.
fn generate_man_page() {
    println!("cargo:rerun-if-changed=src/cli.rs");

    use clap::CommandFactory;
    mod cli {
        include!("src/cli.rs");
    }

    let cmd = cli::Cli::command()
        .name("monasd")
        .version(env!("CARGO_PKG_VERSION"));
    let mut page = Vec::new();
    if clap_mangen::Man::new(cmd).render(&mut page).is_err() {
        // A missing man page is a lintian warning, not a broken build.
        println!("cargo:warning=could not render the man page");
        return;
    }
    let dir = man_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        println!("cargo:warning=could not create {}", dir.display());
        return;
    }
    let _ = std::fs::write(dir.join("monasd.1"), page);
}

/// Emit `MONASD_VERSION`, the string `--version` prints: `0.4.1 (2ec9491)`.
///
/// The plain crate version stopped being enough the moment install.sh could
/// install from `main` as readily as from a release, because both would print
/// the same number and a bug report naming it would identify nothing.
fn emit_version() {
    let version = match commit() {
        Some(c) => format!("{} ({})", env!("CARGO_PKG_VERSION"), c),
        // Not fatal, and not rare: a tarball of the sources with no git and no
        // packaging step lands here. The plain version is what every release up
        // to 0.4.0 printed anyway.
        None => env!("CARGO_PKG_VERSION").to_string(),
    };
    println!("cargo:rustc-env=MONASD_VERSION={version}");
}

/// The short commit, from whichever of the three sources can answer.
///
/// They are tried in order of authority, not convenience.
fn commit() -> Option<String> {
    // 1. Set by packaging/build-tarball.sh, which resolves it on the host. The
    //    release container has no git in it, and bind-mounting `.git` would hit
    //    git's safe.directory check whenever the uid does not match.
    println!("cargo:rerun-if-env-changed=SDRTOP_COMMIT");
    if let Some(sha) = std::env::var("SDRTOP_COMMIT")
        .ok()
        .filter(|s| !s.is_empty())
    {
        return Some(sha);
    }

    // 2. A git checkout: a contributor's build, or `cargo install --git`.
    //    The commit moves without any tracked file changing, so cargo has to be
    //    told to look again, and watching `.git/HEAD` alone is not enough: that
    //    file holds `ref: refs/heads/main` and a commit does not touch it. The
    //    file it names is the one that moves. Watch both, so a new commit and a
    //    branch switch each rebuild. Neither path existing is fine, including
    //    the packed-refs case, where the worst outcome is a stale local build
    //    that still carries an honest `-dirty` once anything is edited.
    let git = root().join(".git");
    println!("cargo:rerun-if-changed={}", git.join("HEAD").display());
    if let Ok(head) = std::fs::read_to_string(git.join("HEAD")) {
        if let Some(git_ref) = head.trim().strip_prefix("ref: ") {
            println!("cargo:rerun-if-changed={}", git.join(git_ref).display());
        }
    }
    if let Some(sha) = git_commit() {
        return Some(sha);
    }

    // 3. A crate unpacked from crates.io, which has no `.git` but does carry
    //    the commit cargo recorded at publish time.
    vcs_info_commit()
}

fn git_commit() -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if sha.is_empty() {
        return None;
    }

    // A build from a tree with uncommitted changes is not the commit it names,
    // and saying which build this is was the entire point of printing one.
    let dirty = std::process::Command::new("git")
        .args(["diff", "--quiet", "HEAD"])
        .status()
        .map(|s| !s.success())
        .unwrap_or(false);

    Some(if dirty { format!("{sha}-dirty") } else { sha })
}

/// Read the commit out of `.cargo_vcs_info.json`, which cargo writes into every
/// published crate.
///
/// Searched as text rather than parsed: it is one field of a fixed shape, and
/// taking on a JSON build-dependency to read it would cost every user of
/// `cargo install monasd` a compile for one string.
fn vcs_info_commit() -> Option<String> {
    let text = std::fs::read_to_string(".cargo_vcs_info.json").ok()?;
    let after_key = &text[text.find("\"sha1\"")? + "\"sha1\"".len()..];
    let after_colon = after_key.trim_start().strip_prefix(':')?;
    let value = after_colon.trim_start().strip_prefix('"')?;
    let sha = &value[..value.find('"')?];
    // Shortened to match what `git rev-parse --short` gives, so the two sources
    // are indistinguishable in a bug report.
    Some(sha.chars().take(7).collect())
}
