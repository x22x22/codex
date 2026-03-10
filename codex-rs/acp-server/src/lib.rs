#![deny(clippy::print_stdout, clippy::print_stderr)]

use std::collections::HashMap;
use std::io::Error;
use std::io::ErrorKind;
use std::io::Result as IoResult;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use chrono::SecondsFormat;
use chrono::Utc;
use codex_app_server_client::DEFAULT_IN_PROCESS_CHANNEL_CAPACITY;
use codex_app_server_client::InProcessAppServerClient;
use codex_app_server_client::InProcessClientStartArgs;
use codex_app_server_client::InProcessServerEvent;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::CollaborationModeListParams;
use codex_app_server_protocol::CollaborationModeListResponse;
use codex_app_server_protocol::CollaborationModeMask;
use codex_app_server_protocol::CommandExecutionApprovalDecision;
use codex_app_server_protocol::CommandExecutionRequestApprovalParams;
use codex_app_server_protocol::CommandExecutionRequestApprovalResponse;
use codex_app_server_protocol::FileChangeApprovalDecision;
use codex_app_server_protocol::FileChangeRequestApprovalParams;
use codex_app_server_protocol::FileChangeRequestApprovalResponse;
use codex_app_server_protocol::ItemCompletedNotification;
use codex_app_server_protocol::ItemStartedNotification;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::McpServerElicitationRequest as AppMcpServerElicitationRequest;
use codex_app_server_protocol::McpServerElicitationRequestParams;
use codex_app_server_protocol::McpServerElicitationRequestResponse;
use codex_app_server_protocol::Model;
use codex_app_server_protocol::ModelListParams;
use codex_app_server_protocol::ModelListResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ServerRequest;
use codex_app_server_protocol::ThreadForkParams;
use codex_app_server_protocol::ThreadForkResponse;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadListParams;
use codex_app_server_protocol::ThreadListResponse;
use codex_app_server_protocol::ThreadReadParams;
use codex_app_server_protocol::ThreadReadResponse;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::ThreadUnsubscribeParams;
use codex_app_server_protocol::ThreadUnsubscribeResponse;
use codex_app_server_protocol::ToolRequestUserInputAnswer;
use codex_app_server_protocol::ToolRequestUserInputParams;
use codex_app_server_protocol::ToolRequestUserInputQuestion;
use codex_app_server_protocol::ToolRequestUserInputResponse;
use codex_app_server_protocol::TurnCompletedNotification;
use codex_app_server_protocol::TurnInterruptParams;
use codex_app_server_protocol::TurnPlanStepStatus;
use codex_app_server_protocol::TurnPlanUpdatedNotification;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::TurnStatus;
use codex_app_server_protocol::UserInput;
use codex_arg0::Arg0DispatchPaths;
use codex_cloud_requirements::cloud_requirements_loader;
use codex_core::AuthManager;
use codex_core::config::Config;
use codex_core::config::types::McpServerConfig;
use codex_core::config::types::McpServerTransportConfig;
use codex_core::config_loader::LoaderOverrides;
use codex_feedback::CodexFeedback;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::CollaborationModeMask as CoreCollaborationModeMask;
use codex_protocol::config_types::Settings;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::SessionSource;
use codex_rmcp_client::AcpBridge;
use codex_rmcp_client::AcpConnection;
use codex_rmcp_client::set_acp_bridge;
use codex_utils_cli::CliConfigOverrides;
use futures::future::BoxFuture;
use rmcp::model::ClientJsonRpcMessage;
use rmcp::model::ClientResult as McpClientResult;
use rmcp::model::ErrorCode as McpErrorCode;
use rmcp::model::ErrorData as McpErrorData;
use rmcp::model::JsonRpcError as McpJsonRpcError;
use rmcp::model::JsonRpcMessage as McpJsonRpcMessage;
use rmcp::model::JsonRpcNotification as McpJsonRpcNotification;
use rmcp::model::JsonRpcRequest as McpJsonRpcRequest;
use rmcp::model::JsonRpcResponse as McpJsonRpcResponse;
use rmcp::model::RequestId as McpRequestId;
use rmcp::model::ServerJsonRpcMessage;
use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::io::{self};
use tokio::sync::mpsc;
use tracing::debug;
use tracing::error;
use tracing::info;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

mod protocol;

use crate::protocol::AgentCapabilities;
use crate::protocol::AgentInfo;
use crate::protocol::ConfigOption;
use crate::protocol::ConfigOptionValue;
use crate::protocol::ElicitationAction;
use crate::protocol::ElicitationCapabilities;
use crate::protocol::InitializeParams;
use crate::protocol::InitializeResult;
use crate::protocol::JSONRPC_VERSION;
use crate::protocol::JsonRpcError;
use crate::protocol::JsonRpcErrorBody;
use crate::protocol::JsonRpcMessage;
use crate::protocol::JsonRpcNotification;
use crate::protocol::JsonRpcRequest;
use crate::protocol::JsonRpcResponse;
use crate::protocol::Location;
use crate::protocol::McpCapabilities;
use crate::protocol::McpConnectParams;
use crate::protocol::McpConnectResult;
use crate::protocol::McpDisconnectParams;
use crate::protocol::McpDisconnectResult;
use crate::protocol::McpMessageParams;
use crate::protocol::McpMessageResult;
use crate::protocol::Mode;
use crate::protocol::Modes;
use crate::protocol::PROTOCOL_VERSION;
use crate::protocol::PermissionOption;
use crate::protocol::PermissionOutcome;
use crate::protocol::PermissionResponse;
use crate::protocol::PermissionToolCall;
use crate::protocol::PlanEntry;
use crate::protocol::PromptCapabilities;
use crate::protocol::PromptContent;
use crate::protocol::SessionCancelParams;
use crate::protocol::SessionCapabilities;
use crate::protocol::SessionCloseParams;
use crate::protocol::SessionCloseResult;
use crate::protocol::SessionElicitationParams;
use crate::protocol::SessionElicitationRequest;
use crate::protocol::SessionElicitationResponse;
use crate::protocol::SessionForkParams;
use crate::protocol::SessionForkResult;
use crate::protocol::SessionInfo;
use crate::protocol::SessionListParams;
use crate::protocol::SessionListResult;
use crate::protocol::SessionLoadParams;
use crate::protocol::SessionLoadResult;
use crate::protocol::SessionNewParams;
use crate::protocol::SessionNewResult;
use crate::protocol::SessionPromptParams;
use crate::protocol::SessionPromptResult;
use crate::protocol::SessionRequestPermissionParams;
use crate::protocol::SessionResumeParams;
use crate::protocol::SessionResumeResult;
use crate::protocol::SessionSetModeParams;
use crate::protocol::SessionSetModeResult;
use crate::protocol::SessionUpdate;
use crate::protocol::SessionUpdateParams;
use crate::protocol::TextContent;
use crate::protocol::ToolCallContent;

const CHANNEL_CAPACITY: usize = 128;
const CLIENT_NAME: &str = "codex-acp-server";

#[derive(Clone)]
struct ModeBinding {
    acp_mode: Mode,
    mask: CoreCollaborationModeMask,
}

struct SessionState {
    active_turn_id: Option<String>,
    config_options: Vec<ConfigOption>,
    current_mode_id: String,
    current_mode: CollaborationMode,
    cwd: String,
    last_title: Option<String>,
    modes: Vec<ModeBinding>,
    thread_id: String,
}

impl SessionState {
    fn modes_payload(&self) -> Modes {
        Modes {
            current_mode_id: self.current_mode_id.clone(),
            available_modes: self
                .modes
                .iter()
                .map(|binding| binding.acp_mode.clone())
                .collect(),
        }
    }
}

struct PendingPrompt {
    request_id: RequestId,
    session_id: String,
}

struct PendingPermissionRequest {
    app_request_id: RequestId,
    option_results: HashMap<String, JsonValue>,
    fallback_result: JsonValue,
}

struct PendingElicitationRequest {
    app_request_id: RequestId,
    fallback_result: JsonValue,
    kind: PendingElicitationKind,
}

enum PendingElicitationKind {
    ToolUserInput {
        questions: Vec<ToolRequestUserInputQuestion>,
    },
    McpServer,
}

struct PendingMcpConnect {
    responder: tokio::sync::oneshot::Sender<IoResult<McpConnectResult>>,
}

struct PendingMcpDisconnect {
    responder: tokio::sync::oneshot::Sender<IoResult<()>>,
}

struct PendingMcpRequest {
    inbound_tx: mpsc::UnboundedSender<ServerJsonRpcMessage>,
    request_id: McpRequestId,
}

struct RegisteredMcpConnection {
    inbound_tx: mpsc::UnboundedSender<ServerJsonRpcMessage>,
}

struct AcpBridgeState {
    next_request_id: AtomicI64,
    outgoing_tx: mpsc::UnboundedSender<JsonRpcMessage>,
    connections: StdMutex<HashMap<String, RegisteredMcpConnection>>,
    pending_connects: StdMutex<HashMap<RequestId, PendingMcpConnect>>,
    pending_disconnects: StdMutex<HashMap<RequestId, PendingMcpDisconnect>>,
    pending_requests: StdMutex<HashMap<RequestId, PendingMcpRequest>>,
}

impl AcpBridgeState {
    fn new(outgoing_tx: mpsc::UnboundedSender<JsonRpcMessage>) -> Self {
        Self {
            next_request_id: AtomicI64::new(1),
            outgoing_tx,
            connections: StdMutex::new(HashMap::new()),
            pending_connects: StdMutex::new(HashMap::new()),
            pending_disconnects: StdMutex::new(HashMap::new()),
            pending_requests: StdMutex::new(HashMap::new()),
        }
    }

    fn next_request_id(&self, prefix: &str) -> RequestId {
        RequestId::String(format!(
            "{prefix}:{}",
            self.next_request_id.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn register_connection(
        &self,
        connection_id: String,
        inbound_tx: mpsc::UnboundedSender<ServerJsonRpcMessage>,
    ) {
        self.connections
            .lock()
            .unwrap_or_else(|_| panic!("ACP MCP connections registry poisoned"))
            .insert(connection_id, RegisteredMcpConnection { inbound_tx });
    }

    fn unregister_connection(&self, connection_id: &str) -> Option<RegisteredMcpConnection> {
        self.connections
            .lock()
            .unwrap_or_else(|_| panic!("ACP MCP connections registry poisoned"))
            .remove(connection_id)
    }

    fn connection_sender(
        &self,
        connection_id: &str,
    ) -> Option<mpsc::UnboundedSender<ServerJsonRpcMessage>> {
        self.connections
            .lock()
            .unwrap_or_else(|_| panic!("ACP MCP connections registry poisoned"))
            .get(connection_id)
            .map(|connection| connection.inbound_tx.clone())
    }

    fn send_jsonrpc_message(&self, message: JsonRpcMessage) -> IoResult<()> {
        self.outgoing_tx
            .send(message)
            .map_err(|_| Error::new(ErrorKind::BrokenPipe, "ACP stdout writer channel closed"))
    }

    fn send_request(&self, id: RequestId, method: &str, params: JsonValue) -> IoResult<()> {
        self.send_jsonrpc_message(JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            method: method.to_string(),
            params: Some(params),
        }))
    }

    fn send_notification(&self, method: &str, params: JsonValue) -> IoResult<()> {
        self.send_jsonrpc_message(JsonRpcMessage::Notification(JsonRpcNotification {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: method.to_string(),
            params: Some(params),
        }))
    }

    fn send_result(&self, id: RequestId, result: JsonValue) -> IoResult<()> {
        self.send_jsonrpc_message(JsonRpcMessage::Response(JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result,
        }))
    }

