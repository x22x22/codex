use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::Mutex as AsyncMutex;

use crate::protocol::ExecOutputStream;
use crate::protocol::ExecParams;
use crate::protocol::ReadParams;
use crate::protocol::ReadResponse;
use crate::protocol::WriteResponse;
use crate::{
    ExecServerClient, ExecServerClientConnectOptions, ExecServerError, RemoteExecServerConnectArgs,
};

#[derive(Debug, Error)]
pub enum ExecutorError {
    #[error("executor is disabled in this environment: {reason}")]
    Disabled { reason: String },

    #[error("exec-server transport error: {0}")]
    ExecServer(#[from] ExecServerError),
}

/// Request to spawn a new backend-managed process session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecRequest {
    pub process_id: String,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub tty: bool,
    pub arg0: Option<String>,
}

impl From<ExecRequest> for ExecParams {
    fn from(request: ExecRequest) -> Self {
        Self {
            process_id: request.process_id,
            argv: request.argv,
            cwd: request.cwd,
            env: request.env,
            tty: request.tty,
            arg0: request.arg0,
        }
    }
}

/// Request for incremental reads from a spawned process session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecReadRequest {
    pub after_seq: Option<u64>,
    pub max_bytes: Option<usize>,
    pub wait_ms: Option<u64>,
}

/// Output chunk returned by `ExecSession::read`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutputChunk {
    pub seq: u64,
    pub stream: ExecOutputStream,
    pub chunk: Vec<u8>,
}

