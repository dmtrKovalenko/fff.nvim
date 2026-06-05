---
title: "feat: Add fff-engine singleton daemon and rewrite fff-mcp as stateless proxy"
date: 2026-06-05
status: completed
origin: docs/brainstorms/2026-06-05-fff-engine-singleton-requirements.md
type: feat
---

# feat: fff-engine Singleton Daemon

## Summary

Replace fff-mcp's per-process index model with a singleton `fff-engine` daemon that owns all search state (FilePicker, BigramFilter, FrecencyTracker) in its own heap. Multiple `fff-mcp` instances become stateless proxies communicating with `fff-engine` over a bincode-framed Unix socket. Eliminates per-process BigramFilter cost (50–250 MB × N sessions → 1 copy) and removes the mmap design's double-buffer/generation-counter complexity.

**Target repo:** `~/my-workspace/util/fff/` (paths below are repo-relative)

---

## Problem Frame

fff-mcp spawns one process per MCP (Model Context Protocol) client session. Each independently scans the project root, builds a `BigramFilter` (50–250 MB), and runs its own FS (filesystem) watcher. Three concurrent sessions multiply all of these costs by three. The `BigramFilter` is the dominant cost and cannot be shared via mmap because it uses `Arc<>` heap pointers internally.

*See origin: `docs/brainstorms/2026-06-05-fff-engine-singleton-requirements.md`*

---

## Requirements

From the origin document:

- **R1** — BigramFilter reduced from N copies per project root to 1
- **R2** — fff-mcp external interface unchanged (Claude Code spawns it identically)
- **R3** — mmap design complexity eliminated (no double-buffer, generation counters, MmapPtr)
- **R4** — fff-mcp becomes stateless; all index and frecency state moves to fff-engine
- **R5** — Frecency enabled by default via XDG (Cross-Desktop Group) path (was silently disabled)
- **R6** — Install script and `.mcp.json` template updated for frecency flag migration

---

## Key Technical Decisions

**KTD-1: Shared IPC (Inter-Process Communication) types in a new `fff-ipc` crate**
Both `fff-engine` and `fff-mcp` need the `SearchRequest`/`SearchResponse` types and the framing codec. A shared crate gives each binary a single source of truth for the wire format — divergence between the two copies silently breaks the protocol with a bincode deserialization error that is hard to diagnose.

**Why a crate rather than a module in fff-core:** fff-core is the search engine library. Embedding daemon IPC types in it couples search logic with transport concerns, and any future consumer of fff-core (e.g. a C FFI binding or a WASM build) would pull in socket and serialization dependencies they don't need. A separate `fff-ipc` crate has zero runtime logic of its own — it is types + codec + path helpers — and any future client (fff-nvim, fff TUI, a CLI tool) can depend on it without depending on fff-core.

**The counter-argument acknowledged:** At plan time fff-ipc has exactly two consumers. The prior mmap design put shared types in `fff-core/src/ipc.rs` (one file, no new crate) which is a lighter-weight choice for two-consumer scenarios. If the scope stays at two consumers and this codebase is never published upstream, the module-in-fff-core approach is equally valid. The crate boundary is a defensible choice given the planned upstream RFC, but not a hard requirement.

**KTD-2: bincode + 4-byte LE length prefix for wire framing**
`bincode` is compact, fast, and serde-compatible. JSON was rejected (unnecessary overhead on a same-machine socket). The length prefix enables streaming reads on a Tokio `UnixStream` without knowing message boundaries in advance. The workspace currently has no `bincode` dep; it will be added.

**KTD-3: blake3 hash of base_path for daemon identity**
Daemon socket and lockfile paths are derived from `blake3(canonical_base_path)` stored under `$XDG_CACHE_HOME/fff/`. This avoids filesystem path characters in socket names and gives each project root a stable, collision-resistant identity. `blake3` is already a workspace dependency (used in fff-core for frecency keys); no new crate is needed.

**KTD-4: O_CREAT|O_EXCL lockfile for spawn-if-absent race**
The OS guarantees exactly one winner on `O_CREAT|O_EXCL`. The winner spawns fff-engine; losers poll for the socket to appear. Same approach used by proven IPC daemons (Neovim server, language servers). No in-process synchronization is needed.

**KTD-5: RecordAccess defined but not sent in this track**
`FrecencyTracker::track_access()` was never called from fff-mcp tool handlers — frecency writes were silently disabled (no `--frecency-db` in the default `.mcp.json`). `SearchRequest::RecordAccess` is defined in fff-ipc's `types.rs` and handled with a no-op in fff-engine's `handlers.rs`, but fff-mcp never sends it in this implementation. Frecency *scoring* (reads) works immediately from any pre-existing LMDB data.

**What the follow-on track needs to resolve before enabling writes:**