    fn handle_response(&self, response: JsonRpcResponse) -> bool {
        if let Some(pending_connect) = self
            .pending_connects
            .lock()
            .unwrap_or_else(|_| panic!("ACP MCP connect registry poisoned"))
            .remove(&response.id)
        {
            let result = serde_json::from_value::<McpConnectResult>(response.result)
                .map_err(invalid_params_error);
            let _ = pending_connect.responder.send(result);
            return true;
        }

        if let Some(pending_disconnect) = self
            .pending_disconnects
            .lock()
            .unwrap_or_else(|_| panic!("ACP MCP disconnect registry poisoned"))
            .remove(&response.id)
        {
            let result = serde_json::from_value::<McpDisconnectResult>(response.result)
                .map(|_| ())
                .map_err(invalid_params_error);
            let _ = pending_disconnect.responder.send(result);
            return true;
        }

        let Some(pending_request) = self
            .pending_requests
            .lock()
            .unwrap_or_else(|_| panic!("ACP MCP request registry poisoned"))
            .remove(&response.id)
        else {
            return false;
        };

        let message = match serde_json::from_value::<McpMessageResult>(response.result) {
            Ok(result) => mcp_result_to_server_message(pending_request.request_id, result),
            Err(error) => ServerJsonRpcMessage::Error(McpJsonRpcError {
                jsonrpc: rmcp::model::JsonRpcVersion2_0,
                id: pending_request.request_id,
                error: McpErrorData::new(
                    McpErrorCode::INTERNAL_ERROR,
                    format!("invalid ACP mcp/message response: {error}"),
                    None,
                ),
            }),
        };
        let _ = pending_request.inbound_tx.send(message);
        true
    }

    fn handle_error(&self, error_message: JsonRpcError) -> bool {
        if let Some(pending_connect) = self
            .pending_connects
            .lock()
            .unwrap_or_else(|_| panic!("ACP MCP connect registry poisoned"))
            .remove(&error_message.id)
        {
            let _ = pending_connect
                .responder
                .send(Err(io_error_from_jsonrpc_error(&error_message.error)));
            return true;
        }

        if let Some(pending_disconnect) = self
            .pending_disconnects
            .lock()
            .unwrap_or_else(|_| panic!("ACP MCP disconnect registry poisoned"))
            .remove(&error_message.id)
        {
            let _ = pending_disconnect
                .responder
                .send(Err(io_error_from_jsonrpc_error(&error_message.error)));
            return true;
        }

        let Some(pending_request) = self
            .pending_requests
            .lock()
            .unwrap_or_else(|_| panic!("ACP MCP request registry poisoned"))
            .remove(&error_message.id)
        else {
            return false;
        };

        let _ = pending_request
            .inbound_tx
            .send(ServerJsonRpcMessage::Error(McpJsonRpcError {
                jsonrpc: rmcp::model::JsonRpcVersion2_0,
                id: pending_request.request_id,
                error: acp_error_to_mcp_error(error_message.error),
            }));
        true
    }
}

struct AcpBridgeHandle {
    bridge_state: Arc<AcpBridgeState>,
}

struct AcpBridgeConnection {
    bridge_state: Arc<AcpBridgeState>,
    connection_id: String,
    inbound_rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<ServerJsonRpcMessage>>>,
}

impl AcpConnection for AcpBridgeConnection {
    fn send(&self, message: ClientJsonRpcMessage) -> BoxFuture<'static, anyhow::Result<()>> {
        let bridge_state = Arc::clone(&self.bridge_state);
        let connection_id = self.connection_id.clone();
        Box::pin(async move {
            forward_client_mcp_message(&bridge_state, &connection_id, message)
                .map_err(anyhow::Error::from)
        })
    }

    fn recv(&self) -> BoxFuture<'static, anyhow::Result<Option<ServerJsonRpcMessage>>> {
        let inbound_rx = Arc::clone(&self.inbound_rx);
        Box::pin(async move { Ok(inbound_rx.lock().await.recv().await) })
    }

    fn close(&self) -> BoxFuture<'static, anyhow::Result<()>> {
        let bridge_state = Arc::clone(&self.bridge_state);
        let connection_id = self.connection_id.clone();
        Box::pin(async move {
            close_mcp_connection(&bridge_state, &connection_id)
                .await
                .map_err(anyhow::Error::from)
        })
    }
}

impl AcpBridge for AcpBridgeHandle {
    fn connect(
        &self,
        acp_id: String,
    ) -> BoxFuture<'static, anyhow::Result<Arc<dyn AcpConnection>>> {
        let bridge_state = Arc::clone(&self.bridge_state);
        Box::pin(async move {
            let request_id = bridge_state.next_request_id("mcp-connect");
            let (responder, receiver) = tokio::sync::oneshot::channel();
            bridge_state
                .pending_connects
                .lock()
                .unwrap_or_else(|_| panic!("ACP MCP connect registry poisoned"))
                .insert(request_id.clone(), PendingMcpConnect { responder });
            bridge_state.send_request(
                request_id,
                "mcp/connect",
                serde_json::to_value(McpConnectParams { acp_id, meta: None })?,
            )?;
            let result = receiver.await.map_err(|_| {
                Error::new(
                    ErrorKind::BrokenPipe,
                    "ACP MCP connect response channel closed",
                )
            })??;
            let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
            bridge_state.register_connection(result.connection_id.clone(), inbound_tx);
            Ok(Arc::new(AcpBridgeConnection {
                bridge_state,
                connection_id: result.connection_id,
                inbound_rx: Arc::new(tokio::sync::Mutex::new(inbound_rx)),
            }) as Arc<dyn AcpConnection>)
        })
    }
}

struct AcpServer {
    app_client: InProcessAppServerClient,
    bridge_state: Arc<AcpBridgeState>,
    client_elicitation_capabilities: Option<ElicitationCapabilities>,
    incoming_rx: mpsc::Receiver<JsonRpcMessage>,
    initialized: bool,
    next_acp_request_id: i64,
    next_app_request_id: i64,
    outgoing_tx: mpsc::UnboundedSender<JsonRpcMessage>,
    pending_elicitations: HashMap<RequestId, PendingElicitationRequest>,
    pending_permissions: HashMap<RequestId, PendingPermissionRequest>,
    pending_prompts: HashMap<String, PendingPrompt>,
    sessions: HashMap<String, SessionState>,
}

impl AcpServer {
    async fn run(mut self) -> IoResult<()> {
        loop {
            tokio::select! {
                incoming = self.incoming_rx.recv() => {
                    let Some(message) = incoming else {
                        break;
                    };
                    self.handle_incoming_message(message).await?;
                }
                event = self.app_client.next_event() => {
                    let Some(event) = event else {
                        break;
                    };
                    self.handle_app_event(event).await?;
                }
            }
        }

        self.app_client.shutdown().await
    }

    async fn handle_incoming_message(&mut self, message: JsonRpcMessage) -> IoResult<()> {
        match message {
            JsonRpcMessage::Request(request) => self.handle_request(request).await,
            JsonRpcMessage::Notification(notification) => {
                self.handle_notification(notification).await
            }
            JsonRpcMessage::Response(response) => self.handle_response(response).await,
            JsonRpcMessage::Error(error_message) => self.handle_error_message(error_message).await,
        }
    }

    async fn handle_request(&mut self, request: JsonRpcRequest) -> IoResult<()> {
        if request.jsonrpc != JSONRPC_VERSION {
            self.send_error(
                request.id,
                -32600,
                format!("unsupported jsonrpc version: {}", request.jsonrpc),
            )?;
            return Ok(());
        }

        match request.method.as_str() {
            "initialize" => self.handle_initialize(request).await,
            "session/list" => self.handle_session_list(request).await,
            "session/new" => self.handle_session_new(request).await,
            "session/load" => self.handle_session_load(request).await,
            "session/resume" => self.handle_session_resume(request).await,
            "session/fork" => self.handle_session_fork(request).await,
            "session/prompt" => self.handle_session_prompt(request).await,
            "session/set_mode" => self.handle_session_set_mode(request).await,
            "session/close" => self.handle_session_close(request).await,
            "mcp/message" => self.handle_mcp_message_request(request).await,
            "mcp/disconnect" => self.handle_mcp_disconnect(request).await,
            other => {
                self.send_error(request.id, -32601, format!("method not found: {other}"))?;
                Ok(())
            }
        }
    }

    async fn handle_notification(&mut self, notification: JsonRpcNotification) -> IoResult<()> {
        if notification.jsonrpc != JSONRPC_VERSION {
            return Ok(());
        }

        match notification.method.as_str() {
            "session/cancel" => {
                let params = self.parse_params::<SessionCancelParams>(notification.params)?;
                self.handle_session_cancel(params).await
            }
            "mcp/message" => {
                let params = self.parse_params::<McpMessageParams>(notification.params)?;
                self.handle_mcp_message_notification(params)
            }
            other => {
                debug!("ignoring ACP notification {other}");
                Ok(())
            }
        }
    }

    async fn handle_response(&mut self, response: JsonRpcResponse) -> IoResult<()> {
        if self.bridge_state.handle_response(response.clone()) {
            return Ok(());
        }

        if let Some(pending_elicitation) = self.pending_elicitations.remove(&response.id) {
            let result = serde_json::from_value::<SessionElicitationResponse>(response.result)
                .map(|response| {
                    session_elicitation_to_app_result(response, &pending_elicitation.kind)
                })
                .unwrap_or_else(|_| pending_elicitation.fallback_result);
            return self
                .app_client
                .resolve_server_request(pending_elicitation.app_request_id, result)
                .await;
        }

        let Some(pending_permission) = self.pending_permissions.remove(&response.id) else {
            debug!("ignoring unmatched ACP response id {:?}", response.id);
            return Ok(());
        };

        let result = serde_json::from_value::<PermissionResponse>(response.result)
            .ok()
            .and_then(|permission_response| match permission_response.outcome {
                PermissionOutcome::Selected { option_id } => {
                    pending_permission.option_results.get(&option_id).cloned()
                }
                PermissionOutcome::Cancelled => None,
            })
            .unwrap_or(pending_permission.fallback_result);

        self.app_client
            .resolve_server_request(pending_permission.app_request_id, result)
            .await
    }

    async fn handle_error_message(&mut self, error_message: JsonRpcError) -> IoResult<()> {
        if self.bridge_state.handle_error(error_message.clone()) {
            return Ok(());
        }

        if let Some(pending_elicitation) = self.pending_elicitations.remove(&error_message.id) {
            return self
                .app_client
                .resolve_server_request(
                    pending_elicitation.app_request_id,
                    pending_elicitation.fallback_result,
                )
                .await;
        }

        if let Some(pending_permission) = self.pending_permissions.remove(&error_message.id) {
            return self
                .app_client
                .resolve_server_request(
                    pending_permission.app_request_id,
                    pending_permission.fallback_result,
                )
                .await;
        }

        error!("received unmatched ACP error response: {error_message:?}");
        Ok(())
    }