/// Incremental read response for a spawned process session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecReadResponse {
    pub chunks: Vec<ExecOutputChunk>,
    pub next_seq: u64,
    pub exited: bool,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecWriteResponse {
    pub accepted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecTerminateResponse {
    pub running: bool,
}

/// A backend-managed process session.
///
/// This is intentionally lower-level than core's unified-exec process model so
/// tools and skills can adopt Environment-backed execution incrementally.
#[async_trait]
pub trait ExecSession: Send + Sync {
    fn process_id(&self) -> &str;

    async fn read(&self, request: ExecReadRequest) -> Result<ExecReadResponse, ExecutorError>;

    async fn write(&self, chunk: Vec<u8>) -> Result<ExecWriteResponse, ExecutorError>;

    async fn terminate(&self) -> Result<ExecTerminateResponse, ExecutorError>;
}

/// Execution backend exposed through `Environment`.
#[async_trait]
pub trait Executor: Send + Sync {
    async fn spawn(&self, request: ExecRequest) -> Result<Box<dyn ExecSession>, ExecutorError>;
}

#[derive(Clone)]
pub struct LocalExecutor {
    client: Arc<AsyncMutex<Option<Arc<ExecServerClient>>>>,
}

#[derive(Clone)]
pub struct RemoteExecutor {
    client: Arc<ExecServerClient>,
}

#[derive(Clone)]
struct ExecServerExecSession {
    process_id: String,
    client: Arc<ExecServerClient>,
}

impl LocalExecutor {
    pub const CLIENT_NAME: &str = "codex-core";

    pub fn new() -> Self {
        Self {
            client: Arc::new(AsyncMutex::new(None)),
        }
    }

    pub fn from_client(client: Arc<ExecServerClient>) -> Self {
        Self {
            client: Arc::new(AsyncMutex::new(Some(client))),
        }
    }

    async fn client(&self) -> Result<Arc<ExecServerClient>, ExecutorError> {
        {
            let lock = self.client.lock().await;
            if let Some(client) = &*lock {
                return Ok(Arc::clone(client));
            }
        }

        let connected = ExecServerClient::connect_in_process(ExecServerClientConnectOptions {
            client_name: Self::CLIENT_NAME.to_string(),
            ..ExecServerClientConnectOptions::default()
        })
        .await?;

        let connected = Arc::new(connected);
        let mut lock = self.client.lock().await;
        if let Some(existing) = &*lock {
            return Ok(Arc::clone(existing));
        }

        *lock = Some(Arc::clone(&connected));
        Ok(connected)
    }
}

impl RemoteExecutor {
    pub const CLIENT_NAME: &str = "codex-core";

    pub fn from_client(client: Arc<ExecServerClient>) -> Self {
        Self { client }
    }

    pub async fn connect(url: String) -> Result<Self, ExecutorError> {
        let client = ExecServerClient::connect_websocket(RemoteExecServerConnectArgs::new(
            url,
            Self::CLIENT_NAME.to_string(),
        ))
        .await?;
        Ok(Self {
            client: Arc::new(client),
        })
    }

    pub fn client(&self) -> Arc<ExecServerClient> {
        Arc::clone(&self.client)
    }
}

#[async_trait]
impl Executor for LocalExecutor {
    async fn spawn(&self, request: ExecRequest) -> Result<Box<dyn ExecSession>, ExecutorError> {
        let client = self.client().await?;
        let response = client.exec(request.into()).await?;
        Ok(Box::new(ExecServerExecSession {
            process_id: response.process_id,
            client,
        }))
    }
}

#[async_trait]
impl Executor for RemoteExecutor {
    async fn spawn(&self, request: ExecRequest) -> Result<Box<dyn ExecSession>, ExecutorError> {
        let response = self.client.exec(request.into()).await?;
        Ok(Box::new(ExecServerExecSession {
            process_id: response.process_id,
            client: Arc::clone(&self.client),
        }))
    }
}

#[async_trait]
impl ExecSession for ExecServerExecSession {
    fn process_id(&self) -> &str {
        &self.process_id
    }

    async fn read(&self, request: ExecReadRequest) -> Result<ExecReadResponse, ExecutorError> {
        let response = self
            .client
            .read(ReadParams {
                process_id: self.process_id.clone(),
                after_seq: request.after_seq,
                max_bytes: request.max_bytes,
                wait_ms: request.wait_ms,
            })
            .await?;
        Ok(response_to_read(response))
    }

    async fn write(&self, chunk: Vec<u8>) -> Result<ExecWriteResponse, ExecutorError> {
        let response: WriteResponse = self.client.write(&self.process_id, chunk).await?;
        Ok(ExecWriteResponse {
            accepted: response.accepted,
        })
    }

    async fn terminate(&self) -> Result<ExecTerminateResponse, ExecutorError> {
        let response = self.client.terminate(&self.process_id).await?;
        Ok(ExecTerminateResponse {
            running: response.running,
        })
    }
}

fn response_to_read(response: ReadResponse) -> ExecReadResponse {
    ExecReadResponse {
        chunks: response
            .chunks
            .into_iter()
            .map(|chunk| ExecOutputChunk {
                seq: chunk.seq,
                stream: chunk.stream,
                chunk: chunk.chunk.into_inner(),
            })
            .collect(),
        next_seq: response.next_seq,
        exited: response.exited,
        exit_code: response.exit_code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_request_into_protocol_request_maps_all_fields() {
        let request = ExecRequest {
            process_id: "proc-1".to_string(),
            argv: vec!["/bin/echo".to_string(), "hello".to_string()],
            cwd: PathBuf::from("/tmp"),
            env: HashMap::from([(String::from("A"), String::from("B"))]),
            tty: false,
            arg0: Some("echo".to_string()),
        };

        let protocol_request = ExecParams::from(request);

        assert_eq!(protocol_request.process_id, "proc-1");
        assert_eq!(
            protocol_request.argv,
            vec!["/bin/echo".to_string(), "hello".to_string()]
        );
        assert_eq!(protocol_request.cwd, PathBuf::from("/tmp"));
        assert_eq!(
            protocol_request.env,
            HashMap::from([(String::from("A"), String::from("B"))])
        );
        assert_eq!(protocol_request.tty, false);
        assert_eq!(protocol_request.arg0, Some("echo".to_string()));
    }

    #[test]
    fn read_response_maps_chunks_to_public_shape() {
        let response = ReadResponse {
            chunks: vec![crate::protocol::ProcessOutputChunk {
                seq: 1,
                stream: crate::protocol::ExecOutputStream::Stdout,
                chunk: vec![b'a', b'b'].into(),
            }],
            next_seq: 7,
            exited: true,
            exit_code: Some(0),
        };

        let public_response = response_to_read(response);

        assert_eq!(
            public_response.chunks,
            vec![ExecOutputChunk {
                seq: 1,
                stream: ExecOutputStream::Stdout,
                chunk: b"ab".to_vec(),
            }]
        );
        assert_eq!(public_response.next_seq, 7);
        assert!(public_response.exited);
        assert_eq!(public_response.exit_code, Some(0));
    }
}
