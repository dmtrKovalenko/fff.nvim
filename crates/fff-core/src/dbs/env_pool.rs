use heed::{Env, EnvOpenOptions, WithoutTls};
use std::collections::HashMap;
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError, Weak};
use std::thread;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::lmdb::DbHealth;

pub(crate) struct EnvSpec {
    pub label: &'static str,
    pub map_size: usize,
    pub max_dbs: u32,
    pub size_cap_bytes: u64,
}

pub(crate) struct PooledEnv {
    env: Env<WithoutTls>,
    key: PathBuf,
    /// lmdb's env spec label
    label: &'static str,
    map_size: usize,
    max_dbs: u32,
    health: DbHealth,
    gc_started: AtomicBool,
    dbi_lock: Mutex<()>,
}

impl Drop for PooledEnv {
    fn drop(&mut self) {
        let mut pool = POOL.lock().unwrap_or_else(PoisonError::into_inner);
        // Only remove a dead entry: begin_exclusive_destroy may have removed ours.
        if pool.get(&self.key).is_some_and(|w| w.strong_count() == 0) {
            pool.remove(&self.key);
        }
        // heed closes the env right after this body; a concurrent reopen of the
        // same path rides out that gap via env_closing_event in get_or_open.
    }
}

// Cloneable handle to a process-shared LMDB env, derefs to `heed::Env`.
#[derive(Clone)]
pub(crate) struct SharedEnv(Arc<PooledEnv>);

impl Deref for SharedEnv {
    type Target = Env<WithoutTls>;
    fn deref(&self) -> &Env<WithoutTls> {
        &self.0.env
    }
}

impl std::fmt::Debug for SharedEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SharedEnv").field(&self.0.env).finish()
    }
}

impl SharedEnv {
    pub(crate) fn get_or_open(db_path: &Path, spec: &EnvSpec) -> Result<Self> {
        fs::create_dir_all(db_path).map_err(Error::CreateDir)?;
        let path = fs::canonicalize(db_path).map_err(|e| Error::EnvOpen {
            db: spec.label,
            source: heed::Error::Io(e),
        })?;

        let mut close_waits = 0u32;
        let mut transient_retries = 0u32;

        loop {
            let mut open_failed = false;

            {
                let mut pool = POOL.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(existing) = pool.get(&path).and_then(Weak::upgrade) {
                    drop(pool);
                    if existing.label != spec.label
                        || existing.map_size != spec.map_size
                        || existing.max_dbs != spec.max_dbs
                    {
                        return Err(Error::EnvSpecMismatch {
                            path,
                            open_as: existing.label,
                            requested_as: spec.label,
                        });
                    }
                    return Ok(Self(existing));
                }

                erase_if_oversized(&path, spec);
                let result = unsafe {
                    // MDB_NOTLS: reader slots are tied to txn objects (freed on
                    // commit/abort) instead of pinned per thread for its lifetime (#783).
                    let mut opts = EnvOpenOptions::new().read_txn_without_tls();
                    opts.map_size(spec.map_size);
                    opts.max_readers(max_readers());
                    if spec.max_dbs > 0 {
                        opts.max_dbs(spec.max_dbs);
                    }
                    opts.open(&path)
                };

                match result {
                    Ok(env) => {
                        let entry = Arc::new(PooledEnv {
                            env,
                            key: path.clone(),
                            label: spec.label,
                            map_size: spec.map_size,
                            max_dbs: spec.max_dbs,
                            health: DbHealth::new(),
                            gc_started: AtomicBool::new(false),
                            dbi_lock: Mutex::new(()),
                        });
                        pool.insert(path.clone(), Arc::downgrade(&entry));
                        drop(pool);
                        let shared = Self(entry);

                        match shared.clear_stale_readers() {
                            Ok(cleared_count) if cleared_count > 0 => {
                                tracing::info!(
                                    cleared_count,
                                    db = spec.label,
                                    "reclaimed stale LMDB reader slots at open"
                                );
                            }
                            Ok(_) => {}
                            Err(e) => {
                                tracing::debug!("clear_stale_readers at open failed: {e}")
                            }
                        }

                        return Ok(shared);
                    }
                    Err(heed::Error::EnvAlreadyOpened) => open_failed = true,
                    // special handling cause we know this happens randomly
                    Err(e)
                        if is_transient_env_open_error(&e)
                            && transient_retries < MAX_TRANSIENT_RETRIES =>
                    {
                        transient_retries += 1;
                        tracing::debug!(
                            path = %path.display(),
                            transient_retries,
                            error = ?e,
                            "transient LMDB env open error, retrying"
                        );
                    }
                    Err(e) => {
                        return Err(Error::EnvOpen {
                            db: spec.label,
                            source: e,
                        });
                    }
                }
            }

            if open_failed {
                close_waits += 1;
                if close_waits > MAX_CLOSE_WAITS {
                    return Err(Error::EnvOpen {
                        db: spec.label,
                        source: heed::Error::EnvAlreadyOpened,
                    });
                }

                match heed::env_closing_event(&path) {
                    Some(event) => {
                        event.wait_timeout(CLOSE_WAIT);
                    }
                    None => thread::sleep(Duration::from_millis(2)),
                }
            } else {
                thread::sleep(TRANSIENT_RETRY_SLEEP);
            }
        }
    }