    async fn handle_initialize(&mut self, request: JsonRpcRequest) -> IoResult<()> {
        let params = self.parse_params::<InitializeParams>(request.params)?;
        if self.initialized {
            self.send_error(request.id, -32600, "initialize called more than once")?;
            return Ok(());
        }

        self.client_elicitation_capabilities =
            params.client_capabilities.and_then(|caps| caps.elicitation);
        self.initialized = true;
        let result = InitializeResult {
            protocol_version: PROTOCOL_VERSION,
            agent_capabilities: AgentCapabilities {
                load_session: true,
                session_capabilities: SessionCapabilities {
                    list: Default::default(),
                    fork: Default::default(),
                    resume: Default::default(),
                    close: Default::default(),
                },
                prompt_capabilities: PromptCapabilities {
                    image: true,
                    audio: false,
                    embedded_context: true,
                },
                mcp_capabilities: McpCapabilities {
                    http: false,
                    sse: false,
                    acp: true,
                },
            },
            agent_info: AgentInfo {
                name: CLIENT_NAME.to_string(),
                title: "Codex ACP Server".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            auth_methods: Vec::new(),
        };
        self.send_result(request.id, serde_json::to_value(result)?)?;
        Ok(())
    }

    async fn handle_session_new(&mut self, request: JsonRpcRequest) -> IoResult<()> {
        self.require_initialized(&request.id)?;
        let params = self.parse_params::<SessionNewParams>(request.params)?;
        let request_overrides = build_request_overrides_from_mcp_servers(&params.mcp_servers)
            .map_err(invalid_params_error)?;

        let request_id = self.next_app_request_id();
        let response: ThreadStartResponse = self
            .app_client
            .request_typed(ClientRequest::ThreadStart {
                request_id,
                params: ThreadStartParams {
                    cwd: Some(params.cwd.clone()),
                    config: request_overrides,
                    ..Default::default()
                },
            })
            .await
            .map_err(io_error_from_typed_request)?;

        let session_id = response.thread.id.clone();
        let session_state = self
            .build_session_state(
                response.thread.id.clone(),
                params.cwd.clone(),
                response.thread.name.clone(),
                &response.model,
                response.reasoning_effort,
            )
            .await?;
        let modes = session_state.modes_payload();
        let config_options = session_state.config_options.clone();
        let updated_at = rfc3339_timestamp(response.thread.updated_at);
        self.sessions.insert(session_id.clone(), session_state);

        self.send_result(
            request.id.clone(),
            serde_json::to_value(SessionNewResult {
                session_id: session_id.clone(),
                modes,
                config_options,
            })?,
        )?;
        self.maybe_send_session_info_update(&session_id, updated_at)?;
        Ok(())
    }

    async fn handle_session_list(&mut self, request: JsonRpcRequest) -> IoResult<()> {
        self.require_initialized(&request.id)?;
        let params = self.parse_params::<SessionListParams>(request.params)?;
        let filter_cwd = params.cwd.clone();
        let request_id = self.next_app_request_id();
        let response: ThreadListResponse = self
            .app_client
            .request_typed(ClientRequest::ThreadList {
                request_id,
                params: ThreadListParams {
                    cursor: params.cursor,
                    limit: None,
                    sort_key: None,
                    model_providers: None,
                    source_kinds: None,
                    archived: None,
                    cwd: filter_cwd.clone(),
                    search_term: None,
                },
            })
            .await
            .map_err(io_error_from_typed_request)?;
        let mut sessions = response
            .data
            .into_iter()
            .map(|thread| SessionInfo {
                session_id: thread.id,
                cwd: thread.cwd.display().to_string(),
                title: thread.name,
                updated_at: rfc3339_timestamp(thread.updated_at),
                meta: None,
            })
            .collect::<Vec<_>>();
        for session in self.sessions.values() {
            if sessions
                .iter()
                .any(|info| info.session_id == session.thread_id)
            {
                continue;
            }
            if let Some(filter_cwd) = filter_cwd.as_deref()
                && session.cwd != filter_cwd
            {
                continue;
            }
            sessions.push(SessionInfo {
                session_id: session.thread_id.clone(),
                cwd: session.cwd.clone(),
                title: session.last_title.clone(),
                updated_at: None,
                meta: Some(serde_json::json!({ "loaded": true })),
            });
        }

        self.send_result(
            request.id,
            serde_json::to_value(SessionListResult {
                sessions,
                next_cursor: response.next_cursor,
            })?,
        )
    }

    async fn handle_session_load(&mut self, request: JsonRpcRequest) -> IoResult<()> {
        self.require_initialized(&request.id)?;
        let params = self.parse_params::<SessionLoadParams>(request.params)?;
        let request_overrides = build_request_overrides_from_mcp_servers(&params.mcp_servers)
            .map_err(invalid_params_error)?;

        let read_request_id = self.next_app_request_id();
        let read_response: ThreadReadResponse = self
            .app_client
            .request_typed(ClientRequest::ThreadRead {
                request_id: read_request_id,
                params: ThreadReadParams {
                    thread_id: params.session_id.clone(),
                    include_turns: true,
                },
            })
            .await
            .map_err(io_error_from_typed_request)?;

        let resume_request_id = self.next_app_request_id();
        let resume_response: ThreadResumeResponse = self
            .app_client
            .request_typed(ClientRequest::ThreadResume {
                request_id: resume_request_id,
                params: ThreadResumeParams {
                    thread_id: params.session_id.clone(),
                    cwd: Some(params.cwd.clone()),
                    config: request_overrides,
                    ..Default::default()
                },
            })
            .await
            .map_err(io_error_from_typed_request)?;

        let session_state = self
            .build_session_state(
                resume_response.thread.id.clone(),
                params.cwd.clone(),
                resume_response.thread.name.clone(),
                &resume_response.model,
                resume_response.reasoning_effort,
            )
            .await?;
        let modes = session_state.modes_payload();
        let config_options = session_state.config_options.clone();
        let updated_at = rfc3339_timestamp(resume_response.thread.updated_at);
        self.sessions
            .insert(params.session_id.clone(), session_state);

        self.replay_thread_history(&params.session_id, read_response)
            .await?;
        self.send_result(
            request.id.clone(),
            serde_json::to_value(SessionLoadResult {
                modes,
                config_options,
            })?,
        )?;
        self.maybe_send_session_info_update(&params.session_id, updated_at)?;
        Ok(())
    }

    async fn handle_session_resume(&mut self, request: JsonRpcRequest) -> IoResult<()> {
        self.require_initialized(&request.id)?;
        let params = self.parse_params::<SessionResumeParams>(request.params)?;
        let cwd = params.cwd.clone();
        let request_overrides = build_request_overrides_from_mcp_servers(&params.mcp_servers)
            .map_err(invalid_params_error)?;

        let request_id = self.next_app_request_id();
        let response: ThreadResumeResponse = self
            .app_client
            .request_typed(ClientRequest::ThreadResume {
                request_id,
                params: ThreadResumeParams {
                    thread_id: params.session_id.clone(),
                    cwd: Some(cwd.clone()),
                    config: request_overrides,
                    ..Default::default()
                },
            })
            .await
            .map_err(io_error_from_typed_request)?;

        let session_state = self
            .build_session_state(
                response.thread.id.clone(),
                cwd,
                response.thread.name.clone(),
                &response.model,
                response.reasoning_effort,
            )
            .await?;
        let modes = session_state.modes_payload();
        let config_options = session_state.config_options.clone();
        let updated_at = rfc3339_timestamp(response.thread.updated_at);
        self.sessions
            .insert(params.session_id.clone(), session_state);

        self.send_result(
            request.id.clone(),
            serde_json::to_value(SessionResumeResult {
                modes,
                config_options,
            })?,
        )?;
        self.maybe_send_session_info_update(&params.session_id, updated_at)?;
        Ok(())
    }

    async fn handle_session_fork(&mut self, request: JsonRpcRequest) -> IoResult<()> {
        self.require_initialized(&request.id)?;
        let params = self.parse_params::<SessionForkParams>(request.params)?;
        let cwd = params.cwd.clone();
        let request_overrides = build_request_overrides_from_mcp_servers(&params.mcp_servers)
            .map_err(invalid_params_error)?;

        let request_id = self.next_app_request_id();
        let response: ThreadForkResponse = self
            .app_client
            .request_typed(ClientRequest::ThreadFork {
                request_id,
                params: ThreadForkParams {
                    thread_id: params.session_id,
                    cwd: Some(cwd.clone()),
                    config: request_overrides,
                    ..Default::default()
                },
            })
            .await
            .map_err(io_error_from_typed_request)?;

        let session_id = response.thread.id.clone();
        let session_state = self
            .build_session_state(
                response.thread.id.clone(),
                cwd,
                response.thread.name.clone(),
                &response.model,
                response.reasoning_effort,
            )
            .await?;
        let modes = session_state.modes_payload();
        let config_options = session_state.config_options.clone();
        let updated_at = rfc3339_timestamp(response.thread.updated_at);
        self.sessions.insert(session_id.clone(), session_state);

        self.send_result(
            request.id.clone(),
            serde_json::to_value(SessionForkResult {
                session_id: session_id.clone(),
                modes,
                config_options,
            })?,
        )?;
        self.maybe_send_session_info_update(&session_id, updated_at)?;
        Ok(())
    }

    async fn handle_session_close(&mut self, request: JsonRpcRequest) -> IoResult<()> {
        self.require_initialized(&request.id)?;
        let params = self.parse_params::<SessionCloseParams>(request.params)?;
        let request_id = self.next_app_request_id();
        let _: ThreadUnsubscribeResponse = self
            .app_client
            .request_typed(ClientRequest::ThreadUnsubscribe {
                request_id,
                params: ThreadUnsubscribeParams {
                    thread_id: params.session_id.clone(),
                },
            })
            .await
            .map_err(io_error_from_typed_request)?;
        self.sessions.remove(&params.session_id);
        self.send_result(request.id, serde_json::to_value(SessionCloseResult {})?)?;
        Ok(())
    }

    async fn handle_session_prompt(&mut self, request: JsonRpcRequest) -> IoResult<()> {
        self.require_initialized(&request.id)?;
        let params = self.parse_params::<SessionPromptParams>(request.params)?;
        let input = prompt_to_user_input(&params.prompt).map_err(invalid_params_error)?;

        let Some(session) = self.sessions.get(&params.session_id) else {
            self.send_error(request.id, -32602, "unknown sessionId")?;
            return Ok(());
        };
        if session.active_turn_id.is_some() {
            self.send_error(request.id, -32600, "session already has an active turn")?;
            return Ok(());
        }
        let thread_id = session.thread_id.clone();
        let collaboration_mode = Some(session.current_mode.clone());

        let request_id = self.next_app_request_id();
        let response: TurnStartResponse = self
            .app_client
            .request_typed(ClientRequest::TurnStart {
                request_id,
                params: TurnStartParams {
                    thread_id,
                    input,
                    collaboration_mode,
                    ..Default::default()
                },
            })
            .await
            .map_err(io_error_from_typed_request)?;

        let turn_id = response.turn.id;
        if let Some(session) = self.sessions.get_mut(&params.session_id) {
            session.active_turn_id = Some(turn_id.clone());
        }
        self.pending_prompts.insert(
            turn_id,
            PendingPrompt {
                request_id: request.id,
                session_id: params.session_id,
            },
        );
        Ok(())
    }

    async fn handle_session_set_mode(&mut self, request: JsonRpcRequest) -> IoResult<()> {
        self.require_initialized(&request.id)?;
        let params = self.parse_params::<SessionSetModeParams>(request.params)?;

        let Some(session) = self.sessions.get_mut(&params.session_id) else {
            self.send_error(request.id, -32602, "unknown sessionId")?;
            return Ok(());
        };

        let Some(binding) = session
            .modes
            .iter()
            .find(|binding| binding.acp_mode.id == params.mode_id)
            .cloned()
        else {
            self.send_error(
                request.id,
                -32602,
                format!("unknown modeId: {}", params.mode_id),
            )?;
            return Ok(());
        };

        session.current_mode_id = binding.acp_mode.id.clone();
        session.current_mode = session.current_mode.apply_mask(&binding.mask);
        update_mode_config_options(
            &mut session.config_options,
            &session.current_mode_id,
            session.current_mode.model(),
            session.current_mode.reasoning_effort(),
        );

        let config_options = session.config_options.clone();
        let current_mode_id = session.current_mode_id.clone();

        self.send_result(request.id, serde_json::to_value(SessionSetModeResult {})?)?;
        self.send_session_update(
            &params.session_id,
            SessionUpdate::CurrentModeUpdate { current_mode_id },
        )?;
        self.send_session_update(
            &params.session_id,
            SessionUpdate::ConfigOptionsUpdate { config_options },
        )?;
        Ok(())
    }

    async fn handle_session_cancel(&mut self, params: SessionCancelParams) -> IoResult<()> {
        let Some(active_turn_id) = self
            .sessions
            .get(&params.session_id)
            .and_then(|session| session.active_turn_id.clone())
        else {
            return Ok(());
        };

        let Some(thread_id) = self
            .sessions
            .get(&params.session_id)
            .map(|session| session.thread_id.clone())
        else {
            return Ok(());
        };
        let request_id = self.next_app_request_id();

        let _: codex_app_server_protocol::TurnInterruptResponse = self
            .app_client
            .request_typed(ClientRequest::TurnInterrupt {
                request_id,
                params: TurnInterruptParams {
                    thread_id,
                    turn_id: active_turn_id,
                },
            })
            .await
            .map_err(io_error_from_typed_request)?;
        Ok(())
    }

    async fn handle_mcp_message_request(&mut self, request: JsonRpcRequest) -> IoResult<()> {
        self.require_initialized(&request.id)?;
        let params = self.parse_params::<McpMessageParams>(request.params)?;
        let connection_id = params.connection_id.clone();
        let Some(inbound_tx) = self.bridge_state.connection_sender(&connection_id) else {
            self.send_error(
                request.id,
                -32602,
                format!("unknown MCP connectionId: {connection_id}"),
            )?;
            return Ok(());
        };
        inbound_tx
            .send(mcp_request_from_acp(request.id, params)?)
            .map_err(|_| Error::new(ErrorKind::BrokenPipe, "ACP MCP inbound channel closed"))?;
        Ok(())
    }

    fn handle_mcp_message_notification(&mut self, params: McpMessageParams) -> IoResult<()> {
        let connection_id = params.connection_id.clone();
        let Some(inbound_tx) = self.bridge_state.connection_sender(&connection_id) else {
            debug!("ignoring MCP notification for unknown connection {connection_id}");
            return Ok(());
        };
        inbound_tx
            .send(mcp_notification_from_acp(params)?)
            .map_err(|_| Error::new(ErrorKind::BrokenPipe, "ACP MCP inbound channel closed"))
    }

    async fn handle_mcp_disconnect(&mut self, request: JsonRpcRequest) -> IoResult<()> {
        self.require_initialized(&request.id)?;
        let params = self.parse_params::<McpDisconnectParams>(request.params)?;
        if self
            .bridge_state
            .connection_sender(&params.connection_id)
            .is_none()
        {
            self.send_error(
                request.id,
                -32602,
                format!("unknown MCP connectionId: {}", params.connection_id),
            )?;
            return Ok(());
        }
        self.send_result(
            request.id,
            serde_json::to_value(McpDisconnectResult { meta: None })
                .map_err(invalid_params_error)?,
        )?;
        self.bridge_state
            .unregister_connection(&params.connection_id);
        Ok(())
    }

    async fn handle_app_event(&mut self, event: InProcessServerEvent) -> IoResult<()> {
        match event {
            InProcessServerEvent::ServerNotification(notification) => {
                self.handle_server_notification(notification).await
            }
            InProcessServerEvent::ServerRequest(request) => {
                self.handle_server_request(request).await
            }
            InProcessServerEvent::LegacyNotification(_) | InProcessServerEvent::Lagged { .. } => {
                Ok(())
            }
        }
    }

    async fn handle_server_notification(
        &mut self,
        notification: ServerNotification,
    ) -> IoResult<()> {
        match notification {
            ServerNotification::AgentMessageDelta(update) => self.send_session_update(
                &update.thread_id,
                SessionUpdate::AgentMessageChunk {
                    content: text_content(update.delta),
                },
            ),
            ServerNotification::TurnPlanUpdated(update) => self.send_turn_plan_update(update),
            ServerNotification::ItemStarted(notification) => self.send_item_started(notification),
            ServerNotification::ItemCompleted(notification) => {
                self.send_item_completed(notification)
            }
            ServerNotification::TurnCompleted(notification) => {
                self.handle_turn_completed(notification).await
            }
            _ => Ok(()),
        }
    }

    async fn handle_server_request(&mut self, request: ServerRequest) -> IoResult<()> {
        match request {
            ServerRequest::CommandExecutionRequestApproval { request_id, params } => {
                self.forward_command_approval(request_id, params).await
            }
            ServerRequest::FileChangeRequestApproval { request_id, params } => {
                self.forward_file_change_approval(request_id, params).await
            }
            ServerRequest::ToolRequestUserInput { request_id, params } => {
                self.forward_tool_user_input(request_id, params).await
            }
            ServerRequest::McpServerElicitationRequest { request_id, params } => {
                self.forward_mcp_elicitation(request_id, params).await
            }
            other => {
                self.app_client
                    .reject_server_request(
                        other.id().clone(),
                        JSONRPCErrorError {
                            code: -32601,
                            data: None,
                            message: "ACP adapter does not support this interactive request yet"
                                .to_string(),
                        },
                    )
                    .await
            }
        }
    }

    async fn forward_command_approval(
        &mut self,
        app_request_id: RequestId,
        params: CommandExecutionRequestApprovalParams,
    ) -> IoResult<()> {
        let (options, option_results, fallback_result) =
            build_command_permission_options(&params.available_decisions);
        let acp_request_id = self.next_acp_request_id();
        let title = params
            .command
            .clone()
            .unwrap_or_else(|| "Approve command execution".to_string());
        self.pending_permissions.insert(
            acp_request_id.clone(),
            PendingPermissionRequest {
                app_request_id,
                option_results,
                fallback_result,
            },
        );
        self.send_request(
            acp_request_id,
            "session/request_permission",
            serde_json::to_value(SessionRequestPermissionParams {
                session_id: params.thread_id,
                tool_call: PermissionToolCall {
                    tool_call_id: params.item_id,
                    title,
                    kind: "exec".to_string(),
                    status: "pending".to_string(),
                },
                options,
            })?,
        )
    }

    async fn forward_file_change_approval(
        &mut self,
        app_request_id: RequestId,
        params: FileChangeRequestApprovalParams,
    ) -> IoResult<()> {
        let (options, option_results, fallback_result) = build_file_permission_options();
        let acp_request_id = self.next_acp_request_id();
        self.pending_permissions.insert(
            acp_request_id.clone(),
            PendingPermissionRequest {
                app_request_id,
                option_results,
                fallback_result,
            },
        );
        self.send_request(
            acp_request_id,
            "session/request_permission",
            serde_json::to_value(SessionRequestPermissionParams {
                session_id: params.thread_id,
                tool_call: PermissionToolCall {
                    tool_call_id: params.item_id,
                    title: "Approve file changes".to_string(),
                    kind: "edit".to_string(),
                    status: "pending".to_string(),
                },
                options,
            })?,
        )
    }

    async fn forward_tool_user_input(
        &mut self,
        app_request_id: RequestId,
        params: ToolRequestUserInputParams,
    ) -> IoResult<()> {
        if !self.client_supports_form_elicitation() {
            return self
                .app_client
                .reject_server_request(
                    app_request_id,
                    JSONRPCErrorError {
                        code: -32601,
                        data: None,
                        message: "ACP client does not advertise form elicitation support"
                            .to_string(),
                    },
                )
                .await;
        }

        let requested_schema =
            tool_user_input_schema(&params.questions).map_err(invalid_params_error)?;
        let acp_request_id = self.next_acp_request_id();
        self.pending_elicitations.insert(
            acp_request_id.clone(),
            PendingElicitationRequest {
                app_request_id,
                fallback_result: serde_json::to_value(ToolRequestUserInputResponse {
                    answers: HashMap::new(),
                })?,
                kind: PendingElicitationKind::ToolUserInput {
                    questions: params.questions.clone(),
                },
            },
        );
        self.send_request(
            acp_request_id,
            "session/elicitation",
            serde_json::to_value(SessionElicitationParams {
                session_id: params.thread_id,
                elicitation: SessionElicitationRequest::Form {
                    message: tool_user_input_message(&params.questions),
                    requested_schema,
                    meta: Some(tool_user_input_meta(
                        &params.turn_id,
                        &params.item_id,
                        &params.questions,
                    )),
                },
            })?,
        )
    }

    async fn forward_mcp_elicitation(
        &mut self,
        app_request_id: RequestId,
        params: McpServerElicitationRequestParams,
    ) -> IoResult<()> {
        let supports_mode = match &params.request {
            AppMcpServerElicitationRequest::Form { .. } => self.client_supports_form_elicitation(),
            AppMcpServerElicitationRequest::Url { .. } => self.client_supports_url_elicitation(),
        };
        if !supports_mode {
            let mode = match &params.request {
                AppMcpServerElicitationRequest::Form { .. } => "form",
                AppMcpServerElicitationRequest::Url { .. } => "url",
            };
            return self
                .app_client
                .reject_server_request(
                    app_request_id,
                    JSONRPCErrorError {
                        code: -32601,
                        data: None,
                        message: format!(
                            "ACP client does not advertise {mode} elicitation support"
                        ),
                    },
                )
                .await;
        }

        let acp_request_id = self.next_acp_request_id();
        self.pending_elicitations.insert(
            acp_request_id.clone(),
            PendingElicitationRequest {
                app_request_id,
                fallback_result: serde_json::to_value(McpServerElicitationRequestResponse {
                    action: codex_app_server_protocol::McpServerElicitationAction::Decline,
                    content: None,
                    meta: None,
                })?,
                kind: PendingElicitationKind::McpServer,
            },
        );
        self.send_request(
            acp_request_id,
            "session/elicitation",
            serde_json::to_value(SessionElicitationParams {
                session_id: params.thread_id.clone(),
                elicitation: app_elicitation_to_acp_request(params),
            })?,
        )
    }

    async fn build_session_state(
        &mut self,
        thread_id: String,
        cwd: String,
        last_title: Option<String>,
        current_model: &str,
        current_reasoning_effort: Option<ReasoningEffort>,
    ) -> IoResult<SessionState> {
        let mode_bindings = self.list_mode_bindings().await?;
        let current_mode =
            default_collaboration_mode(&mode_bindings, current_model, current_reasoning_effort);
        let current_mode_id = mode_bindings
            .first()
            .map(|binding| binding.acp_mode.id.clone())
            .unwrap_or_else(|| "default".to_string());
        let models = self.list_models().await?;
        let config_options = build_config_options(
            &mode_bindings,
            current_model,
            current_reasoning_effort,
            &current_mode_id,
            &models,
        );

        Ok(SessionState {
            active_turn_id: None,
            config_options,
            current_mode,
            current_mode_id,
            cwd,
            last_title,
            modes: mode_bindings,
            thread_id,
        })
    }

    async fn list_mode_bindings(&mut self) -> IoResult<Vec<ModeBinding>> {
        let request_id = self.next_app_request_id();
        let response: CollaborationModeListResponse = self
            .app_client
            .request_typed(ClientRequest::CollaborationModeList {
                request_id,
                params: CollaborationModeListParams::default(),
            })
            .await
            .map_err(io_error_from_typed_request)?;

        let mode_bindings = response
            .data
            .into_iter()
            .map(|mask| ModeBinding {
                acp_mode: Mode {
                    id: mode_id(&mask.name),
                    name: mask.name.clone(),
                    description: None,
                },
                mask: core_mode_mask(&mask),
            })
            .collect::<Vec<_>>();

        Ok(if mode_bindings.is_empty() {
            vec![fallback_mode_binding()]
        } else {
            mode_bindings
        })
    }

    async fn list_models(&mut self) -> IoResult<Vec<Model>> {
        let request_id = self.next_app_request_id();
        let response: ModelListResponse = self
            .app_client
            .request_typed(ClientRequest::ModelList {
                request_id,
                params: ModelListParams::default(),
            })
            .await
            .map_err(io_error_from_typed_request)?;
        Ok(response.data)
    }

    async fn replay_thread_history(
        &mut self,
        session_id: &str,
        response: ThreadReadResponse,
    ) -> IoResult<()> {
        for turn in response.thread.turns {
            for item in turn.items {
                match item {
                    ThreadItem::UserMessage { content, .. } => {
                        for text in flatten_user_message_text(&content) {
                            self.send_session_update(
                                session_id,
                                SessionUpdate::UserMessageChunk {
                                    content: text_content(text),
                                },
                            )?;
                        }
                    }
                    ThreadItem::AgentMessage { text, .. } => {
                        self.send_session_update(
                            session_id,
                            SessionUpdate::AgentMessageChunk {
                                content: text_content(text),
                            },
                        )?;
                    }
                    ThreadItem::Plan { text, .. } => {
                        self.send_session_update(
                            session_id,
                            SessionUpdate::Plan {
                                entries: vec![PlanEntry {
                                    content: text,
                                    priority: "medium".to_string(),
                                    status: "completed".to_string(),
                                }],
                            },
                        )?;
                    }
                    ThreadItem::CommandExecution {
                        id,
                        command,
                        cwd,
                        status,
                        aggregated_output,
                        ..
                    } => {
                        self.send_session_update(
                            session_id,
                            SessionUpdate::ToolCall {
                                tool_call_id: id.clone(),
                                title: command.clone(),
                                kind: "exec".to_string(),
                                status: "completed".to_string(),
                                locations: Some(vec![Location {
                                    path: cwd.display().to_string(),
                                }]),
                            },
                        )?;
                        self.send_session_update(
                            session_id,
                            SessionUpdate::ToolCallUpdate {
                                tool_call_id: id,
                                status: map_command_status(status),
                                content: tool_call_text_output(
                                    aggregated_output
                                        .or(Some(format!("{command}\n({})", cwd.display()))),
                                ),
                            },
                        )?;
                    }
                    ThreadItem::FileChange {
                        id,
                        changes,
                        status,
                    } => {
                        let locations = changes
                            .iter()
                            .map(|change| Location {
                                path: change.path.clone(),
                            })
                            .collect::<Vec<_>>();
                        self.send_session_update(
                            session_id,
                            SessionUpdate::ToolCall {
                                tool_call_id: id.clone(),
                                title: "Applying file changes".to_string(),
                                kind: "edit".to_string(),
                                status: "completed".to_string(),
                                locations: Some(locations),
                            },
                        )?;
                        self.send_session_update(
                            session_id,
                            SessionUpdate::ToolCallUpdate {
                                tool_call_id: id,
                                status: map_patch_status(status),
                                content: tool_call_text_output(Some(
                                    changes
                                        .into_iter()
                                        .map(|change| change.diff)
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                )),
                            },
                        )?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    async fn handle_turn_completed(
        &mut self,
        notification: TurnCompletedNotification,
    ) -> IoResult<()> {
        let turn_id = notification.turn.id.clone();
        if let Some(pending_prompt) = self.pending_prompts.remove(&turn_id) {
            if let Some(session) = self.sessions.get_mut(&pending_prompt.session_id)
                && session.active_turn_id.as_deref() == Some(turn_id.as_str())
            {
                session.active_turn_id = None;
            }

            match notification.turn.status {
                TurnStatus::Completed => {
                    let _ = self.send_result(
                        pending_prompt.request_id,
                        serde_json::to_value(SessionPromptResult {
                            stop_reason: "end_turn".to_string(),
                        })
                        .unwrap_or(JsonValue::Null),
                    );
                }
                TurnStatus::Interrupted => {
                    let _ = self.send_result(
                        pending_prompt.request_id,
                        serde_json::to_value(SessionPromptResult {
                            stop_reason: "cancelled".to_string(),
                        })
                        .unwrap_or(JsonValue::Null),
                    );
                }
                TurnStatus::Failed => {
                    let message = notification
                        .turn
                        .error
                        .map(|error| error.message)
                        .unwrap_or_else(|| "turn failed".to_string());
                    let _ = self.send_error(pending_prompt.request_id, -32000, message);
                }
                TurnStatus::InProgress => {}
            }
        }

        let request_id = self.next_app_request_id();
        let response: ThreadReadResponse = self
            .app_client
            .request_typed(ClientRequest::ThreadRead {
                request_id,
                params: ThreadReadParams {
                    thread_id: notification.thread_id.clone(),
                    include_turns: false,
                },
            })
            .await
            .map_err(io_error_from_typed_request)?;
        self.maybe_update_session_info(
            &notification.thread_id,
            response.thread.name,
            rfc3339_timestamp(response.thread.updated_at),
        )?;
        Ok(())
    }

    fn send_turn_plan_update(&mut self, update: TurnPlanUpdatedNotification) -> IoResult<()> {
        self.send_session_update(
            &update.thread_id,
            SessionUpdate::Plan {
                entries: update
                    .plan
                    .into_iter()
                    .map(|step| PlanEntry {
                        content: step.step,
                        priority: "medium".to_string(),
                        status: match step.status {
                            TurnPlanStepStatus::Pending => "pending".to_string(),
                            TurnPlanStepStatus::InProgress => "in_progress".to_string(),
                            TurnPlanStepStatus::Completed => "completed".to_string(),
                        },
                    })
                    .collect(),
            },
        )
    }

    fn send_item_started(&mut self, notification: ItemStartedNotification) -> IoResult<()> {
        match notification.item {
            ThreadItem::CommandExecution {
                id, command, cwd, ..
            } => self.send_session_update(
                &notification.thread_id,
                SessionUpdate::ToolCall {
                    tool_call_id: id,
                    title: command,
                    kind: "exec".to_string(),
                    status: "in_progress".to_string(),
                    locations: Some(vec![Location {
                        path: cwd.display().to_string(),
                    }]),
                },
            ),
            ThreadItem::FileChange { id, changes, .. } => self.send_session_update(
                &notification.thread_id,
                SessionUpdate::ToolCall {
                    tool_call_id: id,
                    title: "Applying file changes".to_string(),
                    kind: "edit".to_string(),
                    status: "in_progress".to_string(),
                    locations: Some(
                        changes
                            .into_iter()
                            .map(|change| Location { path: change.path })
                            .collect(),
                    ),
                },
            ),
            _ => Ok(()),
        }
    }

    fn send_item_completed(&mut self, notification: ItemCompletedNotification) -> IoResult<()> {
        match notification.item {
            ThreadItem::CommandExecution {
                id,
                status,
                aggregated_output,
                ..
            } => self.send_session_update(
                &notification.thread_id,
                SessionUpdate::ToolCallUpdate {
                    tool_call_id: id,
                    status: map_command_status(status),
                    content: tool_call_text_output(aggregated_output),
                },
            ),
            ThreadItem::FileChange {
                id,
                status,
                changes,
            } => self.send_session_update(
                &notification.thread_id,
                SessionUpdate::ToolCallUpdate {
                    tool_call_id: id,
                    status: map_patch_status(status),
                    content: tool_call_text_output(Some(
                        changes
                            .into_iter()
                            .map(|change| change.diff)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )),
                },
            ),
            _ => Ok(()),
        }
    }

    fn client_supports_form_elicitation(&self) -> bool {
        self.client_elicitation_capabilities
            .as_ref()
            .is_some_and(|caps| caps.form.is_some() || caps.url.is_none())
    }

    fn client_supports_url_elicitation(&self) -> bool {
        self.client_elicitation_capabilities
            .as_ref()
            .is_some_and(|caps| caps.url.is_some())
    }

    fn maybe_send_session_info_update(
        &mut self,
        session_id: &str,
        updated_at: Option<String>,
    ) -> IoResult<()> {
        let Some(session) = self.sessions.get(session_id) else {
            return Ok(());
        };
        if session.last_title.is_none() && updated_at.is_none() {
            return Ok(());
        }
        self.send_session_update(
            session_id,
            SessionUpdate::SessionInfoUpdate {
                title: session.last_title.clone(),
                updated_at,
                meta: None,
            },
        )
    }

    fn maybe_update_session_info(
        &mut self,
        session_id: &str,
        title: Option<String>,
        updated_at: Option<String>,
    ) -> IoResult<()> {
        let Some(session) = self.sessions.get_mut(session_id) else {
            return Ok(());
        };
        if session.last_title == title {
            if updated_at.is_none() {
                return Ok(());
            }
        } else {
            session.last_title = title.clone();
        }

        self.send_session_update(
            session_id,
            SessionUpdate::SessionInfoUpdate {
                title,
                updated_at,
                meta: None,
            },
        )
    }

    fn parse_params<T>(&self, params: Option<JsonValue>) -> IoResult<T>
    where
        T: DeserializeOwned,
    {
        serde_json::from_value(params.unwrap_or(JsonValue::Null)).map_err(invalid_params_error)
    }

    fn require_initialized(&self, request_id: &RequestId) -> IoResult<()> {
        if self.initialized {
            return Ok(());
        }

        self.send_error(request_id.clone(), -32002, "server not initialized")
    }

    fn next_acp_request_id(&mut self) -> RequestId {
        let request_id = self.next_acp_request_id;
        self.next_acp_request_id += 1;
        RequestId::Integer(request_id)
    }

    fn next_app_request_id(&mut self) -> RequestId {
        let request_id = self.next_app_request_id;
        self.next_app_request_id += 1;
        RequestId::Integer(request_id)
    }

    fn send_session_update(&mut self, session_id: &str, update: SessionUpdate) -> IoResult<()> {
        self.send_notification(
            "session/update",
            serde_json::to_value(SessionUpdateParams {
                session_id: session_id.to_string(),
                update,
            })?,
        )
    }

    fn send_notification(&self, method: &str, params: JsonValue) -> IoResult<()> {
        self.outgoing_tx
            .send(JsonRpcMessage::Notification(JsonRpcNotification {
                jsonrpc: JSONRPC_VERSION.to_string(),
                method: method.to_string(),
                params: Some(params),
            }))
            .map_err(|_| Error::new(ErrorKind::BrokenPipe, "ACP stdout writer channel closed"))
    }

    fn send_request(&self, id: RequestId, method: &str, params: JsonValue) -> IoResult<()> {
        self.outgoing_tx
            .send(JsonRpcMessage::Request(JsonRpcRequest {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id,
                method: method.to_string(),
                params: Some(params),
            }))
            .map_err(|_| Error::new(ErrorKind::BrokenPipe, "ACP stdout writer channel closed"))
    }

    fn send_result(&self, id: RequestId, result: JsonValue) -> IoResult<()> {
        self.outgoing_tx
            .send(JsonRpcMessage::Response(JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id,
                result,
            }))
            .map_err(|_| Error::new(ErrorKind::BrokenPipe, "ACP stdout writer channel closed"))
    }

    fn send_error(&self, id: RequestId, code: i64, message: impl Into<String>) -> IoResult<()> {
        self.outgoing_tx
            .send(JsonRpcMessage::Error(JsonRpcError {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id,
                error: JsonRpcErrorBody {
                    code,
                    message: message.into(),
                    data: None,
                },
            }))
            .map_err(|_| Error::new(ErrorKind::BrokenPipe, "ACP stdout writer channel closed"))
    }
}

pub async fn run_main(
    arg0_paths: Arg0DispatchPaths,
    cli_config_overrides: CliConfigOverrides,
) -> IoResult<()> {
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(EnvFilter::from_default_env());
    let _ = tracing_subscriber::registry().with(fmt_layer).try_init();

    let cli_kv_overrides = cli_config_overrides.parse_overrides().map_err(|error| {
        Error::new(
            ErrorKind::InvalidInput,
            format!("error parsing -c overrides: {error}"),
        )
    })?;
    let config = Config::load_with_cli_overrides(cli_kv_overrides.clone()).await?;
    let auth_manager = AuthManager::shared(
        config.codex_home.clone(),
        false,
        config.cli_auth_credentials_store_mode,
    );
    let cloud_requirements = cloud_requirements_loader(
        auth_manager,
        config.chatgpt_base_url.clone(),
        config.codex_home.clone(),
    );
    let config_warnings = config
        .startup_warnings
        .iter()
        .map(
            |warning| codex_app_server_protocol::ConfigWarningNotification {
                summary: warning.clone(),
                details: None,
                path: None,
                range: None,
            },
        )
        .collect();
    let (incoming_tx, incoming_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<JsonRpcMessage>();
    let bridge_state = Arc::new(AcpBridgeState::new(outgoing_tx.clone()));
    let app_client = InProcessAppServerClient::start(InProcessClientStartArgs {
        arg0_paths,
        config: Arc::new(config),
        cli_overrides: cli_kv_overrides,
        loader_overrides: LoaderOverrides::default(),
        cloud_requirements,
        feedback: CodexFeedback::new(),
        config_warnings,
        session_source: SessionSource::Acp,
        enable_codex_api_key_env: true,
        client_name: CLIENT_NAME.to_string(),
        client_version: env!("CARGO_PKG_VERSION").to_string(),
        experimental_api: true,
        opt_out_notification_methods: Vec::new(),
        channel_capacity: DEFAULT_IN_PROCESS_CHANNEL_CAPACITY,
    })
    .await?;
    set_acp_bridge(Some(Arc::new(AcpBridgeHandle {
        bridge_state: Arc::clone(&bridge_state),
    })));

    let stdin_reader_handle = tokio::spawn(async move {
        let stdin = io::stdin();
        let reader = BufReader::new(stdin);
        let mut lines = reader.lines();

        while let Some(line) = lines.next_line().await.unwrap_or_default() {
            match serde_json::from_str::<JsonRpcMessage>(&line) {
                Ok(message) => {
                    if incoming_tx.send(message).await.is_err() {
                        break;
                    }
                }
                Err(error) => error!("failed to deserialize ACP JSON-RPC message: {error}"),
            }
        }

        debug!("ACP stdin reader finished (EOF)");
    });

    let stdout_writer_handle = tokio::spawn(async move {
        let mut stdout = io::stdout();
        while let Some(message) = outgoing_rx.recv().await {
            match serde_json::to_string(&message) {
                Ok(json) => {
                    if stdout.write_all(json.as_bytes()).await.is_err() {
                        break;
                    }
                    if stdout.write_all(b"\n").await.is_err() {
                        break;
                    }
                }
                Err(error) => error!("failed to serialize ACP JSON-RPC message: {error}"),
            }
        }
        info!("ACP stdout writer finished");
    });

    let server_result = AcpServer {
        app_client,
        bridge_state,
        client_elicitation_capabilities: None,
        incoming_rx,
        initialized: false,
        next_acp_request_id: 1,
        next_app_request_id: 1,
        outgoing_tx,
        pending_elicitations: HashMap::new(),
        pending_permissions: HashMap::new(),
        pending_prompts: HashMap::new(),
        sessions: HashMap::new(),
    }
    .run()
    .await;
    set_acp_bridge(None);

    let _ = tokio::join!(stdin_reader_handle, stdout_writer_handle);
    server_result
}

fn prompt_to_user_input(prompt: &[PromptContent]) -> Result<Vec<UserInput>, Error> {
    let mut input = Vec::new();
    for content in prompt {
        match content {
            PromptContent::Text { text } => input.push(UserInput::Text {
                text: text.clone(),
                text_elements: Vec::new(),
            }),
            PromptContent::Resource { resource } => input.push(resource_to_user_input(resource)?),
        }
    }
    if input.is_empty() {
        return Err(invalid_params_error(
            "prompt must contain at least one text item",
        ));
    }
    Ok(input)
}

fn session_elicitation_to_app_result(
    response: SessionElicitationResponse,
    kind: &PendingElicitationKind,
) -> JsonValue {
    match kind {
        PendingElicitationKind::ToolUserInput { questions } => serde_json::to_value(
            tool_user_input_response_from_elicitation(response, questions),
        )
        .unwrap_or(JsonValue::Null),
        PendingElicitationKind::McpServer => {
            serde_json::to_value(McpServerElicitationRequestResponse {
                action: match response.action {
                    ElicitationAction::Accept => {
                        codex_app_server_protocol::McpServerElicitationAction::Accept
                    }
                    ElicitationAction::Decline => {
                        codex_app_server_protocol::McpServerElicitationAction::Decline
                    }
                    ElicitationAction::Cancel => {
                        codex_app_server_protocol::McpServerElicitationAction::Cancel
                    }
                },
                content: response.content,
                meta: response.meta,
            })
            .unwrap_or(JsonValue::Null)
        }
    }
}

fn tool_user_input_response_from_elicitation(
    response: SessionElicitationResponse,
    questions: &[ToolRequestUserInputQuestion],
) -> ToolRequestUserInputResponse {
    if !matches!(response.action, ElicitationAction::Accept) {
        return ToolRequestUserInputResponse {
            answers: HashMap::new(),
        };
    }

    let Some(JsonValue::Object(content)) = response.content else {
        return ToolRequestUserInputResponse {
            answers: HashMap::new(),
        };
    };

    ToolRequestUserInputResponse {
        answers: questions
            .iter()
            .filter_map(|question| {
                let value = content.get(&question.id)?;
                let answers = match value {
                    JsonValue::String(value) => vec![value.clone()],
                    JsonValue::Array(values) => values
                        .iter()
                        .filter_map(|value| value.as_str().map(ToOwned::to_owned))
                        .collect(),
                    _ => Vec::new(),
                };
                Some((question.id.clone(), ToolRequestUserInputAnswer { answers }))
            })
            .collect(),
    }
}

fn app_elicitation_to_acp_request(
    params: McpServerElicitationRequestParams,
) -> SessionElicitationRequest {
    match params.request {
        AppMcpServerElicitationRequest::Form {
            meta,
            message,
            requested_schema,
        } => SessionElicitationRequest::Form {
            message,
            requested_schema: serde_json::to_value(requested_schema).unwrap_or(JsonValue::Null),
            meta: merge_json_objects(
                meta,
                serde_json::json!({
                    "serverName": params.server_name,
                    "turnId": params.turn_id,
                }),
            ),
        },
        AppMcpServerElicitationRequest::Url {
            meta,
            message,
            url,
            elicitation_id,
        } => SessionElicitationRequest::Url {
            message,
            url,
            elicitation_id,
            meta: merge_json_objects(
                meta,
                serde_json::json!({
                    "serverName": params.server_name,
                    "turnId": params.turn_id,
                }),
            ),
        },
    }
}

fn tool_user_input_schema(
    questions: &[ToolRequestUserInputQuestion],
) -> Result<JsonValue, serde_json::Error> {
    let properties = questions
        .iter()
        .map(|question| {
            let schema = match &question.options {
                Some(options) if !options.is_empty() && !question.is_other => {
                    serde_json::json!({
                        "type": "string",
                        "title": question.header,
                        "description": question.question,
                        "enum": options.iter().map(|option| option.label.clone()).collect::<Vec<_>>(),
                    })
                }
                _ => serde_json::json!({
                    "type": "string",
                    "title": question.header,
                    "description": question.question,
                }),
            };
            Ok((question.id.clone(), schema))
        })
        .collect::<Result<serde_json::Map<String, JsonValue>, serde_json::Error>>()?;
    Ok(JsonValue::Object(serde_json::Map::from_iter([
        ("type".to_string(), JsonValue::String("object".to_string())),
        ("properties".to_string(), JsonValue::Object(properties)),
        (
            "required".to_string(),
            JsonValue::Array(
                questions
                    .iter()
                    .map(|question| JsonValue::String(question.id.clone()))
                    .collect(),
            ),
        ),
    ])))
}

fn tool_user_input_message(questions: &[ToolRequestUserInputQuestion]) -> String {
    questions
        .iter()
        .map(|question| format!("{}: {}", question.header, question.question))
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_user_input_meta(
    turn_id: &str,
    item_id: &str,
    questions: &[ToolRequestUserInputQuestion],
) -> JsonValue {
    serde_json::json!({
        "turnId": turn_id,
        "itemId": item_id,
        "questions": questions.iter().map(|question| {
            serde_json::json!({
                "id": question.id,
                "header": question.header,
                "question": question.question,
                "isOther": question.is_other,
                "isSecret": question.is_secret,
                "options": question.options.as_ref().map(|options| {
                    options.iter().map(|option| {
                        serde_json::json!({
                            "label": option.label,
                            "description": option.description,
                        })
                    }).collect::<Vec<_>>()
                }),
            })
        }).collect::<Vec<_>>(),
    })
}

fn merge_json_objects(left: Option<JsonValue>, right: JsonValue) -> Option<JsonValue> {
    match (left, right) {
        (Some(JsonValue::Object(mut left)), JsonValue::Object(right)) => {
            left.extend(right);
            Some(JsonValue::Object(left))
        }
        (Some(value), _) => Some(value),
        (None, value) => Some(value),
    }
}

fn rfc3339_timestamp(seconds: i64) -> Option<String> {
    chrono::DateTime::<Utc>::from_timestamp(seconds, 0)
        .map(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn resource_to_user_input(resource: &crate::protocol::ResourceContent) -> Result<UserInput, Error> {
    if let Some(text) = resource.text.as_ref() {
        return Ok(UserInput::Text {
            text: format_resource_text(resource, text),
            text_elements: Vec::new(),
        });
    }

    if resource
        .mime_type
        .as_deref()
        .is_some_and(|mime_type| mime_type.starts_with("image/"))
    {
        if let Some(path) = file_uri_to_path(&resource.uri) {
            return Ok(UserInput::LocalImage { path });
        }
        if resource.uri.starts_with("http://") || resource.uri.starts_with("https://") {
            return Ok(UserInput::Image {
                url: resource.uri.clone(),
            });
        }
    }

    Err(invalid_params_error(format!(
        "unsupported resource prompt item: {}",
        resource.uri
    )))
}

fn format_resource_text(resource: &crate::protocol::ResourceContent, text: &str) -> String {
    let mime_type = resource.mime_type.as_deref().unwrap_or("text/plain");
    format!(
        "<resource uri=\"{}\" mimeType=\"{mime_type}\">\n{text}\n</resource>",
        resource.uri
    )
}

fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    uri.strip_prefix("file://").map(PathBuf::from)
}

fn build_command_permission_options(
    available_decisions: &Option<Vec<CommandExecutionApprovalDecision>>,
) -> (Vec<PermissionOption>, HashMap<String, JsonValue>, JsonValue) {
    let mut options = Vec::new();
    let mut option_results = HashMap::new();
    let mut decisions = available_decisions.clone().unwrap_or_else(|| {
        vec![
            CommandExecutionApprovalDecision::Accept,
            CommandExecutionApprovalDecision::Decline,
            CommandExecutionApprovalDecision::Cancel,
        ]
    });
    if decisions.is_empty() {
        decisions = vec![
            CommandExecutionApprovalDecision::Accept,
            CommandExecutionApprovalDecision::Decline,
            CommandExecutionApprovalDecision::Cancel,
        ];
    }

    for decision in decisions {
        let (option_id, name, kind, response) = match decision {
            CommandExecutionApprovalDecision::Accept => (
                "accept",
                "Accept",
                "allow_once",
                CommandExecutionRequestApprovalResponse {
                    decision: CommandExecutionApprovalDecision::Accept,
                },
            ),
            CommandExecutionApprovalDecision::AcceptForSession => (
                "accept-session",
                "Accept for session",
                "allow_always",
                CommandExecutionRequestApprovalResponse {
                    decision: CommandExecutionApprovalDecision::AcceptForSession,
                },
            ),
            CommandExecutionApprovalDecision::Decline => (
                "decline",
                "Decline",
                "reject_once",
                CommandExecutionRequestApprovalResponse {
                    decision: CommandExecutionApprovalDecision::Decline,
                },
            ),
            CommandExecutionApprovalDecision::Cancel => (
                "cancel",
                "Cancel turn",
                "reject_once",
                CommandExecutionRequestApprovalResponse {
                    decision: CommandExecutionApprovalDecision::Cancel,
                },
            ),
            CommandExecutionApprovalDecision::AcceptWithExecpolicyAmendment { .. }
            | CommandExecutionApprovalDecision::ApplyNetworkPolicyAmendment { .. } => {
                continue;
            }
        };
        options.push(PermissionOption {
            option_id: option_id.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
        });
        option_results.insert(
            option_id.to_string(),
            serde_json::to_value(response).unwrap_or(JsonValue::Null),
        );
    }

    let fallback_result = serde_json::to_value(CommandExecutionRequestApprovalResponse {
        decision: CommandExecutionApprovalDecision::Cancel,
    })
    .unwrap_or(JsonValue::Null);

    (options, option_results, fallback_result)
}

fn build_file_permission_options() -> (Vec<PermissionOption>, HashMap<String, JsonValue>, JsonValue)
{
    let mut options = Vec::new();
    let mut option_results = HashMap::new();
    for (option_id, name, kind, decision) in [
        (
            "accept",
            "Accept",
            "allow_once",
            FileChangeApprovalDecision::Accept,
        ),
        (
            "accept-session",
            "Accept for session",
            "allow_always",
            FileChangeApprovalDecision::AcceptForSession,
        ),
        (
            "decline",
            "Decline",
            "reject_once",
            FileChangeApprovalDecision::Decline,
        ),
        (
            "cancel",
            "Cancel turn",
            "reject_once",
            FileChangeApprovalDecision::Cancel,
        ),
    ] {
        options.push(PermissionOption {
            option_id: option_id.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
        });
        option_results.insert(
            option_id.to_string(),
            serde_json::to_value(FileChangeRequestApprovalResponse { decision })
                .unwrap_or(JsonValue::Null),
        );
    }

    let fallback_result = serde_json::to_value(FileChangeRequestApprovalResponse {
        decision: FileChangeApprovalDecision::Cancel,
    })
    .unwrap_or(JsonValue::Null);

    (options, option_results, fallback_result)
}

fn build_request_overrides_from_mcp_servers(
    mcp_servers: &[JsonValue],
) -> Result<Option<HashMap<String, JsonValue>>, Error> {
    if mcp_servers.is_empty() {
        return Ok(None);
    }

    let mut server_map = serde_json::Map::new();
    for server in mcp_servers {
        let (name, config) = acp_mcp_server_to_config(server)?;
        server_map.insert(
            name,
            serde_json::to_value(config).map_err(invalid_params_error)?,
        );
    }

    Ok(Some(HashMap::from([(
        "mcp_servers".to_string(),
        JsonValue::Object(server_map),
    )])))
}

fn acp_mcp_server_to_config(server: &JsonValue) -> Result<(String, McpServerConfig), Error> {
    let object = server
        .as_object()
        .ok_or_else(|| invalid_params_error("mcpServers entries must be objects"))?;
    let name = object
        .get("name")
        .and_then(JsonValue::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| invalid_params_error("mcpServers entries must include a non-empty name"))?
        .to_string();

    let transport = object
        .get("transport")
        .or_else(|| object.get("type"))
        .and_then(JsonValue::as_str);

    let transport = match transport {
        Some("acp") => build_acp_transport(object)?,
        Some("http") | Some("streamable_http") => build_http_transport(object)?,
        Some("stdio") => build_stdio_transport(object)?,
        Some(other) => {
            return Err(invalid_params_error(format!(
                "mcpServers entry '{name}' has unsupported transport '{other}'"
            )));
        }
        None if object.contains_key("url") => build_http_transport(object)?,
        None if object.contains_key("command") => build_stdio_transport(object)?,
        None => {
            return Err(invalid_params_error(format!(
                "mcpServers entry '{name}' must include either transport, url, or command"
            )));
        }
    };

    Ok((
        name,
        McpServerConfig {
            transport,
            enabled: true,
            required: false,
            disabled_reason: None,
            startup_timeout_sec: None,
            tool_timeout_sec: None,
            enabled_tools: None,
            disabled_tools: None,
            scopes: None,
            oauth_resource: None,
        },
    ))
}

fn build_stdio_transport(
    object: &serde_json::Map<String, JsonValue>,
) -> Result<McpServerTransportConfig, Error> {
    let command = object
        .get("command")
        .and_then(JsonValue::as_str)
        .filter(|command| !command.is_empty())
        .ok_or_else(|| invalid_params_error("stdio MCP server must include a non-empty command"))?
        .to_string();
    let args = object
        .get("args")
        .and_then(JsonValue::as_array)
        .map(|args| {
            args.iter()
                .map(|value| {
                    value.as_str().map(str::to_string).ok_or_else(|| {
                        invalid_params_error("stdio MCP server args must be strings")
                    })
                })
                .collect::<Result<Vec<_>, Error>>()
        })
        .transpose()?
        .unwrap_or_default();
    let env = parse_name_value_map(object.get("env"), "stdio MCP server env")?;

    Ok(McpServerTransportConfig::Stdio {
        command,
        args,
        env,
        env_vars: Vec::new(),
        cwd: None,
    })
}

fn build_http_transport(
    object: &serde_json::Map<String, JsonValue>,
) -> Result<McpServerTransportConfig, Error> {
    let url = object
        .get("url")
        .and_then(JsonValue::as_str)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| invalid_params_error("HTTP MCP server must include a non-empty url"))?
        .to_string();
    let http_headers = parse_name_value_map(object.get("headers"), "HTTP MCP server headers")?;

    Ok(McpServerTransportConfig::StreamableHttp {
        url,
        bearer_token_env_var: None,
        http_headers,
        env_http_headers: None,
    })
}

fn build_acp_transport(
    object: &serde_json::Map<String, JsonValue>,
) -> Result<McpServerTransportConfig, Error> {
    let id = object
        .get("id")
        .and_then(JsonValue::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| invalid_params_error("ACP MCP server must include a non-empty id"))?
        .to_string();

    Ok(McpServerTransportConfig::Acp {
        transport: "acp".to_string(),
        id,
    })
}

fn parse_name_value_map(
    value: Option<&JsonValue>,
    label: &str,
) -> Result<Option<HashMap<String, String>>, Error> {
    let Some(value) = value else {
        return Ok(None);
    };

    if let Some(object) = value.as_object() {
        let map = object
            .iter()
            .map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key.clone(), value.to_string()))
                    .ok_or_else(|| invalid_params_error(format!("{label} values must be strings")))
            })
            .collect::<Result<HashMap<_, _>, Error>>()?;
        return Ok(Some(map));
    }

    if let Some(items) = value.as_array() {
        let map = items
            .iter()
            .map(|item| {
                let item = item.as_object().ok_or_else(|| {
                    invalid_params_error(format!("{label} entries must be objects"))
                })?;
                let name = item
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| {
                        invalid_params_error(format!(
                            "{label} entries must include a non-empty name"
                        ))
                    })?;
                let value = item
                    .get("value")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        invalid_params_error(format!("{label} entries must include a string value"))
                    })?;
                Ok((name.to_string(), value.to_string()))
            })
            .collect::<Result<HashMap<_, _>, Error>>()?;
        return Ok(Some(map));
    }

