fn main() {
    // When the `zlob` feature is enabled (Zig-compiled C library):
    // On Windows MSVC, explicitly link the C runtime libraries.
    // Zig-compiled static libraries don't emit /DEFAULTLIB directives for the
    // MSVC CRT, so symbols like strcmp, memcpy etc. would be unresolved.
    if std::env::var("CARGO_FEATURE_ZLOB").is_ok() {
        let target = std::env::var("TARGET").unwrap_or_default();
        if target.contains("windows") && target.contains("msvc") {
            println!("cargo:rustc-link-lib=msvcrt");
            println!("cargo:rustc-link-lib=ucrt");
            println!("cargo:rustc-link-lib=vcruntime");
        }
    } else if std::env::var("CI").is_ok() {
        // CI must always build with zlob for production-quality binaries.
        if !zig_available() {
            panic!(
                "CI detected but Zig is not installed. \
                 Please install Zig and build with `--features zlob`."
            );
        }
        panic!(
            "CI detected but `zlob` feature is not enabled. \
             Build with `--features zlob`."
        );
    } else {
        // Hint: if Zig is available but the zlob feature wasn't enabled,
        // let the developer know they can get faster glob matching.
        if zig_available() {
            println!(
                "cargo:warning=Zig detected but `zlob` feature is not enabled. \
                 Build with `--features zlob` for faster glob matching."
            );
        }
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
