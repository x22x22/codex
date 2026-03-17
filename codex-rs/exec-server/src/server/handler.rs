use std::collections::HashMap;
use std::sync::Arc;

use codex_utils_pty::ExecCommandSession;
use codex_utils_pty::TerminalSize;
use tokio::sync::Mutex;
use tokio::sync::mpsc;

use crate::protocol::ExecExitedNotification;
use crate::protocol::ExecOutputDeltaNotification;
use crate::protocol::ExecOutputStream;
use crate::protocol::ExecResponse;
use crate::protocol::InitializeResponse;
use crate::protocol::PROTOCOL_VERSION;
use crate::protocol::TerminateResponse;
use crate::protocol::WriteResponse;
use crate::server::routing::ExecServerClientNotification;
use crate::server::routing::ExecServerInboundMessage;
use crate::server::routing::ExecServerOutboundMessage;
use crate::server::routing::ExecServerRequest;
use crate::server::routing::ExecServerResponseMessage;
use crate::server::routing::ExecServerServerNotification;
use crate::server::routing::internal_error;
use crate::server::routing::invalid_params;
use crate::server::routing::invalid_request;

struct RunningProcess {
    session: ExecCommandSession,
    tty: bool,
}

pub(crate) struct ExecServerHandler {
    outbound_tx: mpsc::Sender<ExecServerOutboundMessage>,
    // Keyed by the protocol `processId`, which is caller-assigned and scoped to
    // a single client connection rather than an OS pid.
    processes: Arc<Mutex<HashMap<String, RunningProcess>>>,
    initialize_requested: bool,
    initialized: bool,
}

impl ExecServerHandler {
    pub(crate) fn new(outbound_tx: mpsc::Sender<ExecServerOutboundMessage>) -> Self {
        Self {
            outbound_tx,
            processes: Arc::new(Mutex::new(HashMap::new())),
            initialize_requested: false,
            initialized: false,
        }
    }

    pub(crate) async fn shutdown(&self) {
        let remaining = {
            let mut processes = self.processes.lock().await;
            processes
                .drain()
                .map(|(_, process)| process)
                .collect::<Vec<_>>()
        };
        for process in remaining {
            process.session.terminate();
        }
    }

    pub(crate) async fn handle_message(
        &mut self,
        message: ExecServerInboundMessage,
    ) -> Result<(), String> {
        match message {
            ExecServerInboundMessage::Request(request) => self.handle_request(request).await,
            ExecServerInboundMessage::Notification(notification) => {
                self.handle_notification(notification)
            }
        }
    }

    async fn handle_request(&mut self, request: ExecServerRequest) -> Result<(), String> {
        match request {
            ExecServerRequest::Initialize { request_id, .. } => {
                let result = self
                    .handle_initialize_request()
                    .map(ExecServerResponseMessage::Initialize);
                self.send_request_result(request_id, result).await;
            }
            ExecServerRequest::Exec { request_id, params } => {
                self.send_request_result(
                    request_id,
                    match self.require_initialized() {
                        Ok(()) => self
                            .handle_exec_request(params)
                            .await
                            .map(ExecServerResponseMessage::Exec),
                        Err(err) => Err(err),
                    },
                )
                .await;
            }
            ExecServerRequest::Write { request_id, params } => {
                self.send_request_result(
                    request_id,
                    match self.require_initialized() {
                        Ok(()) => self
                            .handle_write_request(params)
                            .await
                            .map(ExecServerResponseMessage::Write),
                        Err(err) => Err(err),
                    },
                )
                .await;
            }
            ExecServerRequest::Terminate { request_id, params } => {
                self.send_request_result(
                    request_id,
                    match self.require_initialized() {
                        Ok(()) => self
                            .handle_terminate_request(params)
                            .await
                            .map(ExecServerResponseMessage::Terminate),
                        Err(err) => Err(err),
                    },
                )
                .await;
            }
        }
        Ok(())
    }

    fn handle_notification(
        &mut self,
        notification: ExecServerClientNotification,
    ) -> Result<(), String> {
        match notification {
            ExecServerClientNotification::Initialized => {
                if !self.initialize_requested {
                    return Err("received `initialized` notification before `initialize`".into());
                }
                self.initialized = true;
                Ok(())
            }
        }
    }

    fn handle_initialize_request(
        &mut self,
    ) -> Result<InitializeResponse, codex_app_server_protocol::JSONRPCErrorError> {
        if self.initialize_requested {
            return Err(invalid_request(
                "initialize may only be sent once per connection".to_string(),
            ));
        }
        self.initialize_requested = true;
        Ok(InitializeResponse {
            protocol_version: PROTOCOL_VERSION.to_string(),
        })
    }