1. **Trigger decision** — fff-mcp has no "file opened" signal from Claude Code; it only sees search calls. Options: (a) send `RecordAccess` for the top-ranked result of every search (cheap heuristic, starts accumulating signal immediately), (b) add a new MCP tool `record_access(path)` that Claude Code calls explicitly when it reads a file (accurate but requires MCP client changes), (c) batch-record all results above a score threshold. Pick one before implementing.

2. **Implementation path (once trigger is decided):**
   - `fff-mcp/src/client.rs`: call `self.record_access(path)` after a successful search (fire-and-forget, already stubbed)
   - `fff-engine/src/handlers.rs`: replace the `RecordAccess` no-op with `shared_frecency.write().as_mut()?.track_access(Path::new(&path))`
   - No new IPC types or codec changes needed — `SearchRequest::RecordAccess { path: String }` is already in the wire format

3. **Test gap to fill:** frecency scores for recently accessed files rise over repeated searches (integration test across multiple search calls).

**KTD-6: fff-mcp retains --base-path and frecency-related flags are removed**
fff-mcp's `--frecency-db`, `--max-cached-files`, `--content-indexing`, and `--no-watch` flags all controlled FilePicker/FrecencyTracker initialization that now lives in fff-engine. fff-mcp removes these flags from its own CLI. fff-engine acquires equivalent flags. `--base-path` stays in fff-mcp (it needs the root to derive the socket path).

**KTD-7: daemon architecture is `#[cfg(unix)]` only; Windows uses the existing per-process model**
`tokio::net::UnixListener` and `UnixStream` are Unix-only APIs — they do not compile on Windows targets. The codebase actively supports Windows (`install-mcp.ps1`, Windows release assets, `#[cfg(not(target_os = "windows"))]` guards in fff-core). The daemon + proxy rewrite is gated behind `#[cfg(unix)]` throughout fff-engine and the new fff-mcp client layer. On Windows, fff-mcp retains its current standalone behaviour (direct fff-core calls, per-process index). This is not a regression — Windows never had the daemon path.

---

## High-Level Technical Design

### Component topology

```
Claude Code ─── stdio (MCP/JSON-RPC 2.0) ───► fff-mcp
                                                  │
                                    bincode-framed UnixStream
                                    (SearchRequest / SearchResponse)
                                                  │
                                                  ▼
                                         fff-engine
                           ┌─────────────────────────────────────┐
                           │  SharedFilePicker (Arc<RwLock<..>>)  │
                           │  BigramFilter (in-heap, one copy)    │
                           │  SharedFrecency  (Arc<RwLock<..>>)   │
                           │  FS watcher thread                   │
                           │  Tokio accept loop                   │
                           │  Rayon global pool (grep workers)    │
                           └─────────────────────────────────────┘
```

### Spawn-if-absent sequence

```
fff-mcp starts
    │
    ├─ resolve socket path: $XDG_CACHE_HOME/fff/sockets/<sha256>.sock
    ├─ resolve lockfile path: $XDG_CACHE_HOME/fff/locks/<sha256>.lock
    │
    ├─ try O_CREAT|O_EXCL on lockfile
    │       ├─ WON → spawn fff-engine --base-path <path> --frecency-db <xdg-data>
    │       │         poll until socket appears (readiness signal)
    │       └─ LOST → poll until socket appears (another fff-mcp is spawning)
    │
    └─ connect UnixStream to socket path → ready
```

### Per-connection query flow

```
fff-mcp receives MCP tool call (e.g. grep)
    │
    ├─ map MCP params → SearchRequest::Grep { query, options }
    ├─ write length-framed bincode to UnixStream
    │
    │  [fff-engine]
    │  read framed SearchRequest
    │  tokio::task::spawn_blocking(|| picker.grep(...))
    │  ← Rayon workers scan files in parallel
    │  serialize SearchResponse
    │  write length-framed bincode back
    │
    └─ read SearchResponse from UnixStream
       map SearchResults → MCP CallToolResult
       return to Claude Code
```

### Crash recovery sequence

```
fff-mcp sends request → ECONNREFUSED or broken pipe
    │
    ├─ read PID from lockfile
    │       ├─ kill(pid, 0) succeeds → daemon starting slowly
    │       │         wait 100 ms → retry connect (backoff: 100→200→400 ms, 3 attempts)
    │       └─ kill(pid, 0) fails (dead) OR lockfile missing/unreadable
    │                 → delete stale lockfile + stale socket file
    │                 → retry O_CREAT|O_EXCL (one fff-mcp wins the race)
    │                 → winner: write PID, spawn fff-engine, wait for socket
    │                 → losers: poll for socket
    │
    └─ reconnect → retry original request
       on 3rd failure → return SearchResponse::Error
```

---

## Output Structure

