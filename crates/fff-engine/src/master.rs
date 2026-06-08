use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use fff_ipc::{
    config::WorkerConfig,
    master_lockfile_path, master_socket_path, routing_table_path, worker_socket_path,
    read_message, write_message,
    routing::{RoutingTable, WorkerEntry},
    types::{MasterRequest, MasterResponse, WorkerInfo},
};
use tokio::{net::UnixListener, process::Command, sync::Mutex, time::interval};

use crate::ring::HashRing;

struct MasterState {
    config: WorkerConfig,
    exe_path: PathBuf,
    routing: Mutex<RoutingTable>,
    ring: Mutex<HashRing>,
    /// Workers spawned this session (have Child handles for try_wait monitoring).
    children: Mutex<HashMap<u32, tokio::process::Child>>,
    /// PIDs of workers adopted from routing.json (master restart — no Child handle).
    adopted_pids: Mutex<HashMap<u32, u32>>,
    /// Monotonically increasing worker index counter.
    next_index: Mutex<u32>,
}

impl MasterState {
    fn new(
        config: WorkerConfig,
        exe_path: PathBuf,
        routing: RoutingTable,
        ring: HashRing,
        next_index: u32,
        adopted_pids: HashMap<u32, u32>,
    ) -> Self {
        Self {
            config,
            exe_path,
            routing: Mutex::new(routing),
            ring: Mutex::new(ring),
            children: Mutex::new(HashMap::new()),
            adopted_pids: Mutex::new(adopted_pids),
            next_index: Mutex::new(next_index),
        }
    }

    async fn alloc_index(&self) -> u32 {
        let mut idx = self.next_index.lock().await;
        let i = *idx;
        *idx += 1;
        i
    }

    /// Spawn a new worker process and register it in the ring and routing table.
    async fn spawn_worker(&self, index: u32) -> Result<(), String> {
        let socket = worker_socket_path(index);
        let child = Command::new(&self.exe_path)
            .args(["--worker-index", &index.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("failed to spawn worker-{index}: {e}"))?;

        let pid = child.id().unwrap_or(0);

        // Wait for worker socket to appear (socket bind = readiness signal).
        wait_for_socket(&socket, Duration::from_secs(10)).await
            .map_err(|e| format!("worker-{index} socket timeout: {e}"))?;

        // Update ring (lock then release before locking routing).
        let ring_snapshot = {
            let mut ring = self.ring.lock().await;
            ring.add_worker_default(index);
            ring.to_serializable()
        };

        // Update routing table and persist.
        {
            let mut routing = self.routing.lock().await;
            routing.workers.insert(index, WorkerEntry {
                index,
                socket_path: socket.to_string_lossy().into(),
                pid,
                root_slugs: vec![],
            });
            routing.ring_state = ring_snapshot;
            if let Err(e) = routing.save(&routing_table_path()) {
                tracing::warn!("master: routing table persist failed: {e}");
            }
        }

        self.children.lock().await.insert(index, child);

        tracing::info!("master: spawned worker-{index} pid={pid}");
        Ok(())
    }

    async fn collect_worker_info(&self) -> Vec<WorkerInfo> {
        let routing = self.routing.lock().await;
        routing.workers.values().map(|e| WorkerInfo {
            index: e.index,
            socket_path: e.socket_path.clone(),
            root_slugs: e.root_slugs.clone(),
            root_count: e.root_slugs.len(),
            pid: e.pid,
        }).collect()
    }

    async fn worker_info(&self, index: u32) -> Option<WorkerInfo> {
        let routing = self.routing.lock().await;
        routing.workers.get(&index).map(|e| WorkerInfo {
            index: e.index,
            socket_path: e.socket_path.clone(),
            root_slugs: e.root_slugs.clone(),
            root_count: e.root_slugs.len(),
            pid: e.pid,
        })
    }

    /// Send SIGTERM to a worker and remove it from state.
    async fn stop_worker(&self, index: u32) {
        // Try the Child handle first (workers spawned this session).
        let child = self.children.lock().await.remove(&index);
        if let Some(mut c) = child {
            let _ = c.kill().await;
        } else {
            // Adopted worker: signal by PID.
            if let Some(&pid) = self.adopted_pids.lock().await.get(&index) {
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM); }
            }
        }
        self.adopted_pids.lock().await.remove(&index);