    Err(invalid_params_error(format!(
        "{label} must be an object or an array of name/value pairs"
    )))
}

fn fallback_mode_binding() -> ModeBinding {
    let name = "Default".to_string();
    ModeBinding {
        acp_mode: Mode {
            id: mode_id(&name),
            name: name.clone(),
            description: None,
        },
        mask: CoreCollaborationModeMask {
            name,
            mode: None,
            model: None,
            reasoning_effort: None,
            developer_instructions: None,
        },
    }
}

fn core_mode_mask(mask: &CollaborationModeMask) -> CoreCollaborationModeMask {
    CoreCollaborationModeMask {
        name: mask.name.clone(),
        mode: mask.mode,
        model: mask.model.clone(),
        reasoning_effort: mask.reasoning_effort,
        developer_instructions: None,
    }
}

fn default_collaboration_mode(
    mode_bindings: &[ModeBinding],
    model: &str,
    reasoning_effort: Option<ReasoningEffort>,
) -> CollaborationMode {
    let base = CollaborationMode {
        mode: mode_bindings
            .first()
            .and_then(|binding| binding.mask.mode)
            .unwrap_or_default(),
        settings: Settings {
            model: model.to_string(),
            reasoning_effort,
            developer_instructions: None,
        },
    };

    mode_bindings
        .first()
        .map(|binding| base.apply_mask(&binding.mask))
        .unwrap_or(base)
}