```
crates/
  fff-ipc/               ← NEW: shared wire types + framing codec
    Cargo.toml
    src/
      lib.rs             ← re-exports
      types.rs           ← SearchRequest, SearchResponse, options structs
      codec.rs           ← length-prefix framing (async read/write helpers)
      paths.rs           ← socket/lockfile path derivation (sha256 + XDG)

  fff-engine/            ← NEW: singleton daemon binary
    Cargo.toml
    src/
      main.rs            ← CLI args (clap), init orchestration
      state.rs           ← SharedFilePicker + SharedFrecency initialization
      handlers.rs        ← SearchRequest dispatch → fff-core calls
      server.rs          ← Tokio UnixListener, accept loop, per-connection tasks
      lifecycle.rs       ← lockfile, socket readiness, graceful shutdown

  fff-mcp/               ← MODIFIED: proxy rewrite
    src/
      client.rs          ← NEW: EngineClient (spawn-if-absent + UnixStream)
      main.rs            ← MODIFIED: remove FilePicker init, add client startup
      server.rs          ← MODIFIED: tool handlers delegate to EngineClient
      recovery.rs        ← NEW: crash detection + respawn logic
```

---

## Implementation Units

### U1. fff-ipc crate — shared wire types and framing codec

**Goal:** Define the `SearchRequest` / `SearchResponse` wire types and the length-prefix codec used by both fff-engine (server) and fff-mcp (client). Establish socket and lockfile path derivation.

**Requirements:** KTD-1, KTD-2, KTD-3

**Dependencies:** none

**Files:**
- `crates/fff-ipc/Cargo.toml` (new)
- `crates/fff-ipc/src/lib.rs` (new)
- `crates/fff-ipc/src/types.rs` (new)
- `crates/fff-ipc/src/codec.rs` (new)
- `crates/fff-ipc/src/paths.rs` (new)
- `Cargo.toml` (add `fff-ipc` to workspace members)
- `crates/fff-ipc/src/types.rs` test module (inline)

**Approach:**

`types.rs` — define:
```
SearchRequest { Grep, FindFiles, MultiGrep, RecordAccess }
SearchResponse { SearchResults(Vec<WireSearchResult>), Error(String) }
GrepOptions  — mirrors GrepSearchOptions fields (serialisable subset)
FindOptions  — mirrors FuzzySearchOptions fields (serialisable subset)

// Owned wire types — NOT the fff-core types (which are lifetime-bound arena borrows)
WireSearchResult {
    path: String,          // materialised from ChunkedString arena while guard is held
    line: Option<u32>,     // grep line number; None for find_files
    snippet: Option<String>,
    score: i64,
    git_status: Option<u32>,  // raw git2::Status bits; None if unknown
}
WireGrepMatch {
    line_number: u32,
    line_text: String,
    match_byte_offsets: Vec<(u32, u32)>,   // converted from SmallVec — serde-compatible
}
```

**Critical:** `fff-core`'s `SearchResult<'a>`, `GrepResult<'a>`, and `FileItem` are **not serializable** — `FileItem` contains `AtomicU8`, `OnceLock<memmap2::Mmap>`, and `ChunkedString` (a private arena-indexed type with raw pointers). None derive `Serialize`. U4 must project results into these owned wire types while holding the picker read-lock.

All fff-ipc types derive `serde::Serialize + serde::Deserialize`. `RecordAccess` variant carries the path string; fff-mcp never sends it in this track but the variant exists for forward compatibility.

`codec.rs` — two async functions:
- `write_message<W, T>(writer, &T)` — serialise with bincode, prepend 4-byte LE length, write to `AsyncWrite`
- `read_message<R, T>(reader) -> Result<T>` — read 4-byte length, read that many bytes, deserialise with bincode from `AsyncRead`

`paths.rs` — given a canonical `base_path: &Path`:
- `socket_path(base_path) -> PathBuf` — `<cache_dir>/fff/sockets/<blake3hex(base_path)>.sock`
- `lockfile_path(base_path) -> PathBuf` — `<cache_dir>/fff/locks/<blake3hex(base_path)>.lock`

Uses `blake3::hash` (already a workspace dep) for the hash. Uses `dirs::cache_dir()` (already in workspace) for XDG resolution — this encodes the correct fallback: `$XDG_CACHE_HOME` if set, else `$HOME/.cache` on Linux/macOS (macOS does not set `XDG_CACHE_HOME` by default). Never call `std::env::var("XDG_CACHE_HOME")` directly without this fallback.

**Patterns to follow:** `heed`'s serde integration for how bincode fits with existing serialisation style; `fff-core/src/types.rs` for FileItem/SearchResult field naming conventions.

