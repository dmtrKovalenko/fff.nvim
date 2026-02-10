fn main() {
    // On Windows MSVC, explicitly link the C runtime libraries.
    // This is needed because Zig-compiled static libraries (zlob) don't emit
    // /DEFAULTLIB directives for the MSVC CRT. Without this, symbols like
    // strcmp, memcpy etc. from vendored C libraries (libgit2, lmdb) are
    // unresolved when linking the cdylib.
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("windows") && target.contains("msvc") {
        println!("cargo:rustc-link-lib=msvcrt");
    }
}
