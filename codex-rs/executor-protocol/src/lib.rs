use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: JsonValue,
}

impl ExecutorToolSpec {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: JsonValue,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ListToolsRequest {
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CallToolRequest {
    pub request_id: String,
    pub tool_name: String,
    pub arguments: JsonValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShutdownRequest {
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OrchestratorToExecutorMessage {
    ListTools(ListToolsRequest),
    CallTool(CallToolRequest),
    Shutdown(ShutdownRequest),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ListToolsResponse {
    pub request_id: String,
    pub tools: Vec<ExecutorToolSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolCallContent {
    InputText { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolCallOutcome {
    Success { content: Vec<ToolCallContent> },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResponse {
    pub request_id: String,
    pub tool_name: String,
    pub outcome: ToolCallOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShutdownResponse {
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ErrorResponse {
    pub request_id: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutorToOrchestratorMessage {
    ListToolsResponse(ListToolsResponse),
    CallToolResponse(CallToolResponse),
    ShutdownResponse(ShutdownResponse),
    Error(ErrorResponse),
}

impl ExecutorToOrchestratorMessage {
    pub fn error(request_id: Option<String>, message: impl Into<String>) -> Self {
        Self::Error(ErrorResponse {
            request_id,
            message: message.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CallToolRequest;
    use super::ExecutorToOrchestratorMessage;
    use super::ExecutorToolSpec;
    use super::ListToolsResponse;
    use super::OrchestratorToExecutorMessage;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn list_tools_response_round_trips_with_camel_case_fields() {
        let message = ExecutorToOrchestratorMessage::ListToolsResponse(ListToolsResponse {
            request_id: "req-1".to_string(),
            tools: vec![ExecutorToolSpec::new(
                "exec_command",
                "Run a command",
                json!({
                    "type": "object",
                    "properties": {
                        "cmd": {"type": "string"},
                    },
                    "required": ["cmd"],
                }),
            )],
        });

        let value = serde_json::to_value(&message).expect("serialize");
        assert_eq!(
            value,
            json!({
                "type": "list_tools_response",
                "requestId": "req-1",
                "tools": [{
                    "name": "exec_command",
                    "description": "Run a command",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "cmd": {"type": "string"},
                        },
                        "required": ["cmd"],
                    },
                }],
            })
        );
    }

    #[test]
    fn call_tool_request_round_trips() {
        let value = json!({
            "type": "call_tool",
            "requestId": "req-2",
            "toolName": "exec_command",
            "arguments": {
                "cmd": "pwd",
            },
        });

        let actual: OrchestratorToExecutorMessage =
            serde_json::from_value(value.clone()).expect("deserialize");

        assert_eq!(
            actual,
            OrchestratorToExecutorMessage::CallTool(CallToolRequest {
                request_id: "req-2".to_string(),
                tool_name: "exec_command".to_string(),
                arguments: json!({
                    "cmd": "pwd",
                }),
            })
        );

        assert_eq!(serde_json::to_value(actual).expect("serialize"), value);
    }
}