**Test scenarios:**
- `write_message` then `read_message` round-trips a `SearchRequest::Grep` with non-ASCII query without data loss
- `write_message` then `read_message` round-trips a `SearchResponse::SearchResults` with an empty vec
- `write_message` then `read_message` round-trips a `SearchResponse::Error` string
- `socket_path` returns a path under a `fff/sockets/` subdirectory
- `socket_path` for two different base paths returns two distinct paths
- `socket_path` for the same base path called twice returns the same path
- `lockfile_path` returns a path under `fff/locks/` and ends in `.lock`
- `read_message` on a truncated byte stream returns an error (not a panic)

**Verification:** `cargo test -p fff-ipc` passes; `cargo clippy -p fff-ipc` clean.

---

### U2. fff-engine crate scaffold

**Goal:** Register the `fff-engine` crate in the workspace, establish its `Cargo.toml` dependencies, and produce a runnable binary with CLI argument parsing.

**Requirements:** R4

**Dependencies:** U1

**Files:**
- `crates/fff-engine/Cargo.toml` (new)
- `crates/fff-engine/src/main.rs` (new, stub)
- `Cargo.toml` (add `fff-engine` to workspace members)

**Approach:**

`Cargo.toml` dependencies: `fff-search` (path = `../fff-core`), `fff-ipc` (path = `../fff-ipc`), `tokio` (features = full), `parking_lot`, `serde`, `bincode`, `blake3`, `dirs`, `tracing`, `tracing-subscriber`, `clap` (derive + env), `mimalloc`. (`blake3` and `dirs` are already workspace deps — no new crates needed for path derivation.)

CLI args (clap `Args` struct):
- `--base-path <PATH>` — project root (required; no default, caller must supply)
- `--frecency-db <PATH>` — LMDB frecency path (optional; defaults to `$XDG_DATA_HOME/fff/frecency/`)
- `--log-file <PATH>` — optional log output
- `--log-level <LEVEL>` — default `info`
- `--no-watch` — disable FS watcher (testing convenience)
- `--no-warmup` — skip background scan warmup (testing convenience)

`main.rs` stub: parse args, initialise tracing, print startup log line, exit 0.

**Test scenarios:**
- `Test expectation: none — scaffold only; no behaviour to verify beyond compilation`

**Verification:** `cargo build -p fff-engine` produces a binary; `fff-engine --help` prints usage.

---

### U3. fff-engine state initialization

**Goal:** Implement the startup sequence that creates and warms `SharedFilePicker`, `SharedFrecency`, and starts the FS watcher. By the end of this unit fff-engine holds a live, queryable index.

**Requirements:** R1, R4, R5

**Dependencies:** U2

**Files:**
- `crates/fff-engine/src/state.rs` (new)
- `crates/fff-engine/src/main.rs` (modified — call `state::init`)

**Approach:**

`state.rs` exposes `EngineState` holding `SharedFilePicker` and `SharedFrecency`. Its `init(args) -> Result<EngineState>` function mirrors fff-mcp's current startup sequence:

1. Resolve canonical `base_path` (same git-root discovery logic as fff-mcp: `git2::Repository::discover`, fallback to explicit path)
2. `SharedFilePicker::default()` + `SharedFrecency::default()`
3. If `--frecency-db` is set, or if the XDG default path is used (always in daemon mode), call `FrecencyTracker::open(path)` → `shared_frecency.init(tracker)`
4. `FilePicker::new_with_shared_state(shared_picker.clone(), shared_frecency.clone(), FilePickerOptions { base_path, mode: FFFMode::Ai, watch: !args.no_watch, enable_mmap_cache: !args.no_warmup, ... })`
5. Return `EngineState { shared_picker, shared_frecency, base_path }`

Frecency is enabled by default: fff-engine always opens the LMDB database at `$XDG_DATA_HOME/fff/frecency/` unless `--frecency-db` overrides. This is the R5 behavior change vs. fff-mcp's opt-in `--frecency-db`.

**Patterns to follow:** `crates/fff-mcp/src/main.rs` lines 215–276 — identical initialization sequence, just extracted to a function.

**Test scenarios:**
- `init` with a valid git repository path sets `base_path` to the git workdir root
- `init` with a non-git directory sets `base_path` to the supplied path
- `init` opens LMDB frecency at the XDG default path when `--frecency-db` is omitted
- `init` opens LMDB frecency at the explicitly supplied `--frecency-db` path when provided
- `shared_picker.read()` returns `Some(picker)` after `init` completes (scan has started)

**Verification:** `cargo test -p fff-engine state` passes; a manually launched `fff-engine --base-path <repo>` emits a log line confirming scan start.

---

### U4. fff-engine search handlers

**Goal:** Implement the dispatch functions that receive a `SearchRequest`, call the appropriate fff-core method via `spawn_blocking`, and return a `SearchResponse`.

**Requirements:** R1, R4

**Dependencies:** U3

**Files:**
- `crates/fff-engine/src/handlers.rs` (new)

**Approach:**

`handlers.rs` exposes one public async function per request variant:

