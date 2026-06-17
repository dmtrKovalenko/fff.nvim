use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::RwLock;

use fff::{FFFMode, FilePicker, FilePickerOptions, SharedFilePicker, SharedFrecency};

/// Max concurrent directory indexes the daemon will keep alive.
pub const MAX_SESSIONS: usize = 8;
/// Sessions idle longer than this are evicted.
const IDLE_TIMEOUT: Duration = Duration::from_mins(10);
/// How often the evictor thread checks for idle sessions.
const EVICTION_INTERVAL: Duration = Duration::from_secs(60);

struct Session {
    picker: SharedFilePicker,
    #[allow(dead_code)]
    frecency: SharedFrecency,
    last_accessed: Instant,
}

struct Inner {
    sessions: RwLock<HashMap<PathBuf, Session>>,
}

/// Signals the evictor thread to wake up and exit.
struct Shutdown {
    mu: Mutex<bool>,
    cv: Condvar,
}

impl Shutdown {
    fn new() -> Self {
        Self { mu: Mutex::new(false), cv: Condvar::new() }
    }

    fn trigger(&self) {
        *self.mu.lock().unwrap() = true;
        self.cv.notify_all();
    }

    /// Sleeps for `dur`, returning `true` immediately if shutdown was signaled.
    fn wait(&self, dur: Duration) -> bool {
        let guard = self.mu.lock().unwrap();
        if *guard {
            return true;
        }
        let (guard, _) = self.cv.wait_timeout(guard, dur).unwrap();
        *guard
    }
}

pub struct SessionPool {
    inner: Arc<Inner>,
    shutdown: Arc<Shutdown>,
    _evictor: JoinHandle<()>,
}

impl SessionPool {
    pub fn new() -> Self {
        let inner = Arc::new(Inner { sessions: RwLock::new(HashMap::new()) });
        let shutdown = Arc::new(Shutdown::new());

        let evictor_inner = inner.clone();
        let evictor_shutdown = shutdown.clone();
        let evictor = std::thread::Builder::new()
            .name("session-evictor".into())
            .spawn(move || {
                while !evictor_shutdown.wait(EVICTION_INTERVAL) {
                    let evicted = evictor_inner.evict_idle();
                    if evicted > 0 {
                        tracing::debug!(
                            evicted,
                            remaining = evictor_inner.session_count(),
                            "eviction sweep"
                        );
                    }
                }
            })
            .expect("failed to spawn evictor thread");

        Self { inner, shutdown, _evictor: evictor }
    }

    pub fn shutdown(&self) {
        self.shutdown.trigger();
    }

    pub fn get_or_create(&self, path: &Path) -> Result<SharedFilePicker, fff::Error> {
        self.inner.get_or_create(path)
    }
}

impl Inner {
    #[tracing::instrument(level = "trace", skip(self), fields(path = %path.display()))]
    fn get_or_create(&self, path: &Path) -> Result<SharedFilePicker, fff::Error> {
        let canonical =
            dunce::canonicalize(path).map_err(|_| fff::Error::InvalidPath(path.to_path_buf()))?;

        let mut sessions = self.sessions.write();

        if let Some(session) = sessions.get_mut(&canonical) {
            session.last_accessed = Instant::now();
            return Ok(session.picker.clone());
        }

        if sessions.len() >= MAX_SESSIONS {
            let lru_key =
                sessions.iter().min_by_key(|(_, s)| s.last_accessed).map(|(k, _)| k.clone());
            if let Some(key) = lru_key {
                tracing::debug!(path = %key.display(), "evicting LRU session (pool full)");
                sessions.remove(&key);
            }
        }

        let picker = SharedFilePicker::default();
        let frecency = SharedFrecency::default();

        FilePicker::new_with_shared_state(
            picker.clone(),
            frecency.clone(),
            FilePickerOptions {
                base_path: canonical.to_string_lossy().into_owned(),
                enable_mmap_cache: false,
                enable_content_indexing: true,
                mode: FFFMode::Ai,
                watch: true,
                follow_symlinks: false,
                ..Default::default()
            },
        )?;

        sessions.insert(
            canonical,
            Session { picker: picker.clone(), frecency, last_accessed: Instant::now() },
        );

        Ok(picker)
    }

    fn evict_idle(&self) -> usize {
        let mut sessions = self.sessions.write();
        let before = sessions.len();
        sessions.retain(|path, session| {
            let keep = session.last_accessed.elapsed() < IDLE_TIMEOUT;
            if !keep {
                tracing::debug!(path = %path.display(), idle_secs = session.last_accessed.elapsed().as_secs(), "evicting idle session");
            }
            keep
        });
        before - sessions.len()
    }

    fn session_count(&self) -> usize {
        self.sessions.read().len()
    }
}
