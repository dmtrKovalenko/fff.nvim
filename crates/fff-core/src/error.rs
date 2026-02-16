use std::path::StripPrefixError;

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum Error {
    #[error("Thread panicked")]
    ThreadPanic,
    #[error("Invalid path {0}")]
    InvalidPath(std::path::PathBuf),
    #[error("File picker not initialized")]
    FilePickerMissing,
    #[error("Failed to acquire lock for frecency")]
    AcquireFrecencyLock,
    #[error("Failed to acquire lock for items by provider")]
    AcquireItemLock,
    #[error("Failed to acquire lock for path cache")]
    AcquirePathCacheLock,
    #[error("Failed to create directory: {0}")]
    CreateDir(#[from] std::io::Error),
    #[error("Failed to open frecency database env: {0}")]
    EnvOpen(#[source] heed::Error),
    #[error("Failed to create frecency database: {0}")]
    DbCreate(#[source] heed::Error),
    #[error("Failed to clear stale readers for frecency database: {0}")]
    DbClearStaleReaders(#[source] heed::Error),

    #[error("Failed to start read transaction for frecency database: {0}")]
    DbStartReadTxn(#[source] heed::Error),
    #[error("Failed to start write transaction for frecency database: {0}")]
    DbStartWriteTxn(#[source] heed::Error),

    #[error("Failed to read from frecency database: {0}")]
    DbRead(#[source] heed::Error),
    #[error("Failed to write to frecency database: {0}")]
    DbWrite(#[source] heed::Error),
    #[error("Failed to commit write transaction to frecency database: {0}")]
    DbCommit(#[source] heed::Error),
    #[error("Failed to start file system watcher: {0}")]
    FileSystemWatch(#[from] notify::Error),

    #[error("Expected a path to be child of another path: {0}")]
    StripPrefixError(#[from] StripPrefixError),

    #[error("libgit2 error occurred: {0}")]
    Git(#[from] git2::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
