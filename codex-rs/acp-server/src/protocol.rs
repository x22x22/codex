use codex_app_server_protocol::RequestId;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;

pub const JSONRPC_VERSION: &str = "2.0";
pub const PROTOCOL_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    Request(JsonRpcRequest),
    Notification(JsonRpcNotification),
    Response(JsonRpcResponse),
    Error(JsonRpcError),
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: RequestId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: RequestId,
    pub result: JsonValue,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct JsonRpcError {
    pub jsonrpc: String,
    pub id: RequestId,
    pub error: JsonRpcErrorBody,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct JsonRpcErrorBody {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_capabilities: Option<ClientCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_info: Option<ClientInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elicitation: Option<ElicitationCapabilities>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ElicitationCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: i64,
    pub agent_capabilities: AgentCapabilities,
    pub agent_info: AgentInfo,
    pub auth_methods: Vec<JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    pub load_session: bool,
    pub session_capabilities: SessionCapabilities,
    pub prompt_capabilities: PromptCapabilities,
    pub mcp_capabilities: McpCapabilities,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCapabilities {
    pub list: EmptyCapability,
    pub fork: EmptyCapability,
    pub resume: EmptyCapability,
    pub close: EmptyCapability,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct EmptyCapability {}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCapabilities {
    pub http: bool,
    pub sse: bool,
    pub acp: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptCapabilities {
    pub image: bool,
    pub audio: bool,
    pub embedded_context: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mode {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Modes {
    pub current_mode_id: String,
    pub available_modes: Vec<Mode>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub name: String,
    pub title: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNewParams {
    pub cwd: String,
    #[serde(default)]
    pub mcp_servers: Vec<JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNewResult {
    pub session_id: String,
    pub modes: Modes,
    pub config_options: Vec<ConfigOption>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLoadParams {
    pub session_id: String,
    pub cwd: String,
    #[serde(default)]
    pub mcp_servers: Vec<JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLoadResult {
    pub modes: Modes,
    pub config_options: Vec<ConfigOption>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResumeParams {
    pub session_id: String,
    pub cwd: String,
    #[serde(default)]
    pub mcp_servers: Vec<JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResumeResult {
    pub modes: Modes,
    pub config_options: Vec<ConfigOption>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionForkParams {
    pub session_id: String,
    pub cwd: String,
    #[serde(default)]
    pub mcp_servers: Vec<JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionForkResult {
    pub session_id: String,
    pub modes: Modes,
    pub config_options: Vec<ConfigOption>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListResult {
    pub sessions: Vec<SessionInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCloseParams {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SessionCloseResult {}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSetModeParams {
    pub session_id: String,
    pub mode_id: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SessionSetModeResult {}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPromptParams {
    pub session_id: String,
    pub prompt: Vec<PromptContent>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptContent {
    Text { text: String },
    Resource { resource: ResourceContent },
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceContent {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPromptResult {
    pub stop_reason: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCancelParams {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUpdateParams {
    pub session_id: String,
    pub update: SessionUpdate,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case")]
pub enum SessionUpdate {
    UserMessageChunk {
        content: TextContent,
    },
    AgentMessageChunk {
        content: TextContent,
    },
    Plan {
        entries: Vec<PlanEntry>,
    },
    CurrentModeUpdate {
        current_mode_id: String,
    },
    ConfigOptionsUpdate {
        config_options: Vec<ConfigOption>,
    },
    SessionInfoUpdate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        updated_at: Option<String>,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<JsonValue>,
    },
    ToolCall {
        tool_call_id: String,
        title: String,
        kind: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        locations: Option<Vec<Location>>,
    },
    ToolCallUpdate {
        tool_call_id: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Vec<ToolCallContent>>,
    },
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct TextContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanEntry {
    pub content: String,
    pub priority: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigOption {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(rename = "type")]
    pub option_type: String,
    pub current_value: String,
    pub options: Vec<ConfigOptionValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigOptionValue {
    pub value: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Location {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolCallContent {
    Content { content: TextContent },
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRequestPermissionParams {
    pub session_id: String,
    pub tool_call: PermissionToolCall,
    pub options: Vec<PermissionOption>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionElicitationParams {
    pub session_id: String,
    pub elicitation: SessionElicitationRequest,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum SessionElicitationRequest {
    #[serde(rename_all = "camelCase")]
    Form {
        message: String,
        requested_schema: JsonValue,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<JsonValue>,
    },
    #[serde(rename_all = "camelCase")]
    Url {
        message: String,
        url: String,
        elicitation_id: String,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<JsonValue>,
    },
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionElicitationResponse {
    pub action: ElicitationAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<JsonValue>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ElicitationAction {
    Accept,
    Decline,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConnectParams {
    pub acp_id: String,
    #[serde(default, alias = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConnectResult {
    pub connection_id: String,
    #[serde(default, alias = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDisconnectParams {
    pub connection_id: String,
    #[serde(default, alias = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDisconnectResult {
    #[serde(default, alias = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpMessageParams {
    pub connection_id: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<JsonValue>,
    #[serde(default, alias = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpMessageResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonValue>,
    #[serde(default, alias = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionToolCall {
    pub tool_call_id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct PermissionResponse {
    pub outcome: PermissionOutcome,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PermissionOutcome {
    Selected { option_id: String },
    Cancelled,
}
