# fff-engine Worker Model — Requirements

**Date:** 2026-06-08
**Status:** Draft
**Topic:** Nginx-style master + sharded worker process model for fff-engine

---

## Problem

The current singleton `fff-engine` model (one process per project root) is functionally sound but has three structural gaps as the number of concurrently active roots grows:

1. **No global resource cap.** N active roots = N independent engine processes, each with its own Rayon thread pool and full memory footprint. There is no knob to bound total machine impact.
2. **No crash isolation between roots.** Each engine is its own failure domain today, but there is no shared lifecycle layer — `fffctl` must know and address every engine socket independently.
3. **No CPU fairness across roots.** Each engine's Rayon pool competes with others via OS scheduling only. A heavy grep on root-A has no principled limit relative to root-B.

---

## Goal

Introduce a **master + worker process model** for `fff-engine` where:

- A single master process (`fff-engine-master`) is the well-known entry point for all `fff-mcp` connections
- N worker processes handle search traffic, each owning a shard of the root namespace
- Workers scale out dynamically up to a configured maximum
- Master downtime is a brief blip for new connections only; existing sessions are unaffected

**Primary outcomes:**
- One operational endpoint for `fffctl` to manage (master socket)
- OS-level crash isolation per shard — a crashed worker affects only its assigned roots
- Global worker cap (N_max) as a machine-level resource control knob
- Worker-level Rayon pools with bounded count; OS scheduler fairness follows naturally

---

## Non-Goals

- Cross-process sharing of `BigramFilter` or `FilePicker` (requires mmap redesign — separate track)
- Live migration of active connections between workers
- Zero-downtime master restart (R2/R3 resilience — over-engineered for a developer tool)
- Remote (non-Unix-socket) worker connections
- Changes to `fff-core`, `fff-grep`, or `fff-query-parser`
- Changing the external interface seen by Claude Code (`fff-mcp` is unchanged)

---

## Architecture

### Components

**`fff-engine-master`** — new binary or subcommand of `fff-engine`

A thin supervisor and router. Owns no search state. Responsibilities:
- Accept initial `fff-mcp` connections on a well-known Unix socket
- Maintain a routing table: `root_id → worker_socket_path`
- Assign roots to workers via consistent hashing
- Spawn new workers when existing ones are full (root count ≥ threshold)
- Monitor worker health; restart crashed workers
- Persist routing table to disk after every mutation for crash recovery

**`fff-engine-worker`** — N instances (existing `fff-engine` binary, worker mode flag)

Each worker is an OS process owning a shard of the root namespace:
- Holds `FilePicker`, `BigramFilter`, `FrecencyTracker`, FS watcher for each assigned root
- Listens on its own Unix socket (`worker-{index}.sock`)
- Handles direct long-lived connections from `fff-mcp` clients after handoff from master
- Self-reports root count and liveness to master (heartbeat or on-demand query)

**`fff-mcp`** — externally unchanged; two-phase connection startup

1. Connects to master socket, sends `{ base_path }` handshake
2. Receives `{ worker_socket_path }` in response
3. Closes master connection; opens a direct connection to the worker socket
4. All search traffic thereafter: `fff-mcp ↔ worker` (master not in the hot path)

### Topology

```
fff-mcp startup:
  ① connect to master.sock
  ② send: { base_path: "/projects/alpha" }
  ③ receive: { worker_socket: "worker-2.sock" }
  ④ connect directly to worker-2.sock
  ⑤ [master connection closed]

Search traffic (long-lived):
  fff-mcp ──── worker-2.sock ──── Worker-2 [owns alpha, delta, ...]
```

Multiple sessions on the same root:

```
fff-mcp #1 ──┐
fff-mcp #2 ──┤──► Worker-2  [root: alpha (warm), root: delta (warm)]
fff-mcp #3 ──┘

fff-mcp #4 ──────► Worker-0  [root: beta (warm)]
```

### Shard Assignment

Roots are assigned to workers via **consistent hashing** on `hash(canonical_root_path)`:

