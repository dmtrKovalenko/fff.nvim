use std::path::PathBuf;
use std::sync::Arc;

use fff_ipc::types::SearchRequest;
use fff_ipc::{read_message, write_message};
use tokio::net::UnixListener;

use crate::state::EngineState;

/// Bind a UnixListener at `socket_path` and accept connections until SIGTERM/SIGINT.
pub async fn run(state: Arc<EngineState>, socket_path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Remove any stale socket left by a previous crash.
    let _ = std::fs::remove_file(&socket_path);

    let listener = UnixListener::bind(&socket_path)?;
    tracing::info!("fff-engine listening on {}", socket_path.display());

    let shutdown = async {
        tokio::signal::ctrl_c().await.ok();
    };
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _)) => {
                        let state_clone = Arc::clone(&state);
                        tokio::spawn(handle_connection(stream, state_clone));
                    }
                    Err(e) => {
                        tracing::error!("Accept error: {e}");
                    }
                }
            }
            _ = &mut shutdown => {
                tracing::info!("Shutdown signal received");
                break;
            }
        }
    }

    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}

async fn handle_connection(stream: tokio::net::UnixStream, state: Arc<EngineState>) {
    let (mut read_half, mut write_half) = tokio::io::split(stream);

    loop {
        let request: SearchRequest = match read_message(&mut read_half).await {
            Ok(req) => req,
            Err(_) => break, // EOF or broken pipe — client disconnected
        };

        match request {
            SearchRequest::RecordAccess { .. } => {
                // KTD-5: fire-and-forget, no response sent
            }
            req => {
                let response = dispatch(&state, req).await;
                if write_message(&mut write_half, &response).await.is_err() {
                    break;
                }
            }
        }
    }
}

async fn dispatch(state: &EngineState, req: SearchRequest) -> fff_ipc::types::SearchResponse {
    use crate::handlers::{handle_find_files, handle_grep, handle_multi_grep};

    match req {
        SearchRequest::Grep { query, options } => handle_grep(state, query, options).await,
        SearchRequest::FindFiles { query, options } => handle_find_files(state, query, options).await,
        SearchRequest::MultiGrep { patterns, constraints, options } => handle_multi_grep(state, patterns, constraints, options).await,
        SearchRequest::RecordAccess { .. } => unreachable!("handled before dispatch"),
    }
}
