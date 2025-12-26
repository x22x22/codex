//! Bridge functionality to convert between Responses API and Chat Completions API.
//!
//! This module provides transformation logic to:
//! 1. Convert Responses API requests to Chat Completions format
//! 2. Convert Chat Completions SSE responses back to Responses API format

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

/// Transform a Responses API request body to a Chat Completions request body.
pub fn transform_request_to_chat(responses_body: Value) -> Result<Value> {
    let model = responses_body
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing 'model' field"))?;

    let instructions = responses_body
        .get("instructions")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let input = responses_body
        .get("input")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("missing or invalid 'input' field"))?;

    let tools = responses_body
        .get("tools")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Build messages array from instructions and input
    let mut messages = Vec::new();

    // Add system message with instructions if present
    if !instructions.is_empty() {
        messages.push(json!({
            "role": "system",
            "content": instructions
        }));
    }

    // Convert input items to chat messages
    for item in input {
        if let Some(role) = item.get("role").and_then(|v| v.as_str()) {
            let content = extract_content_from_item(item);
            if !content.is_empty() || role == "assistant" {
                messages.push(json!({
                    "role": role,
                    "content": content
                }));
            }
        } else if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
            // Handle function calls
            if let (Some(name), Some(arguments), Some(call_id)) = (
                item.get("name").and_then(|v| v.as_str()),
                item.get("arguments").and_then(|v| v.as_str()),
                item.get("call_id").and_then(|v| v.as_str()),
            ) {
                messages.push(json!({
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": call_id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": arguments
                        }
                    }]
                }));
            }
        } else if item.get("type").and_then(|v| v.as_str()) == Some("function_call_output") {
            // Handle function call outputs
            if let (Some(call_id), Some(output)) = (
                item.get("call_id").and_then(|v| v.as_str()),
                item.get("output"),
            ) {
                let content = if let Some(obj) = output.as_object() {
                    obj.get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                } else {
                    output.to_string()
                };

                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": content
                }));
            }
        }
    }

    // Build the chat completions request
    let mut chat_request = json!({
        "model": model,
        "messages": messages,
        "stream": true
    });

    // Add tools if present
    if !tools.is_empty() {
        chat_request["tools"] = json!(tools);
    }

    Ok(chat_request)
}

/// Extract text content from a Responses API item.
fn extract_content_from_item(item: &Value) -> String {
    if let Some(content_array) = item.get("content").and_then(|v| v.as_array()) {
        let mut text = String::new();
        for content_item in content_array {
            if let Some(item_text) = content_item.get("text").and_then(|v| v.as_str()) {
                text.push_str(item_text);
            }
        }
        text
    } else if let Some(content_str) = item.get("content").and_then(|v| v.as_str()) {
        content_str.to_string()
    } else {
        String::new()
    }
}

/// Transform a line of Chat Completions SSE to Responses API SSE format.
/// Returns None if the line should be skipped.
pub fn transform_chat_sse_to_responses(line: &str) -> Option<String> {
    if line.trim().is_empty() {
        return Some(String::new());
    }

    if !line.starts_with("data: ") {
        return Some(line.to_string());
    }

    let data_content = &line[6..];
    if data_content.trim() == "[DONE]" {
        return Some("data: {\"type\":\"response.done\"}\n\n".to_string());
    }

    // Parse the JSON data
    let chat_event: Value = match serde_json::from_str(data_content) {
        Ok(v) => v,
        Err(_) => return Some(line.to_string()),
    };

    // Extract the response ID
    let response_id = chat_event
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("resp-1");

    // Process choices
    let choices = chat_event.get("choices").and_then(|v| v.as_array())?;

    let mut responses_events = Vec::new();

    for choice in choices {
        let delta = match choice.get("delta") {
            Some(d) => d,
            None => continue,
        };

        // Handle reasoning content
        if let Some(reasoning) = delta.get("reasoning") {
            let reasoning_text = if let Some(text) = reasoning.as_str() {
                text.to_string()
            } else if let Some(text) = reasoning.get("content").and_then(|v| v.as_str()) {
                text.to_string()
            } else {
                continue;
            };

            responses_events.push(json!({
                "type": "response.output_item.delta",
                "response": {
                    "id": response_id
                },
                "item": {
                    "type": "reasoning",
                    "content": [{
                        "type": "reasoning_text",
                        "text": reasoning_text
                    }]
                },
                "delta": reasoning_text
            }));
        }

        // Handle content (assistant message)
        if let Some(content) = delta.get("content") {
            let content_text = content.as_str().unwrap_or("");
            if !content_text.is_empty() {
                responses_events.push(json!({
                    "type": "response.output_item.delta",
                    "response": {
                        "id": response_id
                    },
                    "item": {
                        "type": "message",
                        "role": "assistant",
                        "content": [{
                            "type": "output_text",
                            "text": content_text
                        }]
                    },
                    "delta": content_text
                }));
            }
        }

        // Handle tool calls
        if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
            for tool_call in tool_calls {
                if let Some(function) = tool_call.get("function") {
                    let name = function.get("name").and_then(|v| v.as_str());
                    let arguments = function.get("arguments").and_then(|v| v.as_str());

                    if let (Some(name_val), Some(args_val)) = (name, arguments) {
                        let call_id = tool_call
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("call-1");

                        responses_events.push(json!({
                            "type": "response.output_item.done",
                            "response": {
                                "id": response_id
                            },
                            "item": {
                                "type": "function_call",
                                "name": name_val,
                                "arguments": args_val,
                                "call_id": call_id
                            }
                        }));
                    }
                }
            }
        }

        // Handle finish_reason
        if let Some(finish_reason) = choice.get("finish_reason").and_then(|v| v.as_str())
            && (finish_reason == "stop" || finish_reason == "tool_calls")
        {
            responses_events.push(json!({
                "type": "response.completed",
                "response": {
                    "id": response_id
                }
            }));
        }
    }

    // Convert events to SSE format
    if responses_events.is_empty() {
        None
    } else {
        let mut result = String::new();
        for event in responses_events {
            result.push_str("data: ");
            result.push_str(&event.to_string());
            result.push('\n');
            result.push('\n');
        }
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_transform_simple_request() {
        let responses_req = json!({
            "model": "gpt-4",
            "instructions": "You are a helpful assistant.",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Hello!"}]
                }
            ],
            "tools": [],
            "stream": true
        });

        let chat_req = transform_request_to_chat(responses_req).unwrap();

        assert_eq!(
            chat_req.get("model").and_then(|v| v.as_str()),
            Some("gpt-4")
        );
        assert_eq!(chat_req.get("stream"), Some(&json!(true)));

        let messages = chat_req.get("messages").and_then(|v| v.as_array()).unwrap();
        assert_eq!(messages.len(), 2); // system + user
        assert_eq!(
            messages[0].get("role").and_then(|v| v.as_str()),
            Some("system")
        );
        assert_eq!(
            messages[1].get("role").and_then(|v| v.as_str()),
            Some("user")
        );
    }

    #[test]
    fn test_transform_chat_sse_content() {
        let chat_sse =
            r#"data: {"id":"chatcmpl-123","choices":[{"delta":{"content":"Hello"},"index":0}]}"#;

        let result = transform_chat_sse_to_responses(chat_sse).unwrap();

        assert!(result.contains("response.output_item.delta"));
        assert!(result.contains("Hello"));
        assert!(result.contains("output_text"));
    }

    #[test]
    fn test_transform_chat_sse_done() {
        let chat_sse = "data: [DONE]";

        let result = transform_chat_sse_to_responses(chat_sse).unwrap();

        assert!(result.contains("response.done"));
    }
}
