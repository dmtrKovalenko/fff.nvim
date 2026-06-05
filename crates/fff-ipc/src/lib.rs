pub mod codec;
pub mod paths;
pub mod types;

pub use codec::{read_message, read_message_sync, write_message, write_message_sync};
pub use paths::{lockfile_path, socket_path};
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