        // Remove from ring and routing, then persist.
        {
            let mut ring = self.ring.lock().await;
            ring.remove_worker(index);
            let ring_snapshot = ring.to_serializable();
            let mut routing = self.routing.lock().await;
            routing.workers.remove(&index);
            routing.ring_state = ring_snapshot;
            let _ = routing.save(&routing_table_path());
        }

        tracing::info!("master: stopped worker-{index}");
    }

    async fn handle_evicted_root(&self, slug: &str) {
        let mut routing = self.routing.lock().await;
        for entry in routing.workers.values_mut() {
            entry.root_slugs.retain(|s| s != slug);
        }
        let _ = routing.save(&routing_table_path());
        tracing::debug!("master: routing entry removed for evicted slug {slug}");
    }
}

/// Entry point for master mode.
pub async fn run(config: fff_ipc::config::FffConfig) -> Result<(), Box<dyn std::error::Error>> {
    let lockfile = master_lockfile_path();
    if let Some(parent) = lockfile.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // O_CREAT|O_EXCL race — exactly one process wins the master authority.
    use std::fs::OpenOptions;
    match OpenOptions::new().write(true).create_new(true).open(&lockfile) {
        Ok(_) => {}
        Err(_) => {
            // Check whether the existing lock is stale.
            if fff_ipc::lockfile::is_stale(&lockfile) {
                tracing::warn!("master: removing stale lockfile");
                let _ = std::fs::remove_file(&lockfile);
                OpenOptions::new().write(true).create_new(true).open(&lockfile)
                    .map_err(|_| "another master is already running")?;
            } else {
                tracing::info!("master: another instance is already running, exiting");
                return Ok(());
            }
        }
    }
    std::fs::write(&lockfile, format!("{}\n", std::process::id()))?;

    let exe_path = std::env::current_exe()?;
    let worker_cfg = config.worker;

    // Load routing.json and probe surviving workers.
    let rt_path = routing_table_path();
    let mut routing = RoutingTable::load(&rt_path).unwrap_or_default();
    let mut ring = HashRing::new();
    let mut adopted_pids: HashMap<u32, u32> = HashMap::new();
    let mut max_seen_index: u32 = 0;

    // Reconstruct ring from surviving workers only.
    let mut dead_indices: Vec<u32> = vec![];
    for (&idx, entry) in &routing.workers {
        max_seen_index = max_seen_index.max(idx);
        let alive = unsafe { libc::kill(entry.pid as libc::pid_t, 0) == 0 };
        if alive {
            ring.add_worker_default(idx);
            adopted_pids.insert(idx, entry.pid);
            tracing::info!("master: reconnected worker-{idx} pid={}", entry.pid);
        } else {
            dead_indices.push(idx);
            tracing::info!("master: discarded dead worker-{idx} pid={}", entry.pid);
        }
    }
    for idx in dead_indices {
        routing.workers.remove(&idx);
    }

    let surviving = routing.workers.len() as u32;
    let master_state = Arc::new(MasterState::new(
        worker_cfg.clone(),
        exe_path,
        routing,
        ring,
        max_seen_index + 1,
        adopted_pids,
    ));

    // Spawn workers to reach n_min.
    let to_spawn = worker_cfg.n_min.saturating_sub(surviving);
    for _ in 0..to_spawn {
        let index = master_state.alloc_index().await;
        if let Err(e) = master_state.spawn_worker(index).await {
            tracing::error!("master: initial spawn failed: {e}");
        }
    }

    // Bind master socket.
    let socket = master_socket_path();
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)?;
    tracing::info!("fff-engine master listening on {}", socket.display());

    // Background: poll children for crashes.
    let ms_monitor = Arc::clone(&master_state);
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(2));
        loop {
            ticker.tick().await;
            let mut children = ms_monitor.children.lock().await;
            let mut crashed: Vec<u32> = vec![];
            for (&idx, child) in children.iter_mut() {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        tracing::warn!("master: worker-{idx} exited: {status}");
                        crashed.push(idx);
                    }
                    Ok(None) => {}
                    Err(e) => tracing::error!("master: worker-{idx} try_wait: {e}"),
                }
            }
            drop(children);
            for idx in crashed {
                ms_monitor.children.lock().await.remove(&idx);
                // U6 will handle respawn; for now just remove from state.
                ms_monitor.routing.lock().await.workers.remove(&idx);
                ms_monitor.ring.lock().await.remove_worker(idx);
                tracing::warn!("master: worker-{idx} removed from routing (respawn: U6)");
            }
        }
    });

    // Main accept loop.
    let shutdown = async {
        let mut sigterm = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        )
        .expect("install SIGTERM");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => tracing::info!("master: SIGINT"),
            _ = sigterm.recv() => tracing::info!("master: SIGTERM"),
        }
    };
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _)) => {
                        let ms = Arc::clone(&master_state);
                        tokio::spawn(handle_connection(stream, ms));
                    }
                    Err(e) => tracing::error!("master: accept: {e}"),
                }
            }
            _ = &mut shutdown => break,
        }
    }

    // Propagate shutdown to all workers.
    {
        let mut children = master_state.children.lock().await;
        for (idx, mut child) in children.drain() {
            let _ = child.kill().await;
            tracing::info!("master: sent SIGTERM to worker-{idx}");
        }
    }
    {
        let adopted = master_state.adopted_pids.lock().await;
        for (&idx, &pid) in adopted.iter() {
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM); }
            tracing::info!("master: sent SIGTERM to adopted worker-{idx}");
        }
    }

    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_file(&lockfile);
    tracing::info!("master: stopped");
    Ok(())
}

