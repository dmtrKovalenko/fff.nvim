pub mod codec;
pub mod config;
pub mod lockfile;
pub mod paths;
pub mod types;

pub use codec::{read_message, read_message_sync, write_message, write_message_sync};
pub use paths::{base_path_slug, lockfile_path, socket_path, xdg_cache_dir, xdg_data_dir};
pub use types::*;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("encode error: {0}")]
    Encode(#[source] Box<bincode::ErrorKind>),
    #[error("decode error: {0}")]
    Decode(#[source] Box<bincode::ErrorKind>),
}