fn build_config_options(
    mode_bindings: &[ModeBinding],
    current_model: &str,
    current_reasoning_effort: Option<ReasoningEffort>,
    current_mode_id: &str,
    models: &[Model],
) -> Vec<ConfigOption> {
    let mut config_options = vec![ConfigOption {
        id: "mode".to_string(),
        name: "Session Mode".to_string(),
        description: Some("Controls how Codex behaves for future turns.".to_string()),
        category: Some("mode".to_string()),
        option_type: "select".to_string(),
        current_value: current_mode_id.to_string(),
        options: mode_bindings
            .iter()
            .map(|binding| ConfigOptionValue {
                value: binding.acp_mode.id.clone(),
                name: binding.acp_mode.name.clone(),
                description: binding.acp_mode.description.clone(),
            })
            .collect(),
    }];

    if !models.is_empty() {
        config_options.push(ConfigOption {
            id: "model".to_string(),
            name: "Model".to_string(),
            description: Some("Current model for this session.".to_string()),
            category: Some("model".to_string()),
            option_type: "select".to_string(),
            current_value: current_model.to_string(),
            options: models
                .iter()
                .map(|model| ConfigOptionValue {
                    value: model.model.clone(),
                    name: model.display_name.clone(),
                    description: Some(model.description.clone()),
                })
                .collect(),
        });

        if let Some(current_model_info) = models.iter().find(|model| model.model == current_model) {
            config_options.push(ConfigOption {
                id: "thought_level".to_string(),
                name: "Thought Level".to_string(),
                description: Some("Reasoning effort for the current model.".to_string()),
                category: Some("thought_level".to_string()),
                option_type: "select".to_string(),
                current_value: reasoning_effort_id(
                    current_reasoning_effort.unwrap_or(current_model_info.default_reasoning_effort),
                ),
                options: current_model_info
                    .supported_reasoning_efforts
                    .iter()
                    .map(|effort| ConfigOptionValue {
                        value: reasoning_effort_id(effort.reasoning_effort),
                        name: reasoning_effort_name(effort.reasoning_effort),
                        description: Some(effort.description.clone()),
                    })
                    .collect(),
            });
        }
    }

    config_options
}