async fn handle_connection(stream: tokio::net::UnixStream, ms: Arc<MasterState>) {
    let (mut read_half, mut write_half) = tokio::io::split(stream);

    let req: MasterRequest = match read_message(&mut read_half).await {
        Ok(r) => r,
        Err(_) => return,
    };

    match req {
        MasterRequest::Handshake { base_path } => {
            let ring = ms.ring.lock().await;
            let resp = match ring.assign(std::path::Path::new(&base_path)) {
                Some(index) => {
                    drop(ring);
                    let socket = worker_socket_path(index).to_string_lossy().into_owned();
                    // Add routing table entry if this is a new slug (done in U5; for now just route).
                    MasterResponse::WorkerSocket { path: socket, worker_index: index }
                }
                None => {
                    drop(ring);
                    MasterResponse::Error("no workers available".into())
                }
            };
            let _ = write_message(&mut write_half, &resp).await;
        }

        MasterRequest::RouteInfo { base_path } => {
            let ring = ms.ring.lock().await;
            let resp = match ring.assign(std::path::Path::new(&base_path)) {
                Some(index) => {
                    drop(ring);
                    if let Some(info) = ms.worker_info(index).await {
                        MasterResponse::WorkerInfo(info)
                    } else {
                        MasterResponse::Error(format!("worker-{index} not found"))
                    }
                }
                None => {
                    drop(ring);
                    MasterResponse::Error("ring is empty".into())
                }
            };
            let _ = write_message(&mut write_half, &resp).await;
        }

        MasterRequest::ListWorkers => {
            let workers = ms.collect_worker_info().await;
            let _ = write_message(&mut write_half, &MasterResponse::WorkerList { workers }).await;
        }

        MasterRequest::WorkerStatus { index } => {
            let resp = match ms.worker_info(index).await {
                Some(info) => MasterResponse::WorkerInfo(info),
                None => MasterResponse::Error(format!("worker-{index} not found")),
            };
            let _ = write_message(&mut write_half, &resp).await;
        }

        MasterRequest::StopWorker { index } => {
            ms.stop_worker(index).await;
            let _ = write_message(&mut write_half, &MasterResponse::Ack).await;
        }

        MasterRequest::EvictedRoot { slug } => {
            // Fire-and-forget: no response sent.
            ms.handle_evicted_root(&slug).await;
        }
    }
}

/// Poll until `path` exists, up to `timeout`. Returns Err on timeout.
async fn wait_for_socket(path: &std::path::Path, timeout: Duration) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut tick = interval(Duration::from_millis(50));
    loop {
        tick.tick().await;
        if path.exists() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("timeout waiting for {}", path.display()));
        }
    }
}