```
handle_grep(state: &EngineState, req: GrepRequest) -> SearchResponse
handle_find_files(state: &EngineState, req: FindRequest) -> SearchResponse
handle_multi_grep(state: &EngineState, req: MultiGrepRequest) -> SearchResponse
```

Each function follows this exact pattern — **lock acquisition and wire projection must both happen inside `spawn_blocking`**:

1. Clone `state.shared_picker` (`Arc::clone` — cheap, the clone crosses the `Send` boundary into the blocking thread; `parking_lot::RwLockReadGuard` is NOT `Send` and must never cross it)
2. Inside `tokio::task::spawn_blocking(move || { ... })`:
   a. Acquire the read guard: `let guard = picker.read(); let picker_ref = guard.as_ref()?;`
   b. Call the fff-core method (`picker_ref.grep(...)`, `picker_ref.fuzzy_search(...)`, etc.)
   c. **Project results into owned wire types while the guard is still held** — `WireSearchResult { path: item.path.to_string(), ... }` — because `FileItem.path` is a `ChunkedString` (arena-relative pointer) that becomes invalid once the guard drops
   d. Drop the guard (implicit at end of scope)
   e. Return `Vec<WireSearchResult>`
3. Map to `SearchResponse::SearchResults(results)` or `SearchResponse::Error`

`GrepRequest`, `FindRequest`, `MultiGrepRequest` are extracted from the `SearchRequest` enum variants. The mapping from `GrepOptions` (IPC type) → `GrepSearchOptions` (fff-core type) is a straightforward field copy; any IPC field not present in fff-core is defaulted.

`RecordAccess` is handled with a no-op: the variant exists in the enum; the handler returns immediately without calling `track_access`. This is KTD-5.

**Patterns to follow:** `crates/fff-mcp/src/server.rs` `perform_grep`, `handle_find_files`, and `multi_grep` — these are the exact call sites being replicated; extract the fff-core call and strip the MCP formatting layer.

**Test scenarios:**
- `handle_grep` with a query matching a known file returns `SearchResponse::SearchResults` with at least one result containing the expected file path
- `handle_grep` with a query matching nothing returns `SearchResponse::SearchResults` with an empty vec (not an error)
- `handle_find_files` with a partial filename returns the expected file in results
- `handle_multi_grep` with two patterns where only one matches returns results from the matching pattern
- `handle_grep` when `SharedFilePicker` contains `None` (scan not yet complete) returns `SearchResponse::Error` with a descriptive message (not a panic)

**Verification:** `cargo test -p fff-engine handlers` passes against a temporary directory.

---

### U5. fff-engine Unix socket server

**Goal:** Stand up the Tokio `UnixListener`, accept concurrent client connections, and dispatch each request through the handlers from U4.

> `fff-engine` as a whole is a Unix-only binary (KTD-7). The entire crate is compiled only on `#[cfg(unix)]` targets; Windows is out of scope per Non-Goals.

**Requirements:** R1, R4

**Dependencies:** U4

**Files:**
- `crates/fff-engine/src/server.rs` (new)
- `crates/fff-engine/src/main.rs` (modified — call `server::run`)

**Approach:**

`server.rs` exposes `run(state: Arc<EngineState>, socket_path: PathBuf) -> Result<()>`:

1. Create parent dirs for `socket_path`
2. Remove any stale socket file at that path (leftover from a previous crash)
3. `tokio::net::UnixListener::bind(socket_path)`
4. Loop: `listener.accept()` → `tokio::spawn(handle_connection(stream, state.clone()))`
5. Handle `SIGTERM`/`SIGINT` via `tokio::signal` — break the accept loop, remove the socket file on exit

`handle_connection(stream, state)`:
1. Split stream into `read_half` + `write_half`
2. Loop: `codec::read_message(&mut read_half)` → match variant → dispatch to handler → `codec::write_message(&mut write_half, response)` (skip write for `RecordAccess`)
3. Break on EOF or read error (client disconnected)

Concurrency: each connection runs in its own Tokio task. `spawn_blocking` inside handlers provides CPU parallelism via Rayon.

**Patterns to follow:** Standard Tokio echo-server pattern; `rmcp`'s stdio transport (`crates/fff-mcp/src/main.rs` `server.serve(stdio())`) as the existing async server reference.

**Test scenarios:**
- A client that connects and sends `SearchRequest::Grep` receives a `SearchResponse` (not a hang or panic)
- Two clients connected simultaneously both receive responses (concurrency)
- A client that disconnects mid-session (drops the stream) does not crash fff-engine or affect other clients
- A malformed framing byte sequence (wrong length prefix) returns `SearchResponse::Error` and closes the connection gracefully (not a panic)
- `RecordAccess` message receives no response — client does not time out waiting

**Verification:** `cargo test -p fff-engine server` passes; manual `fff-engine --base-path <repo>` accepts a connection from a raw Unix socket client (e.g., `socat`).

---