    fn require_initialized(&self) -> Result<(), codex_app_server_protocol::JSONRPCErrorError> {
        if !self.initialize_requested {
            return Err(invalid_request(
                "client must call initialize before using exec methods".to_string(),
            ));
        }
        if !self.initialized {
            return Err(invalid_request(
                "client must send initialized before using exec methods".to_string(),
            ));
        }
        Ok(())
    }

    async fn handle_exec_request(
        &self,
        params: crate::protocol::ExecParams,
    ) -> Result<ExecResponse, codex_app_server_protocol::JSONRPCErrorError> {
        let (program, args) = params
            .argv
            .split_first()
            .ok_or_else(|| invalid_params("argv must not be empty".to_string()))?;

        let spawned = if params.tty {
            codex_utils_pty::spawn_pty_process(
                program,
                args,
                params.cwd.as_path(),
                &params.env,
                &params.arg0,
                TerminalSize::default(),
            )
            .await
        } else {
            codex_utils_pty::spawn_pipe_process_no_stdin(
                program,
                args,
                params.cwd.as_path(),
                &params.env,
                &params.arg0,
            )
            .await
        }
        .map_err(|err| internal_error(err.to_string()))?;

        let process_id = params.process_id.clone();
        {
            let mut process_map = self.processes.lock().await;
            if process_map.contains_key(&process_id) {
                spawned.session.terminate();
                return Err(invalid_request(format!(
                    "process {process_id} already exists"
                )));
            }
            process_map.insert(
                process_id.clone(),
                RunningProcess {
                    session: spawned.session,
                    tty: params.tty,
                },
            );
        }

        tokio::spawn(stream_output(
            process_id.clone(),
            ExecOutputStream::Stdout,
            spawned.stdout_rx,
            self.outbound_tx.clone(),
        ));
        tokio::spawn(stream_output(
            process_id.clone(),
            ExecOutputStream::Stderr,
            spawned.stderr_rx,
            self.outbound_tx.clone(),
        ));
        tokio::spawn(watch_exit(
            process_id.clone(),
            spawned.exit_rx,
            self.outbound_tx.clone(),
            Arc::clone(&self.processes),
        ));

        Ok(ExecResponse { process_id })
    }

    async fn handle_write_request(
        &self,
        params: crate::protocol::WriteParams,
    ) -> Result<WriteResponse, codex_app_server_protocol::JSONRPCErrorError> {
        let writer_tx = {
            let process_map = self.processes.lock().await;
            let process = process_map.get(&params.process_id).ok_or_else(|| {
                invalid_request(format!("unknown process id {}", params.process_id))
            })?;
            if !process.tty {
                return Err(invalid_request(format!(
                    "stdin is closed for process {}",
                    params.process_id
                )));
            }
            process.session.writer_sender()
        };

        writer_tx
            .send(params.chunk.into_inner())
            .await
            .map_err(|_| internal_error("failed to write to process stdin".to_string()))?;

        Ok(WriteResponse { accepted: true })
    }

    async fn handle_terminate_request(
        &self,
        params: crate::protocol::TerminateParams,
    ) -> Result<TerminateResponse, codex_app_server_protocol::JSONRPCErrorError> {
        let process = {
            let mut process_map = self.processes.lock().await;
            process_map.remove(&params.process_id)
        };

        Ok(if let Some(process) = process {
            process.session.terminate();
            TerminateResponse { running: true }
        } else {
            TerminateResponse { running: false }
        })
    }

    async fn send_request_result(
        &self,
        request_id: codex_app_server_protocol::RequestId,
        result: Result<ExecServerResponseMessage, codex_app_server_protocol::JSONRPCErrorError>,
    ) {
        let outbound = match result {
            Ok(response) => ExecServerOutboundMessage::Response {
                request_id,
                response,
            },
            Err(error) => ExecServerOutboundMessage::Error { request_id, error },
        };
        self.send_outbound(outbound).await;
    }

    async fn send_outbound(&self, outbound: ExecServerOutboundMessage) {
        let _ = self.outbound_tx.send(outbound).await;
    }
}

async fn stream_output(
    process_id: String,
    stream: ExecOutputStream,
    mut receiver: tokio::sync::mpsc::Receiver<Vec<u8>>,
    outbound_tx: mpsc::Sender<ExecServerOutboundMessage>,
) {
    while let Some(chunk) = receiver.recv().await {
        if outbound_tx
            .send(ExecServerOutboundMessage::Notification(
                ExecServerServerNotification::OutputDelta(ExecOutputDeltaNotification {
                    process_id: process_id.clone(),
                    stream,
                    chunk: chunk.into(),
                }),
            ))
            .await
            .is_err()
        {
            break;
        }
    }
}

