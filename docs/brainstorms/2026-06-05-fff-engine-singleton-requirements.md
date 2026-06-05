# fff-engine Singleton — Requirements

**Date:** 2026-06-05
**Status:** Draft
**Topic:** Replace per-process index model with a singleton search engine daemon

---

## Problem

`fff-mcp` is a stdio-only MCP (Model Context Protocol) server — one process per Claude Code session. Every process independently:

- Scans the project root and builds a full `FilePicker` (file index)
- Builds its own `BigramFilter` (50–250 MB per process, rebuilt from file contents)
- Runs its own FS (filesystem) watcher thread

Running 3 sessions on a large repo multiplies each of these costs by 3. The `BigramFilter` is the dominant cost: it cannot be shared via mmap because it uses `Arc<>` heap pointers internally. The original mmap design deferred BigramFilter sharing as a known gap requiring ~2–3 weeks of additional Rust redesign.

---

## Goal

Replace the per-process index model with a **singleton search engine daemon** (`fff-engine`) that owns all state for a project root. Multiple `fff-mcp` instances become stateless proxies that forward search queries over a Unix socket and return results.

**Primary outcomes:**
- BigramFilter reduced from N copies (one per session) to 1 copy per project root
- Mmap design complexity (double-buffer, generation counters, `MmapPtr<T>`, sparse pre-allocation) eliminated entirely
- `fff-mcp` becomes stateless — all index, content, and frecency state moves to `fff-engine`

---

## Non-Goals