- Each worker is placed at one or more points on a hash ring (virtual nodes)
- A root always maps to the same worker for a given ring configuration
- When a new worker is spawned, it claims a contiguous arc of the ring; only unoccupied roots (no active connections) from adjacent workers are migrated. Active connections are never interrupted.
- Hash ring state is owned by master and persisted alongside the routing table

### Dynamic Scale-Out

1. Master tracks root count per worker
2. When any worker's root count reaches `roots_per_worker_max` (config, default: 8), master spawns a new worker process (up to `N_max`)
3. New root assignments flow to the new worker; existing assignments are stable
4. If `N_max` is reached and all workers are at capacity, master LRU-evicts the oldest idle root from the least-loaded worker to make room. A root is "idle" when no `fff-mcp` connection is currently using it (worker tracks active connection count per root)
5. When a worker has zero assigned roots for longer than `worker_idle_ttl` (config, default: 5 min), master terminates it to reclaim resources

### Master Crash Recovery (R1)

- Master persists its full routing table (worker socket paths + root assignments + consistent hash ring state) to `$XDG_RUNTIME_DIR/fff/routing.json` after every mutation
- On master crash, `fff-mcp` (attempting the initial handshake) notices `ECONNREFUSED` and races via `O_CREAT|O_EXCL` on `$XDG_RUNTIME_DIR/fff/master.lock` to respawn master — identical race mechanism to today's engine spawn race
- Respawned master reads `routing.json`, pings each worker socket to confirm liveness, rebuilds in-memory routing table, and begins accepting connections — target recovery time < 500 ms
- Existing `fff-mcp ↔ worker` connections are completely unaffected during master downtime

### Worker Crash Recovery

- Master monitors worker processes (via `waitpid` or equivalent)
- On worker crash: master marks its assigned roots as "unavailable" in the routing table
- `fff-mcp` clients connected to the crashed worker receive broken pipe; they re-connect to master
- Master routes them to a replacement worker (spawns one if needed), which cold-starts the affected roots
- Cold start time: same as today's engine startup for that root (~1–5 s for large repos)

---

## Configuration

| Key | Default | Description |
|---|---|---|
| `worker.n_min` | 1 | Workers spawned at startup |
| `worker.n_max` | 4 | Hard cap on worker count |
| `worker.roots_per_worker_max` | 8 | Roots per worker before scale-out |
| `worker.idle_ttl_secs` | 300 | Worker auto-shutdown if idle this long |

---

## Wire Protocol Changes

`fff-ipc` gains a new message type for the master handshake:

```rust
enum MasterRequest {
    Handshake { base_path: String },
}

enum MasterResponse {
    WorkerSocket { path: String },
    Error(String),
}
```

The existing `SearchRequest` / `SearchResponse` protocol on worker sockets is unchanged.

---

## Scope of Changes

| Component | Change |
|---|---|
| `fff-engine` | Extended with `--worker-mode` flag; master subcommand or sibling binary added |
| `fff-ipc` | New `MasterRequest` / `MasterResponse` types; routing table serialization |
| `fff-mcp` | Updated startup: two-phase handshake instead of direct engine connect |
| `fff-core`, `fff-grep`, `fff-query-parser` | Unchanged |
| `fff-ctl` | Updated to target master socket; gains `list-workers`, `worker-status` commands |
| Install script / `.mcp.json` | Updated: master socket path instead of per-root socket path |

---

## Open Questions

- **fff-mcp hash shortcut**: should fff-mcp learn the hash ring from master and compute worker assignment locally (eliminating the handshake on reconnect)? Trades one roundtrip for coupling to ring topology.
- **"Full" threshold**: root count is a simple proxy for memory. A better threshold might be estimated BigramFilter size (sum of indexed file content). Worth exploring once baseline is stable.
- **fffctl interaction**: should `fffctl stop` stop the master (which drains workers) or allow stopping individual workers?
- **fff-nvim / fff TUI integration**: these attach to `fff-engine` directly today. They need a parallel update to go through master handshake. Defer or include?
- **Upstream RFC**: does this design belong in the upstream `dmtrKovalenko/fff` or stay as a downstream fork feature?
