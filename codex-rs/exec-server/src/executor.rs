use std::collections::HashMap;
use std::fmt;

use async_trait::async_trait;
use codex_utils_absolute_path::AbsolutePathBuf;
use tokio::sync::Mutex;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tokio::time::sleep;

use crate::ExecServerClient;
use crate::client_api::ExecServerClientConnectOptions;
use crate::protocol::ExecOutputStream;
use crate::protocol::ExecParams;
use crate::protocol::ReadParams;

#[derive(Clone, Debug)]
pub struct ExecSpawnRequest {
    pub process_id: String,
    pub argv: Vec<String>,
    pub cwd: AbsolutePathBuf,
    pub env: HashMap<String, String>,
    pub arg0: Option<String>,
    pub tty: bool,
    pub sandbox: SandboxKind,
    pub inherited_fds: Vec<InheritedFd>,
}

#[derive(Clone, Debug)]
pub enum ExecOutputEvent {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecExit {
    pub exit_code: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SandboxKind {
    None,
    MacosSeatbelt,
    LinuxSeccomp,
    WindowsRestrictedToken,
}

#[derive(Clone, Debug)]
pub struct InheritedFd {
    pub target_fd: i32,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("failed to spawn process: {0}")]
    Spawn(String),
    #[error("failed to write to process: {0}")]
    Write(String),
    #[error("failed to resize process pty: {0}")]
    Resize(String),
    #[error("failed to terminate process: {0}")]
    Terminate(String),
    #[error("executor transport failure: {0}")]
    Transport(String),
}

#[async_trait]
pub trait Executor: std::fmt::Debug + Send + Sync {
    async fn spawn(&self, request: ExecSpawnRequest) -> Result<Box<dyn ExecSession>, ExecError>;
}

#[async_trait]
pub trait ExecSession: std::fmt::Debug + Send + Sync {
    fn process_id(&self) -> &str;

    fn subscribe_output(&self) -> broadcast::Receiver<ExecOutputEvent>;

    async fn write(&self, chunk: Vec<u8>) -> Result<(), ExecError>;

    async fn resize(&self, _size: PtySize) -> Result<(), ExecError> {
        Err(ExecError::Resize(
            "resize is not supported by exec-server sessions".to_string(),
        ))
    }

    async fn terminate(&self) -> Result<(), ExecError>;

    async fn wait(&self) -> Result<ExecExit, ExecError>;

    fn try_exit_status(&self) -> Option<ExecExit>;
}

pub struct LocalExecutor {
    client: Mutex<Option<ExecServerClient>>,
}

impl fmt::Debug for LocalExecutor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalExecutor").finish()
    }
}

pub struct RemoteExecutor {
    client: ExecServerClient,
}

impl fmt::Debug for RemoteExecutor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteExecutor").finish()
    }
}

#[derive(Debug)]
pub struct LocalExecSession {
    inner: ExecServerExecSession,
}

#[derive(Debug)]
pub struct RemoteExecSession {
    inner: ExecServerExecSession,
}

struct ExecServerExecSession {
    process_id: String,
    client: ExecServerClient,
    output_tx: broadcast::Sender<ExecOutputEvent>,
    exit_status: watch::Sender<Option<ExecExit>>,
    output_task: JoinHandle<()>,
}

impl fmt::Debug for ExecServerExecSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecServerExecSession")
            .field("process_id", &self.process_id)
            .finish()
    }
}

impl Default for LocalExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalExecutor {
    pub fn new() -> Self {
        Self {
            client: Mutex::new(None),
        }
    }

    async fn client(&self) -> Result<ExecServerClient, ExecError> {
        let mut client_guard = self.client.lock().await;
        if let Some(client) = client_guard.clone() {
            return Ok(client);
        }

        let client =
            ExecServerClient::connect_in_process(ExecServerClientConnectOptions::default())
                .await
                .map_err(|err| ExecError::Spawn(err.to_string()))?;
        *client_guard = Some(client.clone());
        Ok(client)
    }
}

#[async_trait]
impl Executor for LocalExecutor {
    async fn spawn(&self, request: ExecSpawnRequest) -> Result<Box<dyn ExecSession>, ExecError> {
        validate_request(&request)?;
        let client = self.client().await?;
        let response = client
            .exec(request_to_exec_params(request))
            .await
            .map_err(|err| ExecError::Spawn(format!("failed to spawn process: {err}")))?;

        Ok(Box::new(LocalExecSession {
            inner: ExecServerExecSession::new(response.process_id, client),
        }))
    }
}

