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
}

pub trait DbHealthChecker {
    fn get_env(&self) -> &heed::Env;
    fn count_entries(&self) -> Result<Vec<(&'static str, u64)>>;

    fn get_health(&self) -> Result<DbHealth> {
        let env = self.get_env();

        let size = env
            .real_disk_size()
            .map_err(crate::error::Error::EnvOpen)?;
        let path = env.path().to_string_lossy().to_string();
        let entry_counts = self.count_entries()?;

        Ok(DbHealth {
            path,
            disk_size: size,
            entry_counts,
        })
    }
}
