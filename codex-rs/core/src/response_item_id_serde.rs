use codex_api::ResponseCreateWsRequest;
use codex_api::ResponsesApiRequest;
use codex_protocol::models::ResponseItem;
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

pub(crate) fn serialize_response_create_ws_request_body(
    request: &ResponseCreateWsRequest,
    response_item_ids: ResponseItemIdSerialization,
) -> serde_json::Result<Value> {
    serialize_input_payload(request, &request.input, response_item_ids)
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
        attach_input_item_ids(&mut value, input);
    }
    Ok(value)
}

fn attach_rollout_item_id(value: &mut Value, item: &RolloutItem) {
    let RolloutItem::ResponseItem(response_item) = item else {
        return;
    };
    let Some(id) = response_item_id(response_item) else {
        return;
    };

    if let Some(payload) = value.get_mut("payload").and_then(Value::as_object_mut) {
        payload.insert("id".to_string(), Value::String(id.to_string()));
    }
}

pub(crate) fn attach_input_item_ids(payload_json: &mut Value, original_items: &[ResponseItem]) {
    let Some(input_value) = payload_json.get_mut("input") else {
        return;
    };
    let Value::Array(items) = input_value else {
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