async fn watch_exit(
    process_id: String,
    exit_rx: tokio::sync::oneshot::Receiver<i32>,
    outbound_tx: mpsc::Sender<ExecServerOutboundMessage>,
    processes: Arc<Mutex<HashMap<String, RunningProcess>>>,
) {
    let exit_code = exit_rx.await.unwrap_or(-1);
    {
        let mut processes = processes.lock().await;
        processes.remove(&process_id);
    }
    let _ = outbound_tx
        .send(ExecServerOutboundMessage::Notification(
            ExecServerServerNotification::Exited(ExecExitedNotification {
                process_id,
                exit_code,
            }),
        ))
        .await;
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use pretty_assertions::assert_eq;
    use tokio::time::timeout;

    use super::ExecServerHandler;
    use crate::protocol::InitializeParams;
    use crate::protocol::InitializeResponse;
    use crate::protocol::PROTOCOL_VERSION;
    use crate::protocol::TerminateResponse;
    use crate::protocol::WriteParams;
    use crate::server::routing::ExecServerClientNotification;
    use crate::server::routing::ExecServerInboundMessage;
    use crate::server::routing::ExecServerOutboundMessage;
    use crate::server::routing::ExecServerRequest;
    use crate::server::routing::ExecServerResponseMessage;
    use codex_app_server_protocol::RequestId;

    async fn recv_outbound(
        outgoing_rx: &mut tokio::sync::mpsc::Receiver<ExecServerOutboundMessage>,
    ) -> ExecServerOutboundMessage {
        let recv_result = timeout(Duration::from_secs(1), outgoing_rx.recv()).await;
        let maybe_message = match recv_result {
            Ok(maybe_message) => maybe_message,
            Err(err) => panic!("timed out waiting for handler output: {err}"),
        };
        match maybe_message {
            Some(message) => message,
            None => panic!("handler output channel closed unexpectedly"),
        }
    }

    #[tokio::test]
    async fn initialize_response_reports_protocol_version() {
        let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(1);
        let mut handler = ExecServerHandler::new(outgoing_tx);

        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Request(
                ExecServerRequest::Initialize {
                    request_id: RequestId::Integer(1),
                    params: InitializeParams {
                        client_name: "test".to_string(),
                    },
                },
            ))
            .await
        {
            panic!("initialize should succeed: {err}");
        }

        assert_eq!(
            recv_outbound(&mut outgoing_rx).await,
            ExecServerOutboundMessage::Response {
                request_id: RequestId::Integer(1),
                response: ExecServerResponseMessage::Initialize(InitializeResponse {
                    protocol_version: PROTOCOL_VERSION.to_string(),
                }),
            }
        );
    }

    #[tokio::test]
    async fn exec_methods_require_initialize() {
        let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(1);
        let mut handler = ExecServerHandler::new(outgoing_tx);

        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Request(ExecServerRequest::Exec {
                request_id: RequestId::Integer(7),
                params: crate::protocol::ExecParams {
                    process_id: "proc-1".to_string(),
                    argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
                    cwd: std::env::current_dir().expect("cwd"),
                    env: HashMap::new(),
                    tty: true,
                    arg0: None,
                },
            }))
            .await
        {
            panic!("request handling should not fail the handler: {err}");
        }

        let ExecServerOutboundMessage::Error { request_id, error } =
            recv_outbound(&mut outgoing_rx).await
        else {
            panic!("expected invalid-request error");
        };
        assert_eq!(request_id, RequestId::Integer(7));
        assert_eq!(error.code, -32600);
        assert_eq!(
            error.message,
            "client must call initialize before using exec methods"
        );
    }

    #[tokio::test]
    async fn exec_methods_require_initialized_notification_after_initialize() {
        let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(2);
        let mut handler = ExecServerHandler::new(outgoing_tx);

        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Request(
                ExecServerRequest::Initialize {
                    request_id: RequestId::Integer(1),
                    params: InitializeParams {
                        client_name: "test".to_string(),
                    },
                },
            ))
            .await
        {
            panic!("initialize should succeed: {err}");
        }
        let _ = recv_outbound(&mut outgoing_rx).await;

        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Request(ExecServerRequest::Exec {
                request_id: RequestId::Integer(2),
                params: crate::protocol::ExecParams {
                    process_id: "proc-1".to_string(),
                    argv: vec!["bash".to_string(), "-lc".to_string(), "true".to_string()],
                    cwd: std::env::current_dir().expect("cwd"),
                    env: HashMap::new(),
                    tty: true,
                    arg0: None,
                },
            }))
            .await
        {
            panic!("request handling should not fail the handler: {err}");
        }

        let ExecServerOutboundMessage::Error { request_id, error } =
            recv_outbound(&mut outgoing_rx).await
        else {
            panic!("expected invalid-request error");
        };
        assert_eq!(request_id, RequestId::Integer(2));
        assert_eq!(error.code, -32600);
        assert_eq!(
            error.message,
            "client must send initialized before using exec methods"
        );
    }

    #[tokio::test]
    async fn initialized_before_initialize_is_a_protocol_error() {
        let (outgoing_tx, _outgoing_rx) = tokio::sync::mpsc::channel(1);
        let mut handler = ExecServerHandler::new(outgoing_tx);

        let result = handler
            .handle_message(ExecServerInboundMessage::Notification(
                ExecServerClientNotification::Initialized,
            ))
            .await;

        match result {
            Err(err) => {
                assert_eq!(
                    err,
                    "received `initialized` notification before `initialize`"
                );
            }
            Ok(()) => panic!("expected protocol error for early initialized notification"),
        }
    }

    #[tokio::test]
    async fn initialize_may_only_be_sent_once_per_connection() {
        let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(2);
        let mut handler = ExecServerHandler::new(outgoing_tx);

        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Request(
                ExecServerRequest::Initialize {
                    request_id: RequestId::Integer(1),
                    params: InitializeParams {
                        client_name: "test".to_string(),
                    },
                },
            ))
            .await
        {
            panic!("initialize should succeed: {err}");
        }
        let _ = recv_outbound(&mut outgoing_rx).await;

        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Request(
                ExecServerRequest::Initialize {
                    request_id: RequestId::Integer(2),
                    params: InitializeParams {
                        client_name: "test".to_string(),
                    },
                },
            ))
            .await
        {
            panic!("duplicate initialize should not fail the handler: {err}");
        }

        let ExecServerOutboundMessage::Error { request_id, error } =
            recv_outbound(&mut outgoing_rx).await
        else {
            panic!("expected invalid-request error");
        };
        assert_eq!(request_id, RequestId::Integer(2));
        assert_eq!(error.code, -32600);
        assert_eq!(
            error.message,
            "initialize may only be sent once per connection"
        );
    }

    #[tokio::test]
    async fn duplicate_process_ids_are_rejected_per_connection() {
        let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(4);
        let mut handler = ExecServerHandler::new(outgoing_tx);

        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Request(
                ExecServerRequest::Initialize {
                    request_id: RequestId::Integer(1),
                    params: InitializeParams {
                        client_name: "test".to_string(),
                    },
                },
            ))
            .await
        {
            panic!("initialize should succeed: {err}");
        }
        let _ = recv_outbound(&mut outgoing_rx).await;
        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Notification(
                ExecServerClientNotification::Initialized,
            ))
            .await
        {
            panic!("initialized should succeed: {err}");
        }

        let params = crate::protocol::ExecParams {
            process_id: "proc-1".to_string(),
            argv: vec![
                "bash".to_string(),
                "-lc".to_string(),
                "sleep 30".to_string(),
            ],
            cwd: std::env::current_dir().expect("cwd"),
            env: HashMap::new(),
            tty: false,
            arg0: None,
        };
        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Request(ExecServerRequest::Exec {
                request_id: RequestId::Integer(2),
                params: params.clone(),
            }))
            .await
        {
            panic!("first exec should succeed: {err}");
        }
        assert_eq!(
            recv_outbound(&mut outgoing_rx).await,
            ExecServerOutboundMessage::Response {
                request_id: RequestId::Integer(2),
                response: ExecServerResponseMessage::Exec(crate::protocol::ExecResponse {
                    process_id: "proc-1".to_string(),
                }),
            }
        );

        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Request(ExecServerRequest::Exec {
                request_id: RequestId::Integer(3),
                params,
            }))
            .await
        {
            panic!("duplicate exec should not fail the handler: {err}");
        }

        let ExecServerOutboundMessage::Error { request_id, error } =
            recv_outbound(&mut outgoing_rx).await
        else {
            panic!("expected duplicate-process error");
        };
        assert_eq!(request_id, RequestId::Integer(3));
        assert_eq!(error.code, -32600);
        assert_eq!(error.message, "process proc-1 already exists");

        handler.shutdown().await;
    }

    #[tokio::test]
    async fn writes_to_pipe_backed_processes_are_rejected() {
        let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(4);
        let mut handler = ExecServerHandler::new(outgoing_tx);

        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Request(
                ExecServerRequest::Initialize {
                    request_id: RequestId::Integer(1),
                    params: InitializeParams {
                        client_name: "test".to_string(),
                    },
                },
            ))
            .await
        {
            panic!("initialize should succeed: {err}");
        }
        let _ = recv_outbound(&mut outgoing_rx).await;
        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Notification(
                ExecServerClientNotification::Initialized,
            ))
            .await
        {
            panic!("initialized should succeed: {err}");
        }

        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Request(ExecServerRequest::Exec {
                request_id: RequestId::Integer(2),
                params: crate::protocol::ExecParams {
                    process_id: "proc-2".to_string(),
                    argv: vec![
                        "bash".to_string(),
                        "-lc".to_string(),
                        "sleep 30".to_string(),
                    ],
                    cwd: std::env::current_dir().expect("cwd"),
                    env: HashMap::new(),
                    tty: false,
                    arg0: None,
                },
            }))
            .await
        {
            panic!("exec should succeed: {err}");
        }
        let _ = recv_outbound(&mut outgoing_rx).await;

        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Request(
                ExecServerRequest::Write {
                    request_id: RequestId::Integer(3),
                    params: WriteParams {
                        process_id: "proc-2".to_string(),
                        chunk: b"hello\n".to_vec().into(),
                    },
                },
            ))
            .await
        {
            panic!("write should not fail the handler: {err}");
        }

        let ExecServerOutboundMessage::Error { request_id, error } =
            recv_outbound(&mut outgoing_rx).await
        else {
            panic!("expected stdin-closed error");
        };
        assert_eq!(request_id, RequestId::Integer(3));
        assert_eq!(error.code, -32600);
        assert_eq!(error.message, "stdin is closed for process proc-2");

        handler.shutdown().await;
    }

    #[tokio::test]
    async fn writes_to_unknown_processes_are_rejected() {
        let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(2);
        let mut handler = ExecServerHandler::new(outgoing_tx);

        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Request(
                ExecServerRequest::Initialize {
                    request_id: RequestId::Integer(1),
                    params: InitializeParams {
                        client_name: "test".to_string(),
                    },
                },
            ))
            .await
        {
            panic!("initialize should succeed: {err}");
        }
        let _ = recv_outbound(&mut outgoing_rx).await;
        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Notification(
                ExecServerClientNotification::Initialized,
            ))
            .await
        {
            panic!("initialized should succeed: {err}");
        }

        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Request(
                ExecServerRequest::Write {
                    request_id: RequestId::Integer(2),
                    params: WriteParams {
                        process_id: "missing".to_string(),
                        chunk: b"hello\n".to_vec().into(),
                    },
                },
            ))
            .await
        {
            panic!("write should not fail the handler: {err}");
        }

        let ExecServerOutboundMessage::Error { request_id, error } =
            recv_outbound(&mut outgoing_rx).await
        else {
            panic!("expected unknown-process error");
        };
        assert_eq!(request_id, RequestId::Integer(2));
        assert_eq!(error.code, -32600);
        assert_eq!(error.message, "unknown process id missing");
    }

    #[tokio::test]
    async fn terminate_unknown_processes_report_running_false() {
        let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::channel(2);
        let mut handler = ExecServerHandler::new(outgoing_tx);

        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Request(
                ExecServerRequest::Initialize {
                    request_id: RequestId::Integer(1),
                    params: InitializeParams {
                        client_name: "test".to_string(),
                    },
                },
            ))
            .await
        {
            panic!("initialize should succeed: {err}");
        }
        let _ = recv_outbound(&mut outgoing_rx).await;
        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Notification(
                ExecServerClientNotification::Initialized,
            ))
            .await
        {
            panic!("initialized should succeed: {err}");
        }

        if let Err(err) = handler
            .handle_message(ExecServerInboundMessage::Request(
                ExecServerRequest::Terminate {
                    request_id: RequestId::Integer(2),
                    params: crate::protocol::TerminateParams {
                        process_id: "missing".to_string(),
                    },
                },
            ))
            .await
        {
            panic!("terminate should not fail the handler: {err}");
        }

        assert_eq!(
            recv_outbound(&mut outgoing_rx).await,
            ExecServerOutboundMessage::Response {
                request_id: RequestId::Integer(2),
                response: ExecServerResponseMessage::Terminate(TerminateResponse {
                    running: false,
                }),
            }
        );
    }
}
