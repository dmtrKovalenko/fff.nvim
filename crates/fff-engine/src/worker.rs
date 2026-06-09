use std::{
    collections::HashMap,
    fs::OpenOptions,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use fff_ipc::{
    base_path_slug, master_socket_path,
    config::FffConfig,
    types::{MasterRequest, SearchRequest, SearchResponse},
    worker_lockfile_path, worker_socket_path,
    write_message_sync,
};
use parking_lot::{Mutex, RwLock};
use tokio::{net::UnixListener, sync::Mutex as TokioMutex};
use fff_ipc::{read_message, write_message};

use crate::state::{EffectiveArgs, EngineState};

struct RootEntry {
    state: Arc<EngineState>,
    // Milliseconds since Unix epoch; updated atomically on every access.
    // Allows fast-path reads to hold roots.read() instead of roots.write().
    last_access_ms: AtomicU64,
}

pub(crate) struct WorkerState {
    pub index: u32,
    config: FffConfig,
    roots: Arc<RwLock<HashMap<String, RootEntry>>>,
    // Per-slug async mutex serialises concurrent inits for the same root.
    // Outer Mutex is sync (held only briefly to clone the inner Arc).
    init_gates: Mutex<HashMap<String, Arc<TokioMutex<()>>>>,
}

impl WorkerState {
    pub fn new(index: u32, config: FffConfig) -> Self {
        Self {
            index,
            config,
            roots: Arc::new(RwLock::new(HashMap::new())),
            init_gates: Mutex::new(HashMap::new()),
        }
    }

    // Return a loaded `Arc<EngineState>` for `base_path`, initialising it on first access.
    // Two concurrent callers for the same slug serialise behind the slug's gate;
    // the second caller hits the registry after the first completes init.
    pub async fn get_or_init(&self, base_path: PathBuf) -> Result<Arc<EngineState>, String> {
        let slug = base_path_slug(&base_path);
        let max_roots = self.config.worker.roots_per_worker_max as usize;
        let now = now_ms();

        // Fast path: slug loaded — update last_access atomically (read lock only).
        {
            let map = self.roots.read();
            if let Some(entry) = map.get(&slug) {
                entry.last_access_ms.store(now, Ordering::Relaxed);
                return Ok(Arc::clone(&entry.state));
            }
        }

        // Slow path: gate serialises concurrent inits for the same slug.
        let gate = {
            let mut gates = self.init_gates.lock();
            Arc::clone(gates.entry(slug.clone()).or_insert_with(|| Arc::new(TokioMutex::new(()))))
        };
        let _gate_guard = gate.lock().await;

        // Double-check after acquiring gate.
        {
            let map = self.roots.read();
            if let Some(entry) = map.get(&slug) {
                entry.last_access_ms.store(now, Ordering::Relaxed);
                return Ok(Arc::clone(&entry.state));
            }
        }

        if self.roots.read().len() >= max_roots {
            self.evict_lru().await;
        }

        let args = EffectiveArgs {
            base_path: base_path.clone(),
            frecency_db_path: self.config.frecency.db.as_deref().map(PathBuf::from),
            no_watch: self.config.index.no_watch,
            no_warmup: self.config.index.no_warmup,
        };

        // Convert error to String inside closure so the return type is Send.
        let new_state = tokio::task::spawn_blocking(move || {
            crate::state::init(&args).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| format!("spawn_blocking join error: {e}"))??;

        let new_state = Arc::new(new_state);
        self.roots.write().insert(slug, RootEntry {
            state: Arc::clone(&new_state),
            last_access_ms: AtomicU64::new(now_ms()),
        });

        Ok(new_state)
    }

    // Evict the LRU root with no active connections (Arc::strong_count == 1).
    // Roots with live connections are skipped; if none are evictable the new
    // root loads anyway as a temporary overflow.
    async fn evict_lru(&self) {
        let victim = {
            let map = self.roots.read();
            map.iter()
                .filter(|(_, e)| Arc::strong_count(&e.state) == 1)
                .min_by_key(|(_, e)| e.last_access_ms.load(Ordering::Relaxed))
                .map(|(slug, _)| slug.clone())
        };

        if let Some(slug) = victim {
            self.roots.write().remove(&slug);
            tracing::debug!("worker-{}: evicted root {slug}", self.index);
            self.notify_evicted(slug).await;
        }
    }

    // Fire-and-forget EvictedRoot to master socket.
    // Uses spawn_blocking because std::os::unix::net::UnixStream::connect is blocking.
    // Failure is benign — idle TTL will clean up the routing entry.
    async fn notify_evicted(&self, slug: String) {
        let master = master_socket_path();
        let msg = MasterRequest::EvictedRoot { slug };
        tokio::task::spawn_blocking(move || {
            use std::net::Shutdown;
            use std::os::unix::net::UnixStream;
            if let Ok(mut stream) = UnixStream::connect(&master) {
                let _ = write_message_sync(&mut stream, &msg);
                let _ = stream.shutdown(Shutdown::Both);
            }
        });
    }
}

/// Entry point for worker mode. Binds the worker socket and serves connections.
pub async fn run(index: u32, config: FffConfig) -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = worker_socket_path(index);
    let lockfile_path = worker_lockfile_path(index);

    if let Some(parent) = lockfile_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Use O_CREAT|O_EXCL so two concurrent workers with the same index cannot
    // both overwrite each other's PID (unlike plain std::fs::write).
    match OpenOptions::new().write(true).create_new(true).open(&lockfile_path) {
        Ok(_) => {
            std::fs::write(&lockfile_path, format!("{}\n", std::process::id()))?;
        }
        Err(_) => {
            if fff_ipc::lockfile::is_stale(&lockfile_path) {
                let _ = std::fs::remove_file(&lockfile_path);
                std::fs::write(&lockfile_path, format!("{}\n", std::process::id()))?;
            } else {
                tracing::info!("worker-{index}: another instance already running, exiting");
                return Ok(());
            }
        }
    }

    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(&socket_path);

    let listener = UnixListener::bind(&socket_path)?;
    tracing::info!("fff-engine worker-{index} listening on {}", socket_path.display());

    let worker_state = Arc::new(WorkerState::new(index, config));

    let shutdown = async {
        let mut sigterm = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        )
        .expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => tracing::info!("worker-{index} SIGINT"),
            _ = sigterm.recv() => tracing::info!("worker-{index} SIGTERM"),
        }
    };
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _)) => {
                        let ws = Arc::clone(&worker_state);
                        tokio::spawn(handle_worker_connection(stream, ws));
                    }
                    Err(e) => tracing::error!("worker-{index} accept error: {e}"),
                }
            }
            _ = &mut shutdown => break,
        }
    }

    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&lockfile_path);
    tracing::info!("fff-engine worker-{index} stopped");
    Ok(())
}

