fn main() {
    // On Windows MSVC, explicitly link the C runtime libraries.
    // This is needed because Zig-compiled static libraries (zlob) don't emit
    // /DEFAULTLIB directives for the MSVC CRT. Without this, symbols like
    // strcmp, memcpy, memchr etc. from vendored C libraries (libgit2, lmdb)
    // are unresolved when linking the cdylib.
    //
    // We link both msvcrt (classic CRT) and ucrt (Universal CRT where memchr,
    // strcmp etc. live on newer MSVC/ARM64 targets).
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("windows") && target.contains("msvc") {
        println!("cargo:rustc-link-lib=msvcrt");
        println!("cargo:rustc-link-lib=ucrt");
        println!("cargo:rustc-link-lib=vcruntime");
    }
}