    pub(crate) fn health(&self) -> &DbHealth {
        &self.0.health
    }

    // First caller wins: GC runs once per opened env, not once per tracker.
    pub(crate) fn try_start_gc(&self) -> bool {
        !self.0.gc_started.swap(true, Ordering::AcqRel)
    }

    // LMDB forbids mdb_dbi_open from concurrent txns in the same process.
    pub(crate) fn lock_dbi_open(&self) -> MutexGuard<'_, ()> {
        self.0
            .dbi_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn destroy(&self) -> Result<Option<heed::EnvClosingEvent>> {
        let mut pool = POOL.lock().unwrap_or_else(PoisonError::into_inner);
        let holders = Arc::strong_count(&self.0);

        if holders > 1 {
            return Err(Error::DbInUse {
                db: self.0.label,
                path: self.0.key.clone(),
                holders: holders - 1,
            });
        }

        pool.remove(&self.0.key);
        Ok(heed::env_closing_event(&self.0.key))
    }
}

static POOL: LazyLock<Mutex<HashMap<PathBuf, Weak<PooledEnv>>>> = LazyLock::new(Mutex::default);

const CLOSE_WAIT: Duration = Duration::from_millis(100);
const MAX_CLOSE_WAITS: u32 = 100;
const TRANSIENT_RETRY_SLEEP: Duration = Duration::from_millis(50);
const MAX_TRANSIENT_RETRIES: u32 = 8;

// Concurrent mdb_env_open calls on the same path can race on macOS
// this is for some reason fixable by simple retry of the open
// heed's default reader table is 126 slots. In TLS mode each thread pins a slot
// for its lifetime, so long-lived embedders (Neovim, node agents) that share one
// lock file across many processes/threads exhaust it (#783). Reader slots are
// tiny (~64B), so raise the ceiling; `FFF_LMDB_MAX_READERS` lets hosts tune it.
const DEFAULT_MAX_READERS: u32 = 1024;

fn max_readers() -> u32 {
    parse_max_readers(std::env::var("FFF_LMDB_MAX_READERS").ok())
}

// Never drop below heed's default 126; ignore missing/garbage/too-small values.
fn parse_max_readers(raw: Option<String>) -> u32 {
    raw.and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|&n| n >= 126)
        .unwrap_or(DEFAULT_MAX_READERS)
}

fn is_transient_env_open_error(err: &heed::Error) -> bool {
    match err {
        heed::Error::Io(io) => matches!(
            io.kind(),
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::NotFound
        ),
        _ => false,
    }
}

fn erase_if_oversized(db_path: &Path, spec: &EnvSpec) {
    let data = db_path.join("data.mdb");
    let Ok(meta) = fs::metadata(&data) else {
        return;
    };

    if meta.len() <= spec.size_cap_bytes {
        return;
    }

    tracing::error!(
        path = %db_path.display(),
        size = meta.len(),
        cap = spec.size_cap_bytes,
        "LMDB db exceeds size cap, erasing"
    );
    let _ = fs::remove_file(&data);
    let _ = fs::remove_file(db_path.join("lock.mdb"));
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_MAX_READERS, parse_max_readers};

    #[test]
    fn max_readers_parsing() {
        assert_eq!(parse_max_readers(None), DEFAULT_MAX_READERS);
        assert_eq!(parse_max_readers(Some("nan".into())), DEFAULT_MAX_READERS);
        assert_eq!(parse_max_readers(Some("64".into())), DEFAULT_MAX_READERS); // below 126 floor
        assert_eq!(parse_max_readers(Some(" 512 ".into())), 512);
        assert_eq!(parse_max_readers(Some("126".into())), 126);
    }
}