async fn handle_worker_connection(stream: tokio::net::UnixStream, ws: Arc<WorkerState>) {
    let (mut read_half, mut write_half) = tokio::io::split(stream);

    let base_path = match read_message(&mut read_half).await {
        Ok(SearchRequest::Connect { base_path }) => PathBuf::from(base_path),
        Ok(other) => {
            tracing::warn!("worker-{}: unexpected first message {:?}, closing", ws.index, other);
            return;
        }
        Err(_) => return,
    };

    let state = match ws.get_or_init(base_path).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("worker-{}: state init failed: {e}", ws.index);
            let _ = write_message(&mut write_half, &SearchResponse::Error(e)).await;
            return;
        }
    };

    if write_message(&mut write_half, &SearchResponse::Ack).await.is_err() {
        return;
    }

    loop {
        let req: SearchRequest = match read_message(&mut read_half).await {
            Ok(r) => r,
            Err(_) => break,
        };

        match req {
            SearchRequest::Connect { .. } => {
                let _ = write_message(
                    &mut write_half,
                    &SearchResponse::Error("unexpected Connect after handshake".into()),
                ).await;
                break;
            }
            SearchRequest::RecordAccess { path } => {
                let frecency = state.shared_frecency.clone();
                let base = state.base_path.clone();
                tokio::task::spawn_blocking(move || {
                    let abs_path = if std::path::Path::new(&path).is_absolute() {
                        PathBuf::from(&path)
                    } else {
                        base.join(&path)
                    };
                    if let Ok(guard) = frecency.read()
                        && let Some(tracker) = guard.as_ref()
                    {
                        if let Err(e) = tracker.track_access(&abs_path) {
                            tracing::warn!(?abs_path, "RecordAccess failed: {e}");
                        }
                    }
                });
            }
            SearchRequest::SetLogLevel { level } => {
                let response = match crate::set_log_level(&level) {
                    Ok(()) => SearchResponse::Ack,
                    Err(e) => SearchResponse::Error(e),
                };
                if write_message(&mut write_half, &response).await.is_err() {
                    break;
                }
            }
            req => {
                let response = crate::server::dispatch_request(&state, req).await;
                if write_message(&mut write_half, &response).await.is_err() {
                    break;
                }
            }
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
