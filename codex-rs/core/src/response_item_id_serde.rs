use codex_api::ResponsesApiRequest;
use codex_api::common::ResponsesWsRequest;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::CompactedItem;
use codex_protocol::protocol::RolloutItem;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ResponseItemIdSerialization {
    #[default]
    Disabled,
    Enabled,
}

impl ResponseItemIdSerialization {
    pub(crate) fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

pub(crate) fn serialize_responses_request_body(
    request: &ResponsesApiRequest,
    response_item_ids: ResponseItemIdSerialization,
) -> serde_json::Result<Value> {
    serialize_input_payload(request, &request.input, response_item_ids)
}

pub(crate) fn serialize_responses_ws_request_body(
    request: &ResponsesWsRequest,
    response_item_ids: ResponseItemIdSerialization,
) -> serde_json::Result<Value> {
    let mut value = serde_json::to_value(request)?;
    if response_item_ids.is_enabled() {
        match request {
            ResponsesWsRequest::ResponseCreate(request) => {
                attach_response_item_ids(value.get_mut("input"), &request.input);
            }
        }
    }
    Ok(value)
}

pub(crate) fn serialize_rollout_line(
    timestamp: String,
    item: &RolloutItem,
    response_item_ids: ResponseItemIdSerialization,
) -> serde_json::Result<String> {
    let mut value = serde_json::to_value(RolloutLineRef { timestamp, item })?;
    if response_item_ids.is_enabled() {
        attach_rollout_item_id(&mut value, item);
    }
    serde_json::to_string(&value)
}

fn serialize_input_payload<T: Serialize>(
    request: &T,
    input: &[ResponseItem],
    response_item_ids: ResponseItemIdSerialization,
) -> serde_json::Result<Value> {
    let mut value = serde_json::to_value(request)?;
    if response_item_ids.is_enabled() {
        attach_response_item_ids(value.get_mut("input"), input);
    }
    Ok(value)
}

fn attach_rollout_item_id(value: &mut Value, item: &RolloutItem) {
    match item {
        RolloutItem::ResponseItem(response_item) => {
            let Some(id) = response_item_id(response_item) else {
                return;
            };

            if let Some(payload) = value.get_mut("payload").and_then(Value::as_object_mut) {
                payload.insert("id".to_string(), Value::String(id.to_string()));
            }
        }
        RolloutItem::Compacted(compacted_item) => {
            attach_compacted_replacement_history_ids(value, compacted_item);
        }
        RolloutItem::SessionMeta(_) | RolloutItem::TurnContext(_) | RolloutItem::EventMsg(_) => {}
    }
}

fn attach_compacted_replacement_history_ids(value: &mut Value, compacted_item: &CompactedItem) {
    let Some(payload) = value.get_mut("payload").and_then(Value::as_object_mut) else {
        return;
    };

    attach_response_item_ids(
        payload.get_mut("replacement_history"),
        compacted_item
            .replacement_history
            .as_deref()
            .unwrap_or_default(),
    );
}

fn attach_response_item_ids(items_value: Option<&mut Value>, original_items: &[ResponseItem]) {
    let Some(items_value) = items_value else {
        return;
    };
    let Value::Array(items) = items_value else {
        return;
    };

    for (value, item) in items.iter_mut().zip(original_items.iter()) {
        let Some(id) = response_item_id(item) else {
            continue;
        };

        if id.is_empty() {
            continue;
        }

        if let Some(obj) = value.as_object_mut() {
            obj.insert("id".to_string(), Value::String(id.to_string()));
        }
    }
}

fn response_item_id(item: &ResponseItem) -> Option<&str> {
    match item {
        ResponseItem::Reasoning { id, .. } => Some(id.as_str()),
        ResponseItem::Message { id: Some(id), .. }
        | ResponseItem::WebSearchCall { id: Some(id), .. }
        | ResponseItem::FunctionCall { id: Some(id), .. }
        | ResponseItem::ToolSearchCall { id: Some(id), .. }
        | ResponseItem::LocalShellCall { id: Some(id), .. }
        | ResponseItem::CustomToolCall { id: Some(id), .. } => Some(id.as_str()),
        ResponseItem::Message { id: None, .. }
        | ResponseItem::WebSearchCall { id: None, .. }
        | ResponseItem::LocalShellCall { id: None, .. }
        | ResponseItem::FunctionCall { id: None, .. }
        | ResponseItem::ToolSearchCall { id: None, .. }
        | ResponseItem::CustomToolCall { id: None, .. }
        | ResponseItem::FunctionCallOutput { .. }
        | ResponseItem::CustomToolCallOutput { .. }
        | ResponseItem::ToolSearchOutput { .. }
        | ResponseItem::ImageGenerationCall { .. }
        | ResponseItem::GhostSnapshot { .. }
        | ResponseItem::Compaction { .. }
        | ResponseItem::Other => None,
    }
}

#[derive(Serialize)]
struct RolloutLineRef<'a> {
    timestamp: String,
    #[serde(flatten)]
    item: &'a RolloutItem,
}

#[cfg(test)]
mod tests {
    use super::ResponseItemIdSerialization;
    use super::serialize_responses_ws_request_body;
    use super::serialize_rollout_line;
    use codex_api::ResponseCreateWsRequest;
    use codex_api::common::ResponsesWsRequest;
    use codex_protocol::models::ContentItem;
    use codex_protocol::models::ResponseItem;
    use codex_protocol::protocol::CompactedItem;
    use codex_protocol::protocol::RolloutItem;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn serialize_responses_ws_request_body_preserves_response_create_wrapper() {
        let request = ResponsesWsRequest::ResponseCreate(ResponseCreateWsRequest {
            model: "gpt-test".to_string(),
            instructions: "Be helpful".to_string(),
            previous_response_id: None,
            input: vec![assistant_message(Some("msg_123"))],
            tools: Vec::new(),
            tool_choice: "auto".to_string(),
            parallel_tool_calls: false,
            reasoning: None,
            store: true,
            stream: true,
            include: Vec::new(),
            service_tier: None,
            prompt_cache_key: None,
            text: None,
            generate: None,
            client_metadata: None,
        });

        let value =
            serialize_responses_ws_request_body(&request, ResponseItemIdSerialization::Enabled)
                .expect("websocket request should serialize");

        assert_eq!(value["type"], json!("response.create"));
        assert_eq!(value["input"][0]["id"], json!("msg_123"));
    }

    #[test]
    fn serialize_rollout_line_preserves_ids_in_compacted_replacement_history() {
        let line = serialize_rollout_line(
            "2026-03-18T00:00:00Z".to_string(),
            &RolloutItem::Compacted(CompactedItem {
                message: "compacted".to_string(),
                replacement_history: Some(vec![assistant_message(Some("msg_123"))]),
            }),
            ResponseItemIdSerialization::Enabled,
        )
        .expect("rollout line should serialize");

        let value: serde_json::Value =
            serde_json::from_str(&line).expect("serialized rollout line should be valid json");
        assert_eq!(
            value["payload"]["replacement_history"][0]["id"],
            json!("msg_123")
        );
    }

    fn assistant_message(id: Option<&str>) -> ResponseItem {
        ResponseItem::Message {
            id: id.map(str::to_string),
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: "hello".to_string(),
            }],
            end_turn: None,
            phase: None,
        }
    }
}
