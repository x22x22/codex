use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::Child;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::warn;

use crate::protocol::INITIALIZE_METHOD;
use crate::protocol::INITIALIZED_METHOD;
use crate::protocol::InitializeParams;
use crate::protocol::InitializeResponse;
use crate::server_process::ExecServerLaunchCommand;
use crate::server_process::spawn_stdio_exec_server;

struct Inner {
    child: StdMutex<Option<Child>>,
    write_tx: mpsc::UnboundedSender<JSONRPCMessage>,
    pending: Mutex<HashMap<RequestId, oneshot::Sender<Result<Value, JSONRPCErrorError>>>>,
    next_request_id: AtomicI64,
    reader_task: JoinHandle<()>,
    writer_task: JoinHandle<()>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.reader_task.abort();
        self.writer_task.abort();
        if let Ok(mut child_guard) = self.child.lock()
            && let Some(child) = child_guard.as_mut()
        {
            let _ = child.start_kill();
        }
    }
}

#[derive(Clone)]
pub struct ExecServerClient {
    inner: Arc<Inner>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecServerError {
    #[error("failed to spawn exec-server: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("exec-server transport closed")]
    Closed,
    #[error("failed to serialize or deserialize exec-server JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("exec-server protocol error: {0}")]
    Protocol(String),
    #[error("exec-server rejected request ({code}): {message}")]
    Server { code: i64, message: String },
}

impl ExecServerClient {
    pub async fn spawn(command: ExecServerLaunchCommand) -> Result<Self, ExecServerError> {
        let crate::server_process::SpawnedStdioExecServer {
            child,
            stdin,
            stdout,
        } = spawn_stdio_exec_server(command)?;

        let (write_tx, mut write_rx) = mpsc::unbounded_channel::<JSONRPCMessage>();
        let writer_task = tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(message) = write_rx.recv().await {
                let encoded = match serde_json::to_vec(&message) {
                    Ok(encoded) => encoded,
                    Err(err) => {
                        warn!("failed to encode exec-server message: {err}");
                        break;
                    }
                };
                if stdin.write_all(&encoded).await.is_err() {
                    break;
                }
                if stdin.write_all(b"\n").await.is_err() {
                    break;
                }
                if stdin.flush().await.is_err() {
                    break;
                }
            }
        });

        let pending = Mutex::new(HashMap::<
            RequestId,
            oneshot::Sender<Result<Value, JSONRPCErrorError>>,
        >::new());
        let inner = Arc::new_cyclic(move |weak| {
            let weak = weak.clone();
            let reader_task = tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                loop {
                    let Some(inner) = weak.upgrade() else {
                        break;
                    };
                    match lines.next_line().await {
                        Ok(Some(line)) => {
                            if line.trim().is_empty() {
                                continue;
                            }

                            match serde_json::from_str::<JSONRPCMessage>(&line) {
                                Ok(message) => {
                                    if let Err(err) = handle_server_message(&inner, message).await {
                                        warn!("failed to handle exec-server message: {err}");
                                        break;
                                    }
                                }
                                Err(err) => {
                                    warn!("failed to parse exec-server message: {err}");
                                    break;
                                }
                            }
                        }
                        Ok(None) => break,
                        Err(err) => {
                            warn!("failed to read exec-server stdout: {err}");
                            break;
                        }
                    }
                }

                if let Some(inner) = weak.upgrade() {
                    fail_pending_requests(&inner).await;
                }
            });

            Inner {
                child: StdMutex::new(Some(child)),
                write_tx,
                pending,
                next_request_id: AtomicI64::new(1),
                reader_task,
                writer_task,
            }
        });

        let client = Self { inner };
        client
            .initialize(InitializeParams {
                client_name: "codex-core".to_string(),
            })
            .await?;
        client.send_notification(INITIALIZED_METHOD, serde_json::json!({}))?;
        Ok(client)
    }

    pub async fn initialize(
        &self,
        params: InitializeParams,
    ) -> Result<InitializeResponse, ExecServerError> {
        self.send_request(INITIALIZE_METHOD, params).await
    }

    async fn send_request<P, R>(&self, method: &str, params: P) -> Result<R, ExecServerError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let id = RequestId::Integer(self.inner.next_request_id.fetch_add(1, Ordering::SeqCst));
        let params = serde_json::to_value(params)?;
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().await.insert(id.clone(), tx);

        if let Err(err) = self
            .inner
            .write_tx
            .send(JSONRPCMessage::Request(JSONRPCRequest {
                id: id.clone(),
                method: method.to_string(),
                params: Some(params),
                trace: None,
            }))
        {
            let _ = self.inner.pending.lock().await.remove(&id);
            return Err(ExecServerError::Protocol(format!(
                "failed to queue exec-server request: {err}"
            )));
        }

        let result = rx.await.map_err(|_| ExecServerError::Closed)??;
        Ok(serde_json::from_value(result)?)
    }

    fn send_notification<P>(&self, method: &str, params: P) -> Result<(), ExecServerError>
    where
        P: Serialize,
    {
        let params = serde_json::to_value(params)?;
        self.inner
            .write_tx
            .send(JSONRPCMessage::Notification(JSONRPCNotification {
                method: method.to_string(),
                params: Some(params),
            }))
            .map_err(|err| {
                ExecServerError::Protocol(format!(
                    "failed to queue exec-server notification: {err}"
                ))
            })
    }
}

impl From<JSONRPCErrorError> for ExecServerError {
    fn from(error: JSONRPCErrorError) -> Self {
        Self::Server {
            code: error.code,
            message: error.message,
        }
    }
}

async fn handle_server_message(
    inner: &Inner,
    message: JSONRPCMessage,
) -> Result<(), ExecServerError> {
    match message {
        JSONRPCMessage::Response(JSONRPCResponse { id, result }) => {
            if let Some(tx) = inner.pending.lock().await.remove(&id) {
                let _ = tx.send(Ok(result));
            }
            Ok(())
        }
        JSONRPCMessage::Error(JSONRPCError { id, error }) => {
            if let Some(tx) = inner.pending.lock().await.remove(&id) {
                let _ = tx.send(Err(error));
                Ok(())
            } else {
                Err(ExecServerError::Server {
                    code: error.code,
                    message: error.message,
                })
            }
        }
        JSONRPCMessage::Notification(notification) => Err(ExecServerError::Protocol(format!(
            "unexpected exec-server notification: {}",
            notification.method
        ))),
        JSONRPCMessage::Request(request) => Err(ExecServerError::Protocol(format!(
            "unexpected exec-server request: {}",
            request.method
        ))),
    }
}

async fn fail_pending_requests(inner: &Inner) {
    let mut pending = inner.pending.lock().await;
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(JSONRPCErrorError {
            code: -32000,
            message: "exec-server transport closed".to_string(),
            data: None,
        }));
    }
}