fn update_mode_config_options(
    config_options: &mut [ConfigOption],
    current_mode_id: &str,
    current_model: &str,
    current_reasoning_effort: Option<ReasoningEffort>,
) {
    for option in config_options {
        match option.id.as_str() {
            "mode" => option.current_value = current_mode_id.to_string(),
            "model" => option.current_value = current_model.to_string(),
            "thought_level" => {
                if let Some(reasoning_effort) = current_reasoning_effort {
                    option.current_value = reasoning_effort_id(reasoning_effort);
                }
            }
            _ => {}
        }
    }
}

fn mode_id(name: &str) -> String {
    let mut id = String::new();
    let mut last_was_separator = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            id.push(ch.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator && !id.is_empty() {
            id.push('_');
            last_was_separator = true;
        }
    }
    id.trim_end_matches('_').to_string()
}

fn reasoning_effort_id(reasoning_effort: ReasoningEffort) -> String {
    match reasoning_effort {
        ReasoningEffort::None => "none".to_string(),
        ReasoningEffort::Minimal => "minimal".to_string(),
        ReasoningEffort::Low => "low".to_string(),
        ReasoningEffort::Medium => "medium".to_string(),
        ReasoningEffort::High => "high".to_string(),
        ReasoningEffort::XHigh => "xhigh".to_string(),
    }
}