### U6. fff-engine lifecycle — lockfile, readiness, and shutdown

**Goal:** Add the spawn-if-absent lockfile mechanism on the fff-engine side: acquire the lockfile on startup, create the socket file as the readiness signal, and clean up on exit.

**Requirements:** R4

**Dependencies:** U5

**Files:**
- `crates/fff-engine/src/lifecycle.rs` (new)
- `crates/fff-engine/src/main.rs` (modified — integrate lifecycle around server::run)

**Approach:**

`lifecycle.rs` exposes:
- `acquire_lockfile(lockfile_path: &Path) -> Result<LockfileGuard>` — opens with `O_CREAT|O_EXCL`; returns `Err` if another process already holds it. `LockfileGuard` removes the file on drop.
- `await_ready_signal(socket_path: &Path, timeout: Duration) -> Result<()>` — polls for socket file existence (for the fff-mcp side; this is the "readiness" the daemon emits by simply binding the socket)

Startup order in `main.rs`:
1. Resolve paths (U1 `paths.rs`)
2. Attempt `acquire_lockfile` — if it fails, another daemon is running; exit cleanly
3. `state::init` (U3)
4. `server::run` (U5) — binding the socket IS the readiness signal
5. On exit: `LockfileGuard` drops (removes lockfile); `server::run` removes socket file

**Test scenarios:**
- `acquire_lockfile` succeeds when no lockfile exists and creates the file
- `acquire_lockfile` returns an error when the lockfile already exists (simulated by pre-creating the file)
- `LockfileGuard` drop removes the lockfile file
- `await_ready_signal` returns `Ok` once the socket file appears
- `await_ready_signal` returns `Err` after timeout when socket never appears

**Verification:** Two concurrent `fff-engine --base-path <repo>` processes: exactly one succeeds in binding; the other exits with a clean "daemon already running" message.

---

### U7. fff-mcp proxy rewrite

**Goal:** Replace fff-mcp's direct fff-core calls with `EngineClient` calls. Remove FilePicker/FrecencyTracker initialization from fff-mcp. Add spawn-if-absent startup logic.

**Requirements:** R2, R3, R4, R5 (R2: external interface unchanged)

**Dependencies:** U6 (fff-engine fully functional before fff-mcp can connect)

**Files:**
- `crates/fff-mcp/src/client.rs` (new)
- `crates/fff-mcp/src/main.rs` (modified)
- `crates/fff-mcp/src/server.rs` (modified)
- `crates/fff-mcp/src/healthcheck.rs` (modified — rewrite to probe daemon socket)
- `crates/fff-mcp/Cargo.toml` (add `fff-ipc` dep; remove unused deps if any)

**Approach:**

`client.rs` — `EngineClient` struct (entire module gated `#[cfg(unix)]`; on Windows, fff-mcp keeps its current standalone startup path):
- Holds a `tokio::net::UnixStream` to fff-engine
- `connect(base_path: &Path) -> Result<Self>` — runs spawn-if-absent, then connects:
  1. Resolve `socket_path` and `lockfile_path` from fff-ipc `paths`
  2. If socket does not exist: attempt `O_CREAT|O_EXCL` on lockfile
     - Won: `Command::new("fff-engine").arg("--base-path").arg(base_path).arg("--frecency-db").arg(xdg_frecency_path).spawn()` as background process; await socket appearance via `lifecycle::await_ready_signal`
     - Lost: await socket appearance (another fff-mcp is spawning)
  3. `UnixStream::connect(socket_path)`
- `search(&mut self, req: SearchRequest) -> Result<SearchResponse>` — `codec::write_message` + `codec::read_message`
- `record_access(&mut self, path: &str)` — `codec::write_message(SearchRequest::RecordAccess)`, no read (KTD-5: not called, method exists for forward compatibility)

`main.rs` changes:
- Remove: `SharedFilePicker::default()`, `SharedFrecency::default()`, `FilePicker::new_with_shared_state(...)`, frecency init block
- Remove CLI flags: `--frecency-db`, `--max-cached-files`, `--content-indexing`, `--no-watch`, `--no-warmup`
- Add: `EngineClient::connect(&base_path)` call; pass client to `FffServer::new`
- Keep: `--base-path`, `--log-file`, `--log-level`, `--no-update-check`, `--healthcheck`

`server.rs` changes:
- `FffServer` replaces `SharedFilePicker` + `SharedFrecency` fields with `EngineClient`
- Each tool handler (`grep`, `find_files`, `multi_grep`): replace `picker.read()... picker.grep(...)` with `self.client.search(SearchRequest::Grep { ... })`; map `SearchResponse::SearchResults` to the existing output formatting; map `SearchResponse::Error` to `CallToolResult` error
- Existing output formatting code (`output.rs`, cursor handling) is preserved unchanged

