pub mod codec;
pub mod config;
pub mod lockfile;
pub mod paths;
pub mod routing;
pub mod types;

pub use codec::{read_message, read_message_sync, write_message, write_message_sync};
pub use paths::{
    base_path_slug, lockfile_path, log_path, master_lockfile_path, master_socket_path,
    routing_table_path, socket_path, wait_for_socket, worker_lockfile_path, worker_socket_path,
    xdg_cache_dir, xdg_data_dir, xdg_runtime_dir,
};
pub use routing::{RoutingTable, SerializableRing, WorkerEntry};
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