impl RemoteExecutor {
    pub fn new(client: ExecServerClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Executor for RemoteExecutor {
    async fn spawn(&self, request: ExecSpawnRequest) -> Result<Box<dyn ExecSession>, ExecError> {
        validate_request(&request)?;
        let response = self
            .client
            .exec(request_to_exec_params(request))
            .await
            .map_err(|err| ExecError::Spawn(format!("failed to spawn process: {err}")))?;

        Ok(Box::new(RemoteExecSession {
            inner: ExecServerExecSession::new(response.process_id, self.client.clone()),
        }))
    }
}

#[async_trait]
impl ExecSession for LocalExecSession {
    fn process_id(&self) -> &str {
        self.inner.process_id()
    }

    fn subscribe_output(&self) -> broadcast::Receiver<ExecOutputEvent> {
        self.inner.subscribe_output()
    }

    async fn write(&self, chunk: Vec<u8>) -> Result<(), ExecError> {
        self.inner.write(chunk).await
    }

    async fn terminate(&self) -> Result<(), ExecError> {
        self.inner.terminate().await
    }

    async fn wait(&self) -> Result<ExecExit, ExecError> {
        self.inner.wait().await
    }

    fn try_exit_status(&self) -> Option<ExecExit> {
        self.inner.try_exit_status()
    }
}

#[async_trait]
impl ExecSession for RemoteExecSession {
    fn process_id(&self) -> &str {
        self.inner.process_id()
    }

    fn subscribe_output(&self) -> broadcast::Receiver<ExecOutputEvent> {
        self.inner.subscribe_output()
    }

    async fn write(&self, chunk: Vec<u8>) -> Result<(), ExecError> {
        self.inner.write(chunk).await
    }

    async fn terminate(&self) -> Result<(), ExecError> {
        self.inner.terminate().await
    }

    async fn wait(&self) -> Result<ExecExit, ExecError> {
        self.inner.wait().await
    }

    fn try_exit_status(&self) -> Option<ExecExit> {
        self.inner.try_exit_status()
    }
}

impl ExecServerExecSession {
    fn new(process_id: String, client: ExecServerClient) -> Self {
        let (output_tx, _) = broadcast::channel(128);
        let (exit_status, _) = watch::channel(None);
        let mut events = client.event_receiver();

        let process_id_clone = process_id.clone();
        let output_tx_clone = output_tx.clone();
        let exit_status_clone = exit_status.clone();
        let output_task = tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => match event {
                        crate::client_api::ExecServerEvent::OutputDelta(notification) => {
                            if notification.process_id != process_id_clone {
                                continue;
                            }
                            let chunk = notification.chunk.into_inner();
                            let stream = match notification.stream {
                                ExecOutputStream::Stdout => ExecOutputEvent::Stdout(chunk),
                                ExecOutputStream::Stderr => ExecOutputEvent::Stderr(chunk),
                                ExecOutputStream::Pty => ExecOutputEvent::Stdout(chunk),
                            };
                            let _ = output_tx_clone.send(stream);
                        }
                        crate::client_api::ExecServerEvent::Exited(notification) => {
                            if notification.process_id == process_id_clone {
                                let _ = exit_status_clone.send_replace(Some(ExecExit {
                                    exit_code: notification.exit_code,
                                }));
                                break;
                            }
                        }
                    },
                    Err(RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });

        Self {
            process_id,
            client,
            output_tx,
            exit_status,
            output_task,
        }
    }

    fn process_id(&self) -> &str {
        &self.process_id
    }

    fn subscribe_output(&self) -> broadcast::Receiver<ExecOutputEvent> {
        self.output_tx.subscribe()
    }

    async fn write(&self, chunk: Vec<u8>) -> Result<(), ExecError> {
        self.client
            .write(&self.process_id, chunk)
            .await
            .map_err(|err| ExecError::Write(format!("failed to write to process: {err}")))?;
        Ok(())
    }

    async fn terminate(&self) -> Result<(), ExecError> {
        self.client
            .terminate(&self.process_id)
            .await
            .map_err(|err| ExecError::Terminate(format!("failed to terminate process: {err}")))?;
        Ok(())
    }

    async fn wait(&self) -> Result<ExecExit, ExecError> {
        let mut exit_receiver = self.exit_status.subscribe();
        if let Some(status) = *exit_receiver.borrow() {
            return Ok(status);
        }

        loop {
            match exit_receiver.changed().await {
                Ok(()) => {
                    if let Some(status) = *exit_receiver.borrow() {
                        return Ok(status);
                    }
                }
                Err(_) => {
                    return self.wait_for_read_exit().await.map_err(|err| {
                        ExecError::Transport(format!("failed to wait for process: {err}"))
                    });
                }
            }
        }
    }

    async fn wait_for_read_exit(&self) -> Result<ExecExit, crate::ExecServerError> {
        loop {
            let response = self
                .client
                .read(ReadParams {
                    process_id: self.process_id.clone(),
                    after_seq: None,
                    max_bytes: Some(0),
                    wait_ms: Some(50),
                })
                .await?;
            if response.exited {
                return Ok(ExecExit {
                    exit_code: response.exit_code.unwrap_or(-1),
                });
            }
            sleep(Duration::from_millis(25)).await;
        }
    }

    fn try_exit_status(&self) -> Option<ExecExit> {
        *self.exit_status.borrow()
    }
}

impl Drop for ExecServerExecSession {
    fn drop(&mut self) {
        self.output_task.abort();
    }
}

fn request_to_exec_params(request: ExecSpawnRequest) -> ExecParams {
    ExecParams {
        process_id: request.process_id,
        argv: request.argv,
        cwd: request.cwd.into(),
        env: request.env,
        tty: request.tty,
        arg0: request.arg0,
    }
}

fn validate_request(request: &ExecSpawnRequest) -> Result<(), ExecError> {
    if request.sandbox != SandboxKind::None {
        return Err(ExecError::Spawn(format!(
            "sandbox policy {:?} is not supported by this executor",
            request.sandbox
        )));
    }
    if !request.inherited_fds.is_empty() {
        return Err(ExecError::Spawn(
            "inherited file descriptors are not supported by exec-server executor".to_string(),
        ));
    }
    Ok(())
}
