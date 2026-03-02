use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use ts_rs::TS;

fn default_model_visible() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DynamicToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: JsonValue,
    #[serde(default = "default_model_visible")]
    pub model_visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DynamicToolCallRequest {
    pub call_id: String,
    pub turn_id: String,
    pub tool: String,
    pub arguments: JsonValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DynamicToolResponse {
    pub content_items: Vec<DynamicToolCallOutputContentItem>,
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(tag = "type")]
pub enum DynamicToolCallOutputContentItem {
    #[serde(rename_all = "camelCase")]
    InputText { text: String },
    #[serde(rename_all = "camelCase")]
    InputImage { image_url: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn dynamic_tool_spec_defaults_model_visible_when_omitted() {
        let spec: DynamicToolSpec = serde_json::from_value(json!({
            "name": "demo_tool",
            "description": "Demo dynamic tool",
            "inputSchema": { "type": "object" }
        }))
        .expect("dynamic tool spec should deserialize");

        assert_eq!(
            spec,
            DynamicToolSpec {
                name: "demo_tool".to_string(),
                description: "Demo dynamic tool".to_string(),
                input_schema: json!({ "type": "object" }),
                model_visible: true,
            }
        );
    }
}
