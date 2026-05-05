use std::fs;
use std::path::Path;

use heed::{Env, EnvOpenOptions};

use crate::error::{Error, Result};

pub(crate) fn is_map_full(err: &heed::Error) -> bool {
    matches!(err, heed::Error::Mdb(heed::MdbError::MapFull))
}

pub(crate) trait LmdbStore {
    /// LMDB map size in bytes. Must be a multiple of the OS page size.
    const MAP_SIZE: usize;
    /// Number of named sub-databases. `0` for single-db envs.
    const MAX_DBS: u32;
    /// Hard cap on `data.mdb` size.
    const SIZE_CAP_BYTES: u64;

    #[tracing::instrument]
    fn open_env(db_path: &Path) -> Result<Env> {
        Self::erase_if_oversized(db_path);
        fs::create_dir_all(db_path).map_err(Error::CreateDir)?;

        let env = unsafe {
            let mut opts = EnvOpenOptions::new();
            opts.map_size(Self::MAP_SIZE);
            if Self::MAX_DBS > 0 {
                opts.max_dbs(Self::MAX_DBS);
            }
            opts.open(db_path).map_err(Error::EnvOpen)?
        };

        env.clear_stale_readers()
            .map_err(Error::DbClearStaleReaders)?;

        Ok(env)
    }

    fn erase_if_oversized(db_path: &Path) {
        let data = db_path.join("data.mdb");
        let Ok(meta) = fs::metadata(&data) else {
            return;
        };
        if meta.len() <= Self::SIZE_CAP_BYTES {
            return;
        }

        tracing::error!(
            path = %db_path.display(),
            size = meta.len(),
            cap = Self::SIZE_CAP_BYTES,
            "LMDB db exceeds size cap, erasing"
        );
        let _ = fs::remove_file(&data);
        let _ = fs::remove_file(db_path.join("lock.mdb"));
    }
}