- Changing the external interface of `fff-mcp` (Claude Code still spawns it identically)
- Redesigning the BigramFilter data structure (it stays as-is, just moves into fff-engine's heap)
- Supporting remote (non-local) connections — Unix socket only, same machine
- Changing `fff-core`, `fff-grep`, or `fff-query-parser` (all unchanged)

---

## Architecture

### Components

**`fff-engine`** — new binary (`crates/fff-engine/`)

One instance per project root, identified by `sha256(base_path)`. Owns all search state in its own heap. Listens on a Unix socket. Handles concurrent connections from multiple `fff-mcp` instances.

Owns:
- `FilePicker` — file index for the project root (paths, git status, metadata)
- `BigramFilter` — bigram-to-file-set index for grep candidate pruning
- `FrecencyTracker` — LMDB (Lightning Memory-Mapped Database) frecency database (persistent user behaviour)
- FS watcher — keeps `FilePicker` current on file changes
- Tokio async runtime + Rayon thread pool for concurrent query handling

**`fff-mcp`** — modified internally, externally unchanged

Becomes a stateless proxy. On startup, checks for a running `fff-engine` for this project root (via lockfile). If absent, spawns one. Connects via Unix socket. Forwards all tool calls (`grep`, `find_files`, `multi_grep`) as serialised `SearchRequest` messages; receives `SearchResponse`. Has no local index state.

**`fff-core`, `fff-grep`, `fff-query-parser`** — unchanged

fff-engine is built on fff-core, same as fff-mcp is today.

### Topology

```
Claude Code ──stdio (MCP/JSON-RPC 2.0)──► fff-mcp (stateless proxy)
                                               │
                              bincode over Unix socket
                              4-byte LE length prefix
                                               │
                                               ▼
                                    fff-engine (Tokio server)
                              ┌───────────────────────────────┐
                              │  FilePicker  (in-heap)        │
                              │  BigramFilter  (in-heap)      │
                              │  FrecencyTracker  (LMDB)      │
                              │  FS watcher                   │
                              │  Tokio tasks per connection   │
                              │  Rayon pool for CPU search    │
                              └───────────────────────────────┘
```

Multiple sessions on the same root:

```
Claude Code 1 ──► fff-mcp #1 ──┐
Claude Code 2 ──► fff-mcp #2 ──┼──► fff-engine (project-a)
Claude Code 3 ──► fff-mcp #3 ──┘       1× BigramFilter

Claude Code 4 ──► fff-mcp #4 ────────► fff-engine (project-b)
                                        1× BigramFilter
```

### Wire Protocol

Communication between `fff-mcp` and `fff-engine` uses a simple length-prefixed binary framing:

```
[ 4 bytes, little-endian u32: payload length ] [ payload bytes ]
```

Payload is a `serde`-serialised Rust enum using `bincode` (compact, fast, no JSON overhead). Same-machine Unix socket round-trip for a typical grep response: ~0.1–0.5ms.

**Request types:**
```rust
enum SearchRequest {
    Grep { query: String, options: GrepOptions },
    FindFiles { query: String, options: FindOptions },
    MultiGrep { patterns: Vec<String>, options: GrepOptions },
    RecordAccess { path: String },  // fire-and-forget
}
```

**Response types:**
```rust
enum SearchResponse {
    SearchResults(Vec<SearchResult>),
    Error(String),
}
```

`RecordAccess` is fire-and-forget — `fff-mcp` sends and does not await any response. `fff-engine` sends no response for this variant; sending one would fill its socket send buffer on a long-lived connection where the client is not reading. Losing an occasional frecency write on crash degrades ranking slightly; no data is lost.

### Spawn Model

Identical to the original mmap design's spawn-if-absent approach:

1. `fff-mcp` checks for a lockfile at `$XDG_CACHE_HOME/fff/locks/<sha256-of-base-path>.lock`
2. Attempts `O_CREAT | O_EXCL` — the OS guarantees exactly one winner
3. Winner spawns `fff-engine` as a background process and waits for the Unix socket to appear
4. Losers wait with a short poll timeout for the socket to appear (fff-engine already being spawned by the winner)
5. All connect to the Unix socket at `$XDG_CACHE_HOME/fff/sockets/<sha256-of-base-path>.sock`

### Crash Recovery

`fff-mcp` detects `ECONNREFUSED` or broken pipe on the socket, treats it as an fff-engine crash, re-acquires the lockfile with `O_CREAT | O_EXCL`, respawns fff-engine, and reconnects. One fff-mcp wins the respawn race; others retry connection.

### Storage Layers

| Store | What | Where | Owner | Persistence |
|---|---|---|---|---|
| File index | FilePicker — paths, git status, metadata | fff-engine heap | fff-engine | Ephemeral — rebuilt from FS scan |
| Bigram index | BigramFilter — bigram → file set | fff-engine heap | fff-engine | Ephemeral — built from file contents |
| Frecency DB | FrecencyTracker — access timestamps per file | `$XDG_DATA_HOME/fff/frecency/` | fff-engine | Persistent — survives restarts |
| Source files | Individual file contents for grep scanning | OS page cache (mmap per file) | OS | Ephemeral — normal page cache eviction |

The shared index mmap file from the original design is **not used**. FilePicker and BigramFilter live in fff-engine's heap — normal Rust heap allocations, no custom binary format, no inter-process sharing mechanism needed.

LMDB is internally mmap-based (that is what the name means), but this is transparent — fff-engine interacts with it via the `heed` crate as today.

---

## Concurrency Model

fff-engine handles concurrent connections from N fff-mcp instances:

- **Connection handling**: Tokio async task per connected fff-mcp client
- **CPU-bound search**: delegated to Rayon via `tokio::task::spawn_blocking`. Rayon's work-stealing pool parallelises file scanning across available cores.
- **FilePicker reads**: protected by the existing `Arc<parking_lot::RwLock<Option<FilePicker>>>`. Multiple concurrent readers are allowed; FS watcher takes a write lock only during index rebuild.
- **Frecency writes**: serialised by LMDB's internal write lock. Fire-and-forget from fff-mcp; buffered and flushed async in fff-engine.

**Thread-safety confirmed** (pre-implementation audit complete):
- `SharedFilePicker` wraps `Arc<parking_lot::RwLock<Option<FilePicker>>>` — `parking_lot::RwLock` allows true concurrent readers with no blocking between them.
- `BigramFilter` has no interior mutability (`Cell`, `RefCell`, `Mutex`). All query methods are `&self`. Concurrent reads are safe.
- All grep/find_files/fuzzy_search query paths on `FilePicker` are `&self`. The only `&mut` methods (FS watcher callbacks, post-scan indexing) are never called during a search.
- fff-core already uses Rayon internally (grep uses `.par_iter()` on the global pool). fff-engine calling into fff-core from Tokio `spawn_blocking` tasks introduces no nested pool deadlock — the blocking task runs on Tokio's blocking thread pool and Rayon runs on its own separate global pool.

---

## Resource Comparison

| Resource | Before (N=3 sessions, same root) | After |
|---|---|---|
| BigramFilter RAM | 3 × 50–250 MB | 1 × 50–250 MB |
| FilePicker RAM | 3 × ~10–50 MB (mmap dedup helped) | 1 × ~10–50 MB |
| FS watcher threads | 3 | 1 |
| Index scan on startup | 3× sequential | 1× (others connect instantly) |
| Mmap format complexity | High (double-buffer, generation counters, MmapPtr) | None |

---

## What Is Eliminated

The following planned components from the original mmap design are **not needed**:

- Shared index file (`$XDG_CACHE_HOME/fff/index/<hash>.idx`)
- Sparse pre-allocation via `ftruncate`
- Double-buffer slot A / slot B
- Atomic generation counter
- Per-slot reader drain
- `MmapPtr<T>` offset-based pointer wrapper
- Generation check loop in `fff-mcp`

These existed solely to share a memory-mapped index across process boundaries. With a singleton process, the index is a normal heap allocation.

---

## Scope of Changes

| Component | Change |
|---|---|
| `fff-engine` | **New binary** — Tokio async Unix socket server, all search state |
| `fff-mcp` | **Internal rewrite** — stateless proxy; external interface unchanged |
| `fff-core` | **Unchanged** |
| `fff-grep` | **Unchanged** |
| `fff-query-parser` | **Unchanged** |
| `fff-nvim`, `fff` TUI | **Minor** — connect to fff-engine socket instead of building standalone index (follow-on, not in scope here) |
| Install script + `.mcp.json` | **Updated** — `--frecency-db` flag moves from fff-mcp to fff-engine CLI; install script and default `.mcp.json` template updated accordingly |

### Frecency flag migration detail

`--frecency-db` is currently absent from the default `.mcp.json`, silently disabling frecency for all MCP sessions. With fff-engine owning `FrecencyTracker`, the flag becomes an fff-engine CLI argument instead. Frecency is enabled by default when fff-engine is spawned by fff-mcp — fff-mcp passes the standard path (`$XDG_DATA_HOME/fff/frecency/`) when it spawns fff-engine, so users who previously had frecency disabled get it for free with no config change required.

---

## Open Questions

- **fff-engine naming**: `fff-engine` is the chosen name. Confirm this is the name used in `Cargo.toml` and binary output before implementation begins.
- **fff-nvim / fff TUI integration**: minor modification to attach to fff-engine instead of building their own index. Deferred — not in scope for this track.
- **Upstream RFC**: file the design as an RFC to `dmtrKovalenko/fff` or maintain as a downstream fork? Deferred — decide after implementation is stable.
