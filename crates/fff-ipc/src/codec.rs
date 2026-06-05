use std::io::{Read, Write};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::IpcError;

// ── Sync codec (used by fff-mcp's blocking EngineClient) ─────────────────────

/// Synchronous write of a length-prefixed bincode message.
pub fn write_message_sync<W: Write, T: Serialize>(
    writer: &mut W,
    value: &T,
) -> Result<(), IpcError> {
    let payload = bincode::serialize(value).map_err(IpcError::Encode)?;
    let len = payload.len() as u32;
    writer.write_all(&len.to_le_bytes()).map_err(IpcError::Io)?;
    writer.write_all(&payload).map_err(IpcError::Io)?;
    Ok(())
}

/// Synchronous read of a length-prefixed bincode message.
pub fn read_message_sync<R: Read, T: for<'de> Deserialize<'de>>(
    reader: &mut R,
) -> Result<T, IpcError> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).map_err(IpcError::Io)?;
    let len = u32::from_le_bytes(len_buf) as usize;

    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).map_err(IpcError::Io)?;

    bincode::deserialize(&payload).map_err(IpcError::Decode)
}

/// Write a length-prefixed bincode message.
///
/// Frame layout: `[ 4-byte LE u32 payload length ][ payload bytes ]`
pub async fn write_message<W, T>(writer: &mut W, value: &T) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = bincode::serialize(value).map_err(IpcError::Encode)?;
    let len = payload.len() as u32;
    writer
        .write_all(&len.to_le_bytes())
        .await
        .map_err(IpcError::Io)?;
    writer.write_all(&payload).await.map_err(IpcError::Io)?;
    Ok(())
}

/// Read a length-prefixed bincode message.
///
/// Returns `Err(IpcError::Io)` wrapping `UnexpectedEof` when the stream ends
/// before a full frame is received (truncated data or clean EOF mid-frame).
pub async fn read_message<R, T>(reader: &mut R) -> Result<T, IpcError>
where
    R: AsyncRead + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut len_buf = [0u8; 4];
    reader
        .read_exact(&mut len_buf)
        .await
        .map_err(IpcError::Io)?;
    let len = u32::from_le_bytes(len_buf) as usize;

    let mut payload = vec![0u8; len];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(IpcError::Io)?;

    bincode::deserialize(&payload).map_err(IpcError::Decode)
}

// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::io;
    use tokio::io::duplex;

    use super::*;
    use crate::types::{GrepOptions, SearchRequest, SearchResponse};

    #[tokio::test]
    async fn grep_request_round_trips() {
        let (mut client, mut server) = duplex(4096);
        let req = SearchRequest::Grep {
            query: "héllo wörld".into(),
            options: GrepOptions::default(),
        };
        write_message(&mut client, &req).await.unwrap();
        drop(client); // flush EOF so server read completes

        let rt: SearchRequest = read_message(&mut server).await.unwrap();
        match rt {
            SearchRequest::Grep { query, .. } => assert_eq!(query, "héllo wörld"),
            _ => panic!("wrong variant"),
        }
    }

    #[tokio::test]
    async fn empty_search_results_round_trips() {
        let (mut client, mut server) = duplex(4096);
        let resp = SearchResponse::SearchResults(vec![]);
        write_message(&mut client, &resp).await.unwrap();
        drop(client);

        let rt: SearchResponse = read_message(&mut server).await.unwrap();
        match rt {
            SearchResponse::SearchResults(v) => assert!(v.is_empty()),
            _ => panic!("wrong variant"),
        }
    }

    #[tokio::test]
    async fn error_response_round_trips() {
        let (mut client, mut server) = duplex(4096);
        write_message(&mut client, &SearchResponse::Error("oops".into()))
            .await
            .unwrap();
        drop(client);

        let rt: SearchResponse = read_message(&mut server).await.unwrap();
        match rt {
            SearchResponse::Error(msg) => assert_eq!(msg, "oops"),
            _ => panic!("wrong variant"),
        }
    }

    #[tokio::test]
    async fn truncated_stream_returns_error() {
        let (mut client, mut server) = duplex(4096);
        // Write only a partial length prefix (2 bytes instead of 4)
        client.write_all(&[0u8, 1u8]).await.unwrap();
        drop(client);

        let result: Result<SearchResponse, _> = read_message(&mut server).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            IpcError::Io(e) => assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof),
            other => panic!("expected Io error, got {other:?}"),
        }
    }
}
