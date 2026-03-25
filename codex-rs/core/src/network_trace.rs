use codex_protocol::ThreadId;
use serde::Deserialize;
use tracing::Span;
use tracing::field::Empty;
use url::Url;

use crate::codex::Session;
use crate::codex::TurnContext;

#[derive(Debug, Default, Deserialize)]
struct TurnTraceCorrelation {
    session_id: Option<String>,
    turn_id: Option<String>,
}

struct CorrelationFields {
    conversation_id: String,
    session_id: String,
    turn_id: Option<String>,
}

impl CorrelationFields {
    fn from_turn_metadata_header(
        conversation_id: &ThreadId,
        turn_metadata_header: Option<&str>,
    ) -> Self {
        let conversation_id = conversation_id.to_string();
        let correlation = turn_metadata_header
            .and_then(|header| serde_json::from_str::<TurnTraceCorrelation>(header).ok())
            .unwrap_or_default();
        let session_id = correlation
            .session_id
            .unwrap_or_else(|| conversation_id.clone());
        Self {
            conversation_id,
            session_id,
            turn_id: correlation.turn_id,
        }
    }

    fn from_turn_context(session: &Session, turn_context: &TurnContext) -> Self {
        Self {
            conversation_id: session.conversation_id.to_string(),
            session_id: session.conversation_id.to_string(),
            turn_id: Some(turn_context.sub_id.clone()),
        }
    }

    fn record_on(&self, span: &Span) {
        span.record("conversation.id", self.conversation_id.as_str());
        span.record("session.id", self.session_id.as_str());
        if let Some(turn_id) = self.turn_id.as_deref() {
            span.record("turn.id", turn_id);
        }
    }
}

fn record_server_fields(span: &Span, url: Option<&str>) {
    let Some(url) = url else {
        return;
    };
    let Ok(parsed) = Url::parse(url) else {
        return;
    };
    if let Some(host) = parsed.host_str() {
        span.record("server.address", host);
    }
    if let Some(port) = parsed.port_or_known_default() {
        span.record("server.port", port as i64);
    }
}

pub(crate) fn responses_http_request_span(
    conversation_id: &ThreadId,
    turn_metadata_header: Option<&str>,
    provider_name: &str,
    model: &str,
    base_url: &str,
) -> Span {
    let span = tracing::info_span!(
        "responses_http.request",
        otel.kind = "client",
        provider = provider_name,
        model,
        transport = "responses_http",
        api.path = "responses",
        conversation.id = Empty,
        session.id = Empty,
        turn.id = Empty,
        server.address = Empty,
        server.port = Empty,
    );
    CorrelationFields::from_turn_metadata_header(conversation_id, turn_metadata_header)
        .record_on(&span);
    record_server_fields(&span, Some(base_url));
    span
}

pub(crate) struct McpToolCallTrace<'a> {
    pub server_name: &'a str,
    pub tool_name: &'a str,
    pub call_id: &'a str,
    pub server_origin: Option<&'a str>,
    pub connector_id: Option<&'a str>,
    pub connector_name: Option<&'a str>,
}

pub(crate) fn mcp_tool_call_span(
    session: &Session,
    turn_context: &TurnContext,
    trace: McpToolCallTrace<'_>,
) -> Span {
    let transport = match trace.server_origin {
        Some("stdio") => "stdio",
        Some(_) => "streamable_http",
        None => "",
    };
    let span = tracing::info_span!(
        "mcp.tools.call",
        otel.kind = "client",
        rpc.system = "jsonrpc",
        rpc.method = "tools/call",
        mcp.server.name = trace.server_name,
        mcp.server.origin = trace.server_origin.unwrap_or(""),
        mcp.transport = transport,
        mcp.connector.id = trace.connector_id.unwrap_or(""),
        mcp.connector.name = trace.connector_name.unwrap_or(""),
        tool.name = trace.tool_name,
        tool.call_id = trace.call_id,
        conversation.id = Empty,
        session.id = Empty,
        turn.id = Empty,
        server.address = Empty,
        server.port = Empty,
    );
    CorrelationFields::from_turn_context(session, turn_context).record_on(&span);
    record_server_fields(&span, trace.server_origin);
    span
}
