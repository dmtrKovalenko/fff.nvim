use heed::{Database, Env, WithoutTls};
use std::path::Path;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;

use super::env_pool::{EnvSpec, SharedEnv};
use crate::error::{Error, Result};

pub(crate) fn is_map_full(err: &heed::Error) -> bool {
    matches!(err, heed::Error::Mdb(heed::MdbError::MapFull))
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DbHealthState {
    Pending = 0,
    Healthy = 1,
    Degraded = 2,
}

impl DbHealthState {
    fn from_u8(v: u8) -> Self {
        debug_assert!(v <= 2);

        match v {
            0 => Self::Pending,
            1 => Self::Healthy,
            _ => Self::Degraded,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DbHealth(Arc<AtomicU8>);

impl DbHealth {
    pub(crate) fn new() -> Self {
        Self(Arc::new(AtomicU8::new(DbHealthState::Pending as u8)))
    }

    pub(crate) fn is_healthy(&self) -> bool {
        // Pending counts as unhealthy: if the GC thread never flipped to
        // Healthy, something's wrong (deadlocked clear_stale_readers, stuck
        // writer mutex, etc.) and we want that surfaced to the user.
        DbHealthState::from_u8(self.0.load(Ordering::Acquire)) == DbHealthState::Healthy
    }

    pub(crate) fn mark_healthy(&self) {
        let _ = self.0.compare_exchange(
            DbHealthState::Pending as u8,
            DbHealthState::Healthy as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    pub(crate) fn mark_unhealthy(&self, reason: &'static str) {
        let prev = self.0.swap(DbHealthState::Degraded as u8, Ordering::AcqRel);
        if DbHealthState::from_u8(prev) != DbHealthState::Degraded {
            tracing::error!(reason, "LMDB tracker marked unhealthy");
        }
    }
}

/// Spawns a background thread that is ensuring that the environment that was previously
/// open is safe, accessible and doesn't have a corrupted lock.md file. If it does this thread will
/// hang indefinitely but we will have the information that the database is in failure mode
pub(crate) fn spawn_lmdb_gc<T: LmdbStore>(shared: Arc<RwLock<Option<T>>>) {
    let thread_shared = shared.clone();
    let spawn_result = thread::Builder::new()
        .name("fff-lmdb-gc".into())
        .spawn(move || {
            // Holding a read guard blocks `destroy` / re-init's write
            // guard until this thread finishes — natural serialization.
            let guard = match thread_shared.read() {
                Ok(g) => g,
                Err(e) => {
                    tracing::debug!("gc: read lock poisoned: {e}");
                    return;
                }
            };
            let Some(ref tracker) = *guard else {
                return; // destroyed before we started
            };
            // Trackers attaching to an already-pooled env must not repeat the
            // GC; the first opener's run flips the shared health flag.
            if !tracker.shared_env().try_start_gc() {
                return;
            }

            if let Err(e) = T::purge_stale_data(tracker.shared_env()) {
                tracing::debug!("purge_stale_data failed: {e}");
            }

            tracker.health().mark_healthy();
        });

    if let Err(e) = spawn_result {
        tracing::debug!(?e, "failed to spawn fff-lmdb-gc thread");
        // No thread = mark healthy now so healthcheck isn't stuck Pending.
        if let Ok(guard) = shared.read()
            && let Some(ref tracker) = *guard
        {
            tracker.health().mark_healthy();
        }
    }
}

pub(crate) trait LmdbStore: Sized + Send + Sync + 'static {
    /// Short label used to defferintiate different instances of this trait
    const LABEL: &'static str;
    /// LMDB map size in bytes. Must be a multiple of the OS page size.
    const MAP_SIZE: usize;
    /// Number of named sub-databases. `0` for single-db envs.
    const MAX_DBS: u32;
    /// Hard cap on `data.mdb` size.
    const SIZE_CAP_BYTES: u64;

    /// Borrow the pooled env handle shared by every tracker of this path.
    fn shared_env(&self) -> &SharedEnv;
    /// Borrow the health flag from the tracker.
    fn health(&self) -> &DbHealth;

    /// Borrow the raw heed env.
    fn env(&self) -> &Env<WithoutTls> {
        self.shared_env()
    }

    /// Override to purge stale rows, compact, etc. Default no-op. Runs on
    /// the GC thread while a read lock is held against the shared handle,
    /// so destroy / re-init naturally wait for it.
    fn purge_stale_data(_env: &SharedEnv) -> Result<()> {
        Ok(())
    }

    /// Open (or join) the process-shared LMDB env for `db_path`. The health
    /// flag is per-env: the GC of the first opener flips it for everyone.
    #[tracing::instrument]
    fn open_env(db_path: &Path) -> Result<(SharedEnv, DbHealth)> {
        let shared = SharedEnv::get_or_open(
            db_path,
            &EnvSpec {
                label: Self::LABEL,
                map_size: Self::MAP_SIZE,
                max_dbs: Self::MAX_DBS,
                size_cap_bytes: Self::SIZE_CAP_BYTES,
            },
        )?;
        let health = shared.health().clone();
        Ok((shared, health))
    }

    /// Open or create a database without blocking on the LMDB writer mutex
    /// when the database already exists.
    fn open_database_safe<KC, DC>(env: &SharedEnv, name: Option<&str>) -> Result<Database<KC, DC>>
    where
        KC: 'static,
        DC: 'static,
    {
        let db = Self::LABEL;
        // mdb_dbi_open must not run from concurrent txns in this process.
        let _dbi_guard = env.lock_dbi_open();

        let rtxn = env
            .read_txn()
            .map_err(|source| Error::DbStartReadTxn { db, source })?;
        let maybe_db: Option<Database<KC, DC>> = env
            .open_database(&rtxn, name)
            .map_err(|source| Error::DbOpen { db, source })?;

        // do not drop the DB here
        rtxn.commit()
            .map_err(|source| Error::DbCommit { db, source })?;

        match maybe_db {
            Some(handle) => Ok(handle),
            None => {
                // First time: create the database (requires write lock).
                // unfortunately this CAN be deadlocking and this is what we see happens
                // if the other part of the code is segfaulting, so the only rule to prevent this
                // write the good code mf, okay?
                let mut wtxn = env
                    .write_txn()
                    .map_err(|source| Error::DbStartWriteTxn { db, source })?;
                let handle = env
                    .create_database(&mut wtxn, name)
                    .map_err(|source| Error::DbCreate { db, source })?;

                wtxn.commit()
                    .map_err(|source| Error::DbCommit { db, source })?;
                Ok(handle)
            }
        }
    }
}