`healthcheck.rs` rewrite (KTD-6 removes `--frecency-db` / `--history-db` from fff-mcp, breaking the current path-based checks):
- Remove checks on `args.frecency_db_path` and `args.history_db_path` (both flags gone)
- Replace with a daemon connectivity check: attempt `UnixStream::connect(socket_path)` for the derived socket path; report `ENOENT` (daemon not started) vs `ECONNREFUSED` (daemon crashed) vs success as the health signal
- `--healthcheck` flag still triggers this path; exit code semantics unchanged

**Execution note:** Rewrite tool handlers one at a time (grep → find_files → multi_grep), running existing inline tests after each to catch regressions before touching the next.

**Patterns to follow:** Current `server.rs` tool handlers — preserve all output formatting; only replace the fff-core call site with `client.search(...)`.

**Test scenarios:**
- `grep` tool call over stdio returns the same result shape as today (path + snippet + score)
- `find_files` tool call returns ranked results with frecency scores (now actually enabled via fff-engine)
- `multi_grep` with two patterns returns deduplicated results
- A `fff-mcp` started when no fff-engine is running spawns a new fff-engine process (verify process appears)
- A second `fff-mcp` started when fff-engine is already running connects without spawning a second daemon
- Healthcheck passes after startup completes
- fff-mcp `--help` no longer shows `--frecency-db` / `--max-cached-files` / `--content-indexing` flags
- `--healthcheck` with daemon running returns exit 0
- `--healthcheck` with no daemon socket present returns a non-zero exit code with a human-readable "daemon not started" message

**Verification:** `cargo test -p fff-mcp` passes; an end-to-end smoke test (MCP client → fff-mcp stdio → fff-engine socket → grep result) completes without error.

---

### U8. fff-mcp crash recovery

**Goal:** Detect fff-engine crashes at the socket level and transparently respawn and reconnect.

**Requirements:** R4

**Dependencies:** U7

**Files:**
- `crates/fff-mcp/src/recovery.rs` (new)
- `crates/fff-mcp/src/client.rs` (modified — integrate recovery into `search`)

**Approach:**

`recovery.rs` — `respawn(base_path: &Path) -> Result<EngineClient>`:
1. Read the PID from the lockfile (written by the spawner at connect time — see U7 `client.rs` spawn path below)
2. Check `kill(pid, 0)` — if the process is still alive, the daemon is starting slowly (between spawn and listen); wait with backoff rather than deleting the lockfile
3. If the PID is dead (or the lockfile is absent/unreadable): delete the stale lockfile, then retry `EngineClient::connect(base_path)` — this re-runs spawn-if-absent (`O_CREAT|O_EXCL` race; one fff-mcp wins)
4. Return new connected `EngineClient`

`client.rs` changes:
- In the spawn-if-absent path (U7), after winning `O_CREAT|O_EXCL`, write the spawned child's PID into the lockfile before awaiting the socket. This lets crash recovery distinguish a slow-starting daemon from a dead one.
- `search` wraps its inner call with a retry loop using exponential backoff (100 ms → 200 ms → 400 ms, max 3 attempts): on `Err(broken-pipe)` or `Err(ECONNREFUSED)`, call `respawn`, replace `self.stream`, retry. On third failure, return `SearchResponse::Error`.
- Backoff absorbs the window between daemon spawn and socket-ready, preventing a race where fff-mcp kills a live lockfile belonging to a daemon that hasn't bound yet.

**Test scenarios:**
- `search` on a client whose server has been killed returns a valid result after transparent recovery (stream replaced, request retried within backoff window)
- Lockfile present with a live PID (`kill(pid, 0)` succeeds) — recovery waits with backoff instead of deleting the lockfile
- Lockfile present with a dead PID — recovery deletes the stale lockfile and respawns
- If fff-engine crashes and respawn also fails (simulate with bad binary path), `search` returns `SearchResponse::Error` after 3 attempts rather than panicking or hanging
- Two concurrent fff-mcp instances both attempting respawn: exactly one spawns fff-engine (PID written to lockfile); both end up connected

**Verification:** Kill the fff-engine process mid-session; the next MCP tool call succeeds without the caller (Claude Code) seeing an error.

---

### U9. Install script + .mcp.json frecency migration

**Goal:** Update the install script and `.mcp.json` template to include fff-engine in the install path, and ensure frecency is enabled by default.

**Requirements:** R5, R6

**Dependencies:** U7

**Files:**
- `install-mcp.sh` (modified)
- `.mcp.json` (modified — root dev template)
- `README.md` (modified — update install instructions if frecency is mentioned)

**Approach:**

`install-mcp.sh`:
- Binary distribution (downloading fff-engine from GitHub releases, updating the awk asset filter, GitHub Actions (GHA) release workflow) is deferred to follow-up — see Scope Boundaries. The install script change in this track is limited to removing any stale references to removed fff-mcp flags.
- Printed `.mcp.json` template: the current template does not include `--frecency-db` (it was never in the template). No change needed here; fff-engine is spawned internally by fff-mcp.

