//! Glob wildcard detection — delegates to zlob when available, pure-Rust fallback otherwise.
//!
//! All call sites use a single function: `has_wildcards(text) -> bool`.
//! When the `zlob` feature is enabled this calls `zlob::has_wildcards` with
//! `ZlobFlags::RECOMMENDED`; without it we check for the same set of wildcard
//! characters (`*`, `?`, `[`, `{`) in pure Rust.

#[cfg(feature = "zlob")]
#[inline]
pub fn has_wildcards(s: &str) -> bool {
    zlob::has_wildcards(s, zlob::ZlobFlags::RECOMMENDED)
}

#[cfg(not(feature = "zlob"))]
#[inline]
pub fn has_wildcards(s: &str) -> bool {
    s.bytes().any(|b| matches!(b, b'*' | b'?' | b'[' | b'{'))
}
