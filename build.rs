/// Where the generated man page lands.
///
/// A fixed path under `target/`, not `OUT_DIR`: `cargo deb` has to name this file
/// in its asset list, and `OUT_DIR` carries a build hash that nothing can predict.
const MAN_DIR: &str = "target/man";

fn main() {
    generate_man_page();

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