fn reasoning_effort_name(reasoning_effort: ReasoningEffort) -> String {
    match reasoning_effort {
        ReasoningEffort::None => "None".to_string(),
        ReasoningEffort::Minimal => "Minimal".to_string(),
        ReasoningEffort::Low => "Low".to_string(),
        ReasoningEffort::Medium => "Medium".to_string(),
        ReasoningEffort::High => "High".to_string(),
        ReasoningEffort::XHigh => "XHigh".to_string(),
    }
}

fn flatten_user_message_text(content: &[UserInput]) -> Vec<String> {
    content
        .iter()
        .filter_map(|item| match item {
            UserInput::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn tool_call_text_output(text: Option<String>) -> Option<Vec<ToolCallContent>> {
    text.filter(|text| !text.is_empty()).map(|text| {
        vec![ToolCallContent::Content {
            content: text_content(text),
        }]
    })
}

fn text_content(text: String) -> TextContent {
    TextContent {
        content_type: "text".to_string(),
        text,
    }
}

fn map_command_status(status: codex_app_server_protocol::CommandExecutionStatus) -> String {
    match status {
        codex_app_server_protocol::CommandExecutionStatus::Completed => "completed".to_string(),
        codex_app_server_protocol::CommandExecutionStatus::Failed => "failed".to_string(),
        codex_app_server_protocol::CommandExecutionStatus::Declined => "cancelled".to_string(),
        codex_app_server_protocol::CommandExecutionStatus::InProgress => "in_progress".to_string(),
    }
}

fn map_patch_status(status: codex_app_server_protocol::PatchApplyStatus) -> String {
    match status {
        codex_app_server_protocol::PatchApplyStatus::Completed => "completed".to_string(),
        codex_app_server_protocol::PatchApplyStatus::Failed => "failed".to_string(),
        codex_app_server_protocol::PatchApplyStatus::Declined => "cancelled".to_string(),
        codex_app_server_protocol::PatchApplyStatus::InProgress => "in_progress".to_string(),
    }
}

fn invalid_params_error(error: impl std::fmt::Display) -> Error {
    Error::new(ErrorKind::InvalidInput, error.to_string())
}

fn io_error_from_typed_request(error: impl std::fmt::Display) -> Error {
    Error::other(error.to_string())
}

fn io_error_from_jsonrpc_error(error: &JsonRpcErrorBody) -> Error {
    Error::other(format!("ACP error {}: {}", error.code, error.message))
}

fn acp_request_id_to_rmcp(id: RequestId) -> McpRequestId {
    match id {
        RequestId::Integer(value) => McpRequestId::Number(value),
        RequestId::String(value) => McpRequestId::String(value.into()),
    }
}

fn rmcp_request_id_to_acp(id: McpRequestId) -> RequestId {
    match id {
        McpRequestId::Number(value) => RequestId::Integer(value),
        McpRequestId::String(value) => RequestId::String(value.to_string()),
    }
}

fn acp_error_to_mcp_error(error: JsonRpcErrorBody) -> McpErrorData {
    McpErrorData::new(McpErrorCode(error.code as i32), error.message, error.data)
}

fn mcp_request_from_acp(
    id: RequestId,
    params: McpMessageParams,
) -> Result<ServerJsonRpcMessage, Error> {
    let mut value = serde_json::Map::new();
    value.insert(
        "jsonrpc".to_string(),
        JsonValue::String(JSONRPC_VERSION.to_string()),
    );
    value.insert(
        "id".to_string(),
        serde_json::to_value(acp_request_id_to_rmcp(id)).map_err(invalid_params_error)?,
    );
    value.insert("method".to_string(), JsonValue::String(params.method));
    if let Some(request_params) = params.params {
        value.insert("params".to_string(), request_params);
    }
    if let Some(meta) = params.meta {
        value.insert("_meta".to_string(), meta);
    }
    serde_json::from_value(JsonValue::Object(value)).map_err(invalid_params_error)
}

fn mcp_notification_from_acp(params: McpMessageParams) -> Result<ServerJsonRpcMessage, Error> {
    let mut value = serde_json::Map::new();
    value.insert(
        "jsonrpc".to_string(),
        JsonValue::String(JSONRPC_VERSION.to_string()),
    );
    value.insert("method".to_string(), JsonValue::String(params.method));
    if let Some(request_params) = params.params {
        value.insert("params".to_string(), request_params);
    }
    if let Some(meta) = params.meta {
        value.insert("_meta".to_string(), meta);
    }
    serde_json::from_value(JsonValue::Object(value)).map_err(invalid_params_error)
}

fn mcp_message_params_from_value(
    connection_id: &str,
    value: JsonValue,
) -> Result<McpMessageParams, Error> {
    let mut object = value
        .as_object()
        .cloned()
        .ok_or_else(|| invalid_params_error("MCP message must serialize to an object"))?;
    let method = object
        .remove("method")
        .and_then(|value| value.as_str().map(str::to_string))
        .ok_or_else(|| invalid_params_error("MCP message is missing method"))?;
    Ok(McpMessageParams {
        connection_id: connection_id.to_string(),
        method,
        params: object.remove("params"),
        meta: object.remove("_meta").or_else(|| object.remove("meta")),
    })
}

fn mcp_result_to_server_message(
    request_id: McpRequestId,
    result: McpMessageResult,
) -> ServerJsonRpcMessage {
    let McpMessageResult {
        result: result_value,
        error,
        meta,
    } = result;
    if let Some(error) = error {
        let error = serde_json::from_value::<McpErrorData>(with_meta(error, meta)).unwrap_or_else(
            |error| {
                McpErrorData::new(
                    McpErrorCode::INTERNAL_ERROR,
                    format!("invalid ACP mcp/message error payload: {error}"),
                    None,
                )
            },
        );
        return ServerJsonRpcMessage::Error(McpJsonRpcError {
            jsonrpc: rmcp::model::JsonRpcVersion2_0,
            id: request_id,
            error,
        });
    }

    let result_value = with_meta(
        result_value.unwrap_or(JsonValue::Object(serde_json::Map::new())),
        meta,
    );
    match serde_json::from_value(result_value) {
        Ok(result) => ServerJsonRpcMessage::Response(McpJsonRpcResponse {
            jsonrpc: rmcp::model::JsonRpcVersion2_0,
            id: request_id,
            result,
        }),
        Err(error) => ServerJsonRpcMessage::Error(McpJsonRpcError {
            jsonrpc: rmcp::model::JsonRpcVersion2_0,
            id: request_id,
            error: McpErrorData::new(
                McpErrorCode::INTERNAL_ERROR,
                format!("invalid ACP mcp/message result payload: {error}"),
                None,
            ),
        }),
    }
}

fn mcp_result_from_client_response(
    response: McpJsonRpcResponse<McpClientResult>,
) -> Result<McpMessageResult, Error> {
    let mut result = serde_json::to_value(response.result).map_err(invalid_params_error)?;
    let meta = take_meta(&mut result);
    Ok(McpMessageResult {
        result: Some(result),
        error: None,
        meta,
    })
}

fn mcp_result_from_client_error(error: McpJsonRpcError) -> Result<McpMessageResult, Error> {
    let mut error_value = serde_json::to_value(error.error).map_err(invalid_params_error)?;
    let meta = take_meta(&mut error_value);
    Ok(McpMessageResult {
        result: None,
        error: Some(error_value),
        meta,
    })
}

fn forward_client_mcp_message(
    bridge_state: &AcpBridgeState,
    connection_id: &str,
    message: ClientJsonRpcMessage,
) -> IoResult<()> {
    match message {
        McpJsonRpcMessage::Request(McpJsonRpcRequest { id, request, .. }) => {
            let inbound_tx = bridge_state
                .connection_sender(connection_id)
                .ok_or_else(|| Error::new(ErrorKind::NotFound, "ACP MCP connection not found"))?;
            let outer_request_id = bridge_state.next_request_id("mcp-message");
            let params = mcp_message_params_from_value(
                connection_id,
                serde_json::to_value(request).map_err(invalid_params_error)?,
            )?;
            bridge_state
                .pending_requests
                .lock()
                .unwrap_or_else(|_| panic!("ACP MCP request registry poisoned"))
                .insert(
                    outer_request_id.clone(),
                    PendingMcpRequest {
                        inbound_tx,
                        request_id: id,
                    },
                );
            bridge_state.send_request(
                outer_request_id,
                "mcp/message",
                serde_json::to_value(params).map_err(invalid_params_error)?,
            )
        }
        McpJsonRpcMessage::Notification(McpJsonRpcNotification { notification, .. }) => {
            let params = mcp_message_params_from_value(
                connection_id,
                serde_json::to_value(notification).map_err(invalid_params_error)?,
            )?;
            bridge_state.send_notification(
                "mcp/message",
                serde_json::to_value(params).map_err(invalid_params_error)?,
            )
        }
        McpJsonRpcMessage::Response(response) => {
            let request_id = rmcp_request_id_to_acp(response.id.clone());
            bridge_state.send_result(
                request_id,
                serde_json::to_value(mcp_result_from_client_response(response)?)
                    .map_err(invalid_params_error)?,
            )
        }
        McpJsonRpcMessage::Error(error) => {
            let request_id = rmcp_request_id_to_acp(error.id.clone());
            bridge_state.send_result(
                request_id,
                serde_json::to_value(mcp_result_from_client_error(error)?)
                    .map_err(invalid_params_error)?,
            )
        }
    }
}

async fn close_mcp_connection(bridge_state: &AcpBridgeState, connection_id: &str) -> IoResult<()> {
    let request_id = bridge_state.next_request_id("mcp-disconnect");
    let (responder, receiver) = tokio::sync::oneshot::channel();
    bridge_state
        .pending_disconnects
        .lock()
        .unwrap_or_else(|_| panic!("ACP MCP disconnect registry poisoned"))
        .insert(request_id.clone(), PendingMcpDisconnect { responder });
    bridge_state.send_request(
        request_id,
        "mcp/disconnect",
        serde_json::to_value(McpDisconnectParams {
            connection_id: connection_id.to_string(),
            meta: None,
        })
        .map_err(invalid_params_error)?,
    )?;
    let result = receiver.await.map_err(|_| {
        Error::new(
            ErrorKind::BrokenPipe,
            "ACP MCP disconnect response channel closed",
        )
    })?;
    bridge_state.unregister_connection(connection_id);
    result
}

fn take_meta(value: &mut JsonValue) -> Option<JsonValue> {
    value
        .as_object_mut()
        .and_then(|object| object.remove("_meta").or_else(|| object.remove("meta")))
}

fn with_meta(value: JsonValue, meta: Option<JsonValue>) -> JsonValue {
    let Some(meta) = meta else {
        return value;
    };
    match value {
        JsonValue::Object(mut object) => {
            object.insert("_meta".to_string(), meta);
            JsonValue::Object(object)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn prompt_to_user_input_supports_text_resources() {
        let input = prompt_to_user_input(&[PromptContent::Resource {
            resource: crate::protocol::ResourceContent {
                uri: "file:///tmp/test.txt".to_string(),
                mime_type: None,
                text: Some("hello".to_string()),
            },
        }])
        .expect("text resources should be supported");
        assert_eq!(
            input,
            vec![UserInput::Text {
                text: "<resource uri=\"file:///tmp/test.txt\" mimeType=\"text/plain\">\nhello\n</resource>"
                    .to_string(),
                text_elements: Vec::new(),
            }]
        );
    }

    #[test]
    fn command_permission_options_skip_advanced_variants() {
        let (options, _, _) = build_command_permission_options(&Some(vec![
            CommandExecutionApprovalDecision::Accept,
            CommandExecutionApprovalDecision::AcceptForSession,
            CommandExecutionApprovalDecision::Decline,
            CommandExecutionApprovalDecision::Cancel,
        ]));
        assert_eq!(options.len(), 4);
        assert_eq!(options[0].option_id, "accept");
        assert_eq!(options[1].option_id, "accept-session");
    }

    #[test]
    fn request_overrides_translate_stdio_mcp_servers() {
        let overrides = build_request_overrides_from_mcp_servers(&[serde_json::json!({
            "name": "docs",
            "transport": "stdio",
            "command": "mcp-docs",
            "args": ["--stdio"],
            "env": [{"name": "ROOT", "value": "/tmp/project"}],
        })])
        .expect("stdio MCP server should parse")
        .expect("mcp overrides should be present");

        let mcp_servers: HashMap<String, McpServerConfig> =
            serde_json::from_value(overrides["mcp_servers"].clone())
                .expect("mcp_servers should deserialize");
        let docs = mcp_servers.get("docs").expect("docs config missing");
        match &docs.transport {
            McpServerTransportConfig::Stdio {
                command, args, env, ..
            } => {
                assert_eq!(command, "mcp-docs");
                assert_eq!(args, &vec!["--stdio".to_string()]);
                assert_eq!(
                    env.as_ref().and_then(|env| env.get("ROOT")),
                    Some(&"/tmp/project".to_string())
                );
            }
            other => panic!("expected stdio transport, got {other:?}"),
        }
    }

    #[test]
    fn request_overrides_translate_acp_transport_mcp_servers() {
        let overrides = build_request_overrides_from_mcp_servers(&[serde_json::json!({
            "name": "docs",
            "transport": "acp",
            "id": "srv_123",
        })])
        .expect("ACP transport should parse")
        .expect("mcp overrides should be present");

        let mcp_servers: HashMap<String, McpServerConfig> =
            serde_json::from_value(overrides["mcp_servers"].clone())
                .expect("mcp_servers should deserialize");
        let docs = mcp_servers.get("docs").expect("docs config missing");
        assert_eq!(
            docs.transport,
            McpServerTransportConfig::Acp {
                transport: "acp".to_string(),
                id: "srv_123".to_string(),
            }
        );
    }

    #[test]
    fn update_mode_config_options_updates_mode_and_model_values() {
        let mut config_options = vec![
            ConfigOption {
                id: "mode".to_string(),
                name: "Session Mode".to_string(),
                description: None,
                category: Some("mode".to_string()),
                option_type: "select".to_string(),
                current_value: "default".to_string(),
                options: Vec::new(),
            },
            ConfigOption {
                id: "model".to_string(),
                name: "Model".to_string(),
                description: None,
                category: Some("model".to_string()),
                option_type: "select".to_string(),
                current_value: "gpt-5".to_string(),
                options: Vec::new(),
            },
            ConfigOption {
                id: "thought_level".to_string(),
                name: "Thought Level".to_string(),
                description: None,
                category: Some("thought_level".to_string()),
                option_type: "select".to_string(),
                current_value: "low".to_string(),
                options: Vec::new(),
            },
        ];

        update_mode_config_options(
            &mut config_options,
            "plan",
            "gpt-5.2-codex",
            Some(ReasoningEffort::High),
        );

        assert_eq!(config_options[0].current_value, "plan");
        assert_eq!(config_options[1].current_value, "gpt-5.2-codex");
        assert_eq!(config_options[2].current_value, "high");
    }
}
