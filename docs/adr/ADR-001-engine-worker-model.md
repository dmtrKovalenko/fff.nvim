---
section: "Architecture"
type: adr
status: proposed
audience: internal
tags: [fff-engine, architecture, ipc, concurrency]
---

# ADR-001: fff-engine Worker Model — Master + Sharded OS-Process Workers

**Date:** 2026-06-08
**Status:** Proposed
**Deciders:** Abhijit Salvi

## Context

The current `fff-engine` model spawns one OS process per project root. Each engine owns its own `FilePicker`, `BigramFilter`, `FrecencyTracker`, and Rayon thread pool. While this model is clean and isolation is good per-root, three structural gaps emerge as the number of concurrently active roots grows:

1. **No global resource cap.** N active roots = N independent engine processes, each with its own Rayon pool and full memory footprint. There is no knob to bound total machine-level impact.
2. **Fragmented lifecycle.** `fffctl` must address each engine socket independently. There is no single entry point for observability, restart, or shutdown across all roots.
3. **No CPU fairness across roots.** Each engine's Rayon pool competes with others via OS scheduler only — a heavy grep on root-A has no principled limit relative to root-B.

An nginx-style (NGINX: event-driven asynchronous web server) master + worker model was explored as a structural response to all three gaps. The long-lived connection nature of `fff-mcp` sessions (lasting the lifetime of a Claude Code session, potentially hours) was the key constraint that shaped which variant of the model to adopt.

A fungible worker pool (any worker serves any root) breaks down for long-lived connections: if N=3 workers are each occupied by an active `fff-mcp` session, a fourth root has no slot. Shard-based assignment — where each worker owns a stable subset of the root namespace — solves this by making assignment stable for the connection lifetime.

## Decision

We will adopt a **master + sharded OS-process worker model**:

- A single `fff-engine-master` process is the well-known entry point for all `fff-mcp` connections. It is a thin router only — it holds no search state and is never in the per-request hot path.
- N worker processes (`fff-engine-worker`) each own a shard of the root namespace, assigned via consistent hashing on `canonical_root_path`. A root always maps to the same worker for a given ring configuration.
- `fff-mcp` connects to master for a two-phase handshake: master returns the worker socket address, and `fff-mcp` opens a direct long-lived connection to that worker. Master is not involved in any subsequent search traffic.
- Workers scale out dynamically up to `N_max` (configurable). When a worker's assigned root count reaches `roots_per_worker_max`, master spawns a new worker. The consistent hash ring expands; only unoccupied roots from adjacent ring arcs migrate. Active connections are never interrupted.
- Master persists its routing table (worker sockets + ring state) to `$XDG_RUNTIME_DIR/fff/routing.json` on every mutation. On crash, an `fff-mcp` client races via `O_CREAT|O_EXCL` to respawn master, which reconstructs state from the file and pings surviving workers. Recovery target: < 500 ms. Existing `fff-mcp ↔ worker` direct connections are unaffected during master downtime.

## Consequences

### Positive

- **OS-level crash isolation per shard.** A crashed worker affects only its assigned roots; other sessions are unaffected.
- **Single operational entry point.** `fffctl` targets the master socket for lifecycle operations (stop-all, list-workers, worker-status) instead of N per-root sockets.
- **Global resource cap.** `N_max` is a machine-level knob bounding both process count and total Rayon thread pool count.
- **Master never blocks search.** The two-phase handshake offloads master from the hot path entirely; search latency is identical to the current direct-socket model.
- **LRU-safe eviction.** Idle roots (no active connections) can be evicted from workers without disrupting any session.

### Negative / trade-offs

- **Cold start on eviction or worker crash.** When a worker restarts or a root is evicted, `BigramFilter` and `FilePicker` must be rebuilt from scratch (1–5 s for large repos). Same cost as today's engine startup, but now visible to users who were warm.
- **Shard imbalance.** Consistent hashing distributes roots probabilistically. A shard with 3 large repos and 1 small one will have a heavier memory footprint than a shard with 4 small repos. No live rebalancing of active connections.
- **`BigramFilter` stays per-worker-heap.** Cross-process state sharing requires the mmap redesign that was explicitly deferred (see `docs/brainstorms/2026-06-05-fff-engine-singleton-requirements.md`). Each worker independently holds the full BigramFilter for its roots.
- **`fff-mcp` connection startup gains a master roundtrip.** The two-phase handshake adds one Unix socket roundtrip at session start (~0.1 ms). Negligible in practice.

### Neutral

- `fff-core`, `fff-grep`, and `fff-query-parser` are unchanged.
- External interface of `fff-mcp` as seen by Claude Code is unchanged.
- `fff-nvim` and `fff` TUI need a parallel update to use the master handshake. This is a follow-on task, not in scope for the initial implementation.

## Alternatives Considered

### Alternative A: Fungible worker pool (in-process tasks)

Single `fff-engine` OS process; workers are Tokio tasks sharing a common LRU root-state pool (`Arc<Mutex<LruCache<RootId, Arc<RootState>>>>`). The pool caps hot roots in memory rather than capping connections. One Rayon pool provides true CPU fairness.

Rejected primarily because it offers no OS-level crash isolation — a panic in one task kills all roots simultaneously. Also, Tokio tasks are not "workers" in any operational sense, making the model harder to reason about for capacity planning.

### Alternative B: Supervisor + per-root dedicated workers

Current per-root engines, plus a master supervisor process that spawns, monitors, and restarts them. Each worker is dedicated to exactly one root (always warm, no affinity problem). Resource cap = cap on active root count.

This captures the lifecycle management gain but does not address CPU fairness (each worker has its own uncapped Rayon pool) or establish any global thread budget. It is a smaller step that could be a Phase 0 milestone before the full sharded model.

### Alternative C: Current model (no change)

Retain one process per root. Operational overhead addressed via `fffctl` improvements.

Rejected because it does not scale to many concurrent roots and provides no global resource control surface.
