//! Wire types shared between `fff-daemon` and `fff-rg`.
//!
//! The IPC protocol is one request per connection over a Unix domain socket:
//! - Client sends a [`RequestHeader`] (4-byte LE length) + rkyv-serialized
//!   [`SearchRequest`] body, with an output fd attached via SCM_RIGHTS.
//! - Daemon writes results to the fd and replies with a [`SearchStatus`] byte.
//!
//! All structured payloads use rkyv zero-copy serialization. The socket path
//! is per-user via [`daemon_socket_path`].

use std::num::{NonZeroU32, NonZeroU64};
use std::path::PathBuf;

use rkyv::{Archive, Deserialize, Serialize};

/// Returns the daemon socket path: `/tmp/fff-daemon-<uid>.sock`.
pub fn daemon_socket_path() -> PathBuf {
    let uid = unsafe { libc::getuid() };
    std::env::temp_dir().join(format!("fff-daemon-{uid}.sock"))
}

/// Case sensitivity strategy for grep searches. Mirrors `fff::CaseMode` but
/// with rkyv derives — fff-core doesn't depend on rkyv.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[rkyv(derive(Debug))]
pub enum CaseMode {
    /// Case-insensitive if the query is all lowercase, sensitive otherwise.
    Smart,
    /// Always case-sensitive.
    Sensitive,
    /// Always case-insensitive.
    Insensitive,
}

/// Grep pattern — either a regex or a literal string.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[rkyv(derive(Debug))]
pub enum GrepQuery {
    /// Regex pattern.
    Regex(String),
    /// Literal string match.
    Literal(String),
}

/// Grep-specific search parameters.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[rkyv(derive(Debug))]
pub struct GrepSearch {
    /// The search pattern.
    pub query: GrepQuery,
    /// Case sensitivity strategy.
    pub case_mode: CaseMode,
    /// Per-file match limit. `None` = unlimited.
    pub max_count: Option<NonZeroU32>,
    /// Skip files larger than this (bytes). `None` = default (4 MB).
    pub max_filesize: Option<NonZeroU64>,
    /// Lines of context before each match.
    pub before_context: u32,
    /// Lines of context after each match.
    pub after_context: u32,
    /// Strip leading whitespace from matched lines.
    pub trim: bool,
}

/// Discriminated search request — files or grep with variant-specific data.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[rkyv(derive(Debug))]
pub enum SearchKind {
    /// Fuzzy filename search.
    Files {
        /// Fuzzy query string.
        query: String,
    },
    /// Content search within files.
    Grep(GrepSearch),
}

/// Bitmask controlling how the daemon formats search results.
/// Hand-rolled instead of `bitflags!` because the macro-generated struct
/// doesn't derive rkyv `Archive`/`Serialize`/`Deserialize`.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[rkyv(derive(Debug))]
pub struct OutputFlags(u16);

impl OutputFlags {
    pub const COLOR: Self = Self(1 << 0);
    pub const LINE_NUMBER: Self = Self(1 << 1);
    pub const COLUMN: Self = Self(1 << 2);
    pub const HEADING: Self = Self(1 << 3);
    pub const WITH_FILENAME: Self = Self(1 << 4);
    pub const COUNT_ONLY: Self = Self(1 << 5);
    pub const FILES_ONLY: Self = Self(1 << 6);
    pub const QUIET: Self = Self(1 << 7);
    pub const VIMGREP: Self = Self(1 << 8);

    #[must_use]
    pub const fn empty() -> Self { Self(0) }
    #[must_use]
    pub const fn contains(self, flag: Self) -> bool { self.0 & flag.0 == flag.0 }
}

impl std::ops::BitOr for OutputFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self { Self(self.0 | rhs.0) }
}

impl std::ops::BitOrAssign for OutputFlags {
    fn bitor_assign(&mut self, rhs: Self) { self.0 |= rhs.0; }
}

/// Top-level request sent by the client as the rkyv body.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[rkyv(derive(Debug))]
pub struct SearchRequest {
    /// Root directory to search in (absolute path).
    pub directory: String,
    /// What to search for and how.
    pub search: SearchKind,
    /// Output formatting flags.
    pub output: OutputFlags,
}

/// One-byte response code the daemon writes back after a search completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SearchStatus {
    /// At least one result was found and written to the output fd.
    Match = 0,
    /// Search completed successfully but produced no results.
    NoMatch = 1,
    /// Search failed (e.g. indexing timeout, invalid query).
    Failed = 2,
}

impl From<SearchStatus> for u8 {
    fn from(s: SearchStatus) -> Self {
        s as Self
    }
}

impl TryFrom<u8> for SearchStatus {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Match),
            1 => Ok(Self::NoMatch),
            2 => Ok(Self::Failed),
            other => Err(other),
        }
    }
}

/// 4-byte little-endian length prefix for the rkyv request body.
#[derive(Debug, Clone, Copy)]
pub struct RequestHeader {
    /// Length of the rkyv-serialized [`SearchRequest`] body in bytes.
    pub body_len: u32,
}

impl RequestHeader {
    /// Wire size of the length prefix (4 bytes, LE u32).
    pub const SIZE: usize = 4;

    /// Encodes the header as a 4-byte LE array for writing to the socket.
    pub fn encode(body_len: usize) -> [u8; Self::SIZE] {
        u32::try_from(body_len)
            .expect("request body exceeds u32::MAX")
            .to_le_bytes()
    }

    /// Decodes a 4-byte LE buffer into a header.
    pub fn decode(buf: [u8; Self::SIZE]) -> Self {
        Self { body_len: u32::from_le_bytes(buf) }
    }
}
