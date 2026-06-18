use crate::error::Result;

/// Health information about a database
#[derive(Debug, Clone)]
pub struct DbHealth {
    /// Path to the database file
    pub path: String,
    /// Size on disk in bytes
    pub disk_size: u64,
    /// Entry counts by table name
    pub entry_counts: Vec<(&'static str, u64)>,
    /// Set to `false` if can not acquire the write lock
    pub healthy: bool,
}

pub trait DbHealthChecker {
    fn get_env(&self) -> &heed::Env;
    fn is_healthy(&self) -> bool;
    /// Entries per database, each group has a static string label
    fn count_entries(&self) -> Result<Vec<(&'static str, u64)>>;

    /// Health summary of the database, returns summary struct
    fn get_health(&self) -> Result<DbHealth> {
        let env = self.get_env();

        let size = env
            .real_disk_size()
            .map_err(crate::error::Error::GenericDbError)?;
        let path = env.path().to_string_lossy().to_string();
        let entry_counts = self.count_entries()?;

        Ok(DbHealth {
            path,
            disk_size: size,
            entry_counts,
            healthy: self.is_healthy(),
        })
    }
}