`.mcp.json` (root dev template):
- Current `args` field is empty — no frecency flags to remove. No change needed.
- fff-engine is spawned by fff-mcp internally; `.mcp.json` does not reference it directly

`README.md`:
- If frecency documentation exists, update to reflect that frecency is now enabled by default
- Note that frecency data is stored at `$XDG_DATA_HOME/fff/frecency/` and survives daemon restarts

**Test scenarios:**
- `Test expectation: none — install script changes are distribution plumbing; verified by reading the output`

**Verification:** `bash install-mcp.sh` (dry run / local build) produces a `.mcp.json` without `--frecency-db`; both `fff-mcp` and `fff-engine` binaries are present in the install directory.

---

## Scope Boundaries

### In scope
- `fff-ipc` shared crate (U1)
- `fff-engine` new binary: state init, search handlers, socket server, lifecycle (U2–U6)
- `fff-mcp` internal proxy rewrite with crash recovery (U7–U8)
- Install script and frecency flag migration (U9)

### Deferred to Follow-Up Work
- RecordAccess write trigger (when fff-mcp calls `track_access` and for which results) — KTD-5
- fff-nvim and fff TUI daemon attachment (connect to fff-engine instead of standalone index)
- BigramFilter mmap redesign (original "follow-on" from the mmap design; still applies)
- Upstream RFC to `dmtrKovalenko/fff` — decide fork vs. contribution after implementation is stable
- fff-engine idle timeout (auto-shutdown when no clients connected for N minutes)
- install-mcp.sh binary distribution for fff-engine — current script uses an awk filter keyed on `fff-mcp-{target}`; generalising it + adding GHA release workflow changes is a distribution task separate from the daemon implementation

### Non-Goals
- Changing `fff-core`, `fff-grep`, or `fff-query-parser`
- Remote / TCP connections
- Changing fff-mcp's external MCP interface
- Windows daemon path — `#[cfg(unix)]` gates the entire daemon architecture; Windows retains the existing per-process standalone model (KTD-7)

---

## System-Wide Impact

**fff-mcp CLI surface changes:** `--frecency-db`, `--max-cached-files`, `--content-indexing`, `--no-watch`, and `--no-warmup` flags are removed. Any external scripts passing these flags to fff-mcp will break. These flags never appeared in the default `.mcp.json` template, so standard installs are unaffected. Custom configs referencing these flags need updating.

**New process in the system:** fff-engine runs as a background daemon. It must be included in OS-level process audits and in the install/uninstall surface. The binary ships alongside fff-mcp.

**Frecency now enabled by default:** All new fff-engine instances open LMDB at `$XDG_DATA_HOME/fff/frecency/` regardless of configuration. Users who relied on frecency being disabled have no opt-out in this track (add `--no-frecency` to fff-engine as a follow-on if needed).

---

## Risks and Dependencies

| Risk | Severity | Mitigation |
|---|---|---|
| `fff-engine` binary not on `$PATH` when fff-mcp tries to spawn it | High | Install script ensures co-location; fff-mcp logs a clear error if spawn fails |
| Socket file left behind after unclean shutdown → next fff-mcp blocks on stale socket | Medium | fff-engine's server.rs removes stale socket on bind; fff-mcp's crash recovery removes stale lockfile |
| LMDB `MAP_SIZE` too small on large frecency DBs | Low | LMDB resizes are handled by `heed`; existing frecency.rs behavior unchanged |
| bincode format change between versions breaks existing serialized data | Low | None: fff-ipc IPC is in-flight only (not persisted); no compatibility concern |
| `tokio::task::spawn_blocking` thread pool exhaustion under extreme concurrent load | Low | Dev tool; unlikely to see >10 concurrent sessions; Tokio's blocking pool is unbounded by default |

---

## Sources and Research

- Origin requirements document: `docs/brainstorms/2026-06-05-fff-engine-singleton-requirements.md`
- Thread-safety audit findings: confirmed in origin doc (Concurrency Model section)
- fff-mcp startup sequence: `crates/fff-mcp/src/main.rs` lines 215–276
- fff-mcp tool handlers: `crates/fff-mcp/src/server.rs` — `perform_grep`, `handle_find_files`, `multi_grep`
- fff-core public API: `crates/fff-core/src/lib.rs` re-exports; `FilePicker::grep`, `fuzzy_search`, `multi_grep`
- FrecencyTracker API: `crates/fff-core/src/dbs/frecency.rs` — `open`, `track_access`, `get_access_score`
- Install script: `install-mcp.sh` (no existing frecency flag in template — clean migration)
- External research: not needed — Tokio `UnixListener`, bincode, and parking_lot are well-established; local patterns are the primary reference
