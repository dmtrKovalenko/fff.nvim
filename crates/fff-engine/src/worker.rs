use std::{collections::HashMap, path::PathBuf, sync::Arc};

use fff_ipc::{
    base_path_slug,
    config::FffConfig,
    types::{SearchRequest, SearchResponse},
    worker_lockfile_path, worker_socket_path,
};
use parking_lot::{Mutex, RwLock};
use tokio::{net::UnixListener, sync::Mutex as TokioMutex};
use fff_ipc::{read_message, write_message};

use crate::state::{EffectiveArgs, EngineState};

pub struct WorkerState {
    pub index: u32,
    config: FffConfig,
    /// Loaded root states: slug → Arc<EngineState>.
    roots: Arc<RwLock<HashMap<String, Arc<EngineState>>>>,
    /// Per-slug async mutex gates concurrent init requests for the same root.
    /// Outer Mutex is sync (cheap; held only briefly to clone the inner Arc).
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

    /// Return a loaded `Arc<EngineState>` for `base_path`, initialising it on first access.
    ///
    /// Two concurrent callers for the same slug will serialize behind the slug's gate;
    /// the second caller hits the registry after the first completes init.
    pub async fn get_or_init(&self, base_path: PathBuf) -> Result<Arc<EngineState>, String> {
        let slug = base_path_slug(&base_path);

        // Fast path: slug already loaded.
        {
            let map = self.roots.read();
            if let Some(state) = map.get(&slug) {
                return Ok(Arc::clone(state));
            }
        }

        // Slow path: acquire the per-slug gate to serialize concurrent inits.
        let gate = {
            let mut gates = self.init_gates.lock();
            Arc::clone(gates.entry(slug.clone()).or_insert_with(|| Arc::new(TokioMutex::new(()))))
        };
        let _gate_guard = gate.lock().await;

        // Double-check after acquiring gate — another task may have completed init.
        {
            let map = self.roots.read();
            if let Some(state) = map.get(&slug) {
                return Ok(Arc::clone(state));
            }
        }

        // Run the blocking init off the Tokio thread pool.
        let args = EffectiveArgs {
            base_path: base_path.clone(),
            frecency_db_path: self.config.frecency.db.as_deref().map(PathBuf::from),
            no_watch: self.config.index.no_watch,
            no_warmup: self.config.index.no_warmup,
        };

        // Convert the error to String inside the closure so the return type is Send.
        let new_state = tokio::task::spawn_blocking(move || {
            crate::state::init(&args).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| format!("spawn_blocking join error: {e}"))??;

        let new_state = Arc::new(new_state);

        // Insert into registry with write-lock; init is complete before we lock.
        self.roots.write().insert(slug, Arc::clone(&new_state));

        Ok(new_state)
    }

    /// Number of currently loaded roots.
    pub fn root_count(&self) -> usize {
        self.roots.read().len()
    }

    /// Slugs of all currently loaded roots.
    pub fn root_slugs(&self) -> Vec<String> {
        self.roots.read().keys().cloned().collect()
    }
}

/// Entry point for worker mode. Binds the worker socket and serves connections.
pub async fn run(index: u32, config: FffConfig) -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = worker_socket_path(index);
    let lockfile_path = worker_lockfile_path(index);

    // Write PID lockfile so master and fffctl can probe liveness.
    if let Some(parent) = lockfile_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&lockfile_path, format!("{}\n", std::process::id()))?;

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

    // First message must be Connect; any other variant closes the connection.
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

    // Normal request/response loop for all subsequent messages.
    loop {
        let req: SearchRequest = match read_message(&mut read_half).await {
            Ok(r) => r,
            Err(_) => break,
        };

        match req {
            SearchRequest::Connect { .. } => {
                // Connect is only valid as the first message.
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
                // Fire-and-forget — no response.
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
