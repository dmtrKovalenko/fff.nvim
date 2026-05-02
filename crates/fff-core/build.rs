fn main() {
    // Opt-in cfg for the long-running randomized stress tests
    // used by tests/fuzz_git_watcher_stress.rs
    println!("cargo::rustc-check-cfg=cfg(stress)");

    // When the `zlob` feature is enabled (Zig-compiled C library):
    // On Windows MSVC, explicitly link the C runtime libraries.
    // Zig-compiled static libraries don't emit /DEFAULTLIB directives for the
    // MSVC CRT, so symbols like strcmp, memcpy etc. would be unresolved.
    if std::env::var("CARGO_FEATURE_ZLOB").is_ok() {
        if !zig_available() {
            panic!(
                "The `zlob` feature is enabled but Zig is not installed. \
                 Install Zig (https://ziglang.org/download/) or build without \
                 `--features zlob`."
            );
        }

        let target = std::env::var("TARGET").unwrap_or_default();
        if target.contains("windows") && target.contains("msvc") {
            println!("cargo:rustc-link-lib=msvcrt");
            println!("cargo:rustc-link-lib=ucrt");
            println!("cargo:rustc-link-lib=vcruntime");
        }
    } else if std::env::var("CARGO_PRIMARY_PACKAGE").is_ok() && zig_available() {
        // Hint: if Zig is available but the zlob feature wasn't enabled,
        // let the developer know they can get faster glob matching.
        // Only emit this hint when this crate is the primary package to
        // avoid noisy warnings for downstream consumers.
        println!(
            "cargo:warning=Zig detected but `zlob` feature is not enabled. \
             Build with `--features zlob` for faster glob matching."
        );
    }
}

/// Probe the system for a working Zig installation.
fn zig_available() -> bool {
    std::process::Command::new("zig")
        .arg("version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
