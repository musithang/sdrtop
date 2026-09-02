// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

/// Where the generated man page lands.
///
/// A fixed path under `target/`, not `OUT_DIR`: `packaging/build-tarball.sh` has
/// to copy this file into the release tarball, and `OUT_DIR` carries a build
/// hash that nothing can predict.
const MAN_DIR: &str = "target/man";

fn main() {
    emit_version();
    generate_man_page();

    // docs.rs builds every published crate and cannot install system packages,
    // so the probe below would panic there and leave a red build badge on the
    // crates.io page of a binary-only crate whose docs nobody reads. docs.rs
    // announces itself with DOCS_RS=1. Skipping the link directives costs
    // nothing there: `cargo doc` documents the crate, it never links the
    // binary.
    println!("cargo:rerun-if-env-changed=DOCS_RS");
    if std::env::var_os("DOCS_RS").is_some() {
        return;
    }

    // Requires libhackrf >= 2023.01.1 (hackrf_board_rev_read,
    // hackrf_usb_api_version_read). Many .pc files omit the version field so
    // atleast_version() fails even on a correct install; just probe and let
    // the linker error on missing symbols if the library is too old.
    //
    // Install: apt install libhackrf-dev  (Bookworm / Ubuntu 24.04+)
    // Older distros: build from source at
    // https://github.com/greatscottgadgets/hackrf
    if let Err(e) = pkg_config::probe_library("libhackrf") {
        panic!(
            "libhackrf not found ({}). \
             Install: apt install libhackrf-dev  \
             (requires Raspberry Pi OS Bookworm or Ubuntu 24.04+)",
            e
        );
    }

    // librtlsdr powers the RTL-SDR backend. Some distros ship the library
    // without a .pc file, so fall back to a bare link directive (and let the
    // linker error if it is genuinely missing) rather than failing the probe.
    if pkg_config::probe_library("librtlsdr").is_err() {
        println!("cargo:rustc-link-lib=rtlsdr");
    }
}

/// Render `sdrtop.1` from the CLI definition.
///
/// `include!` rather than a shared crate: this is a binary-only package, so
/// `src/cli.rs` is not importable from a build script any other way. It is why
/// that file has to stay self-contained.
fn generate_man_page() {
    // pkg-config emits `rerun-if-env-changed`, which switches off cargo's default
    // "re-run when any file changes" - so the man page's own input has to be
    // declared, or an edit to the flags would not regenerate it.
    println!("cargo:rerun-if-changed=src/cli.rs");

    use clap::CommandFactory;
    mod cli {
        include!("src/cli.rs");
    }

    let cmd = cli::Cli::command()
        .name("sdrtop")
        .version(env!("CARGO_PKG_VERSION"));
    let mut page = Vec::new();
    if clap_mangen::Man::new(cmd).render(&mut page).is_err() {
        // A missing man page is a lintian warning, not a broken build.
        println!("cargo:warning=could not render the man page");
        return;
    }
    if std::fs::create_dir_all(MAN_DIR).is_err() {
        println!("cargo:warning=could not create {MAN_DIR}");
        return;
    }
    let _ = std::fs::write(format!("{MAN_DIR}/sdrtop.1"), page);
}

/// Emit `SDRTOP_VERSION`, the string `--version` prints: `0.4.1 (2ec9491)`.
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
    println!("cargo:rustc-env=SDRTOP_VERSION={version}");
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
    println!("cargo:rerun-if-changed=.git/HEAD");
    if let Ok(head) = std::fs::read_to_string(".git/HEAD") {
        if let Some(git_ref) = head.trim().strip_prefix("ref: ") {
            println!("cargo:rerun-if-changed=.git/{git_ref}");
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
/// `cargo install sdrtop` a compile for one string.
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
