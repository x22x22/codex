use codex_protocol::mcp::CallToolResult;
use serde_json::Map;
use serde_json::Value;

const CHANNEL_ID_FIELD_NAME: &str = "channel_id";
const ID_FIELD_NAME: &str = "id";
const NAME_FIELD_NAME: &str = "name";
const NICKNAME_FIELD_NAME: &str = "nickname";
const RESULT_FIELD_NAME: &str = "result";
const TO_FIELD_NAME: &str = "to";

const SLACK_GET_PROFILE_TOOL_TITLE: &str = "get_profile";
const SLACK_READ_USER_PROFILE_TOOL_TITLE: &str = "slack_read_user_profile";
const SLACK_SEND_MESSAGE_TOOL_TITLE: &str = "slack_send_message";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlackChannelName {
    pub(crate) channel_id: String,
    pub(crate) channel_name: String,
}

pub(crate) fn slack_channel_name_from_profile_result(
    connector_name: Option<&str>,
    tool_title: Option<&str>,
    result: &CallToolResult,
) -> Option<SlackChannelName> {
    if !is_slack_profile_tool(connector_name, tool_title) {
        return None;
    }

    if let Some(channel_name) = result
        .structured_content
        .as_ref()
        .filter(|value| !value.is_null())
        .and_then(slack_channel_name_from_payload)
    {
        return Some(channel_name);
    }

    let parsed_text_payload = parse_payload_from_text_content(&result.content);
    if let Some(channel_name) = parsed_text_payload
        .as_ref()
        .and_then(slack_channel_name_from_payload)
    {
        return Some(channel_name);
    }

    raw_text_content(&result.content).and_then(parse_slack_channel_name_from_text)
}

pub(crate) fn slack_send_message_channel_id<'a>(
    connector_name: Option<&str>,
    tool_title: Option<&str>,
    tool_params: Option<&'a Value>,
) -> Option<&'a str> {
    if !is_slack_send_message_tool(connector_name, tool_title) {
        return None;
    }

    tool_params?
        .as_object()?
        .get(CHANNEL_ID_FIELD_NAME)
        .and_then(nonempty_string)
}

pub(crate) fn translated_slack_send_message_tool_params(
    connector_name: Option<&str>,
    tool_title: Option<&str>,
    tool_params: Option<&Value>,
    channel_name: Option<&str>,
) -> Option<Value> {
    let tool_params = tool_params?;
    if !is_slack_send_message_tool(connector_name, tool_title) {
        return Some(tool_params.clone());
    }

    let Some(channel_name) = channel_name.map(str::trim).filter(|name| !name.is_empty()) else {
        return Some(tool_params.clone());
    };
    let Value::Object(tool_params) = tool_params else {
        return Some(tool_params.clone());
    };

    let mut translated = tool_params.clone();
    translated.insert(
        TO_FIELD_NAME.to_string(),
        Value::String(channel_name.to_string()),
    );
    Some(Value::Object(translated))
}

fn is_slack_profile_tool(connector_name: Option<&str>, tool_title: Option<&str>) -> bool {
    is_slack_connector(connector_name)
        && matches!(
            tool_title.map(str::trim),
            Some(SLACK_GET_PROFILE_TOOL_TITLE) | Some(SLACK_READ_USER_PROFILE_TOOL_TITLE)
        )
}

fn is_slack_send_message_tool(connector_name: Option<&str>, tool_title: Option<&str>) -> bool {
    is_slack_connector(connector_name)
        && matches!(
            tool_title.map(str::trim),
            Some(SLACK_SEND_MESSAGE_TOOL_TITLE)
        )
}

fn is_slack_connector(connector_name: Option<&str>) -> bool {
    connector_name
        .map(str::trim)
        .is_some_and(|connector_name| connector_name.starts_with("Slack"))
}

fn parse_payload_from_text_content(content: &[Value]) -> Option<Value> {
    let text = raw_text_content(content)?;
    serde_json::from_str(text).ok()
}

fn raw_text_content(content: &[Value]) -> Option<&str> {
    let [content_block] = content else {
        return None;
    };
    let content_block = content_block.as_object()?;
    if content_block.get("type").and_then(Value::as_str) != Some("text") {
        return None;
    }

    content_block.get("text").and_then(nonempty_string)
}

fn slack_channel_name_from_payload(payload: &Value) -> Option<SlackChannelName> {
    let payload = payload.as_object()?;
    if let Some(result_text) = payload.get(RESULT_FIELD_NAME).and_then(nonempty_string) {
        return parse_slack_channel_name_from_text(result_text);
    }
    if let Some(result_object) = payload.get(RESULT_FIELD_NAME).and_then(Value::as_object) {
        return slack_channel_name_from_profile_object(result_object);
    }

    slack_channel_name_from_profile_object(payload)
}

fn slack_channel_name_from_profile_object(
    profile: &Map<String, Value>,
) -> Option<SlackChannelName> {
    let channel_id = profile
        .get(ID_FIELD_NAME)
        .and_then(nonempty_string)?
        .to_string();
    let channel_name = readable_slack_channel_name(profile)?;

    Some(SlackChannelName {
        channel_id,
        channel_name,
    })
}

fn readable_slack_channel_name(profile: &Map<String, Value>) -> Option<String> {
    if let Some(nickname) = profile.get(NICKNAME_FIELD_NAME).and_then(nonempty_string) {
        return Some(format_slack_handle(nickname));
    }

    profile
        .get(NAME_FIELD_NAME)
        .and_then(nonempty_string)
        .map(ToString::to_string)
}

fn format_slack_handle(handle: &str) -> String {
    if handle.starts_with('@') {
        handle.to_string()
    } else {
        format!("@{handle}")
    }
}

fn parse_slack_channel_name_from_text(text: &str) -> Option<SlackChannelName> {
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let channel_id = labeled_multiline_value(&lines, "User ID")?;
    let channel_name =
        handle_from_profile_lines(&lines).or_else(|| labeled_multiline_value(&lines, "Name"))?;

    Some(SlackChannelName {
        channel_id,
        channel_name,
    })
}

fn handle_from_profile_lines(lines: &[&str]) -> Option<String> {
    let header = *lines.first()?;
    let open_index = header.rfind('(')?;
    let close_index = header.rfind(')')?;
    if close_index <= open_index {
        return None;
    }

    let candidate = header[open_index + 1..close_index].trim();
    is_plausible_slack_handle(candidate).then(|| format_slack_handle(candidate))
}

fn is_plausible_slack_handle(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

fn labeled_multiline_value(lines: &[&str], label: &str) -> Option<String> {
    let prefix = format!("{label}:");
    for (index, line) in lines.iter().enumerate() {
        if let Some(value) = line.strip_prefix(&prefix) {
            let mut combined = value.trim().to_string();
            let mut next_index = index + 1;
            while next_index < lines.len() && !looks_like_field_label(lines[next_index]) {
                if !combined.is_empty() {
                    combined.push(' ');
                }
                combined.push_str(lines[next_index]);
                next_index += 1;
            }

            return Some(combined);
        }
    }

    None
}

fn looks_like_field_label(line: &str) -> bool {
    let Some((label, _)) = line.split_once(':') else {
        return false;
    };
    !label.trim().is_empty()
        && label
            .chars()
            .all(|ch| ch.is_ascii_alphabetic() || ch == ' ' || ch == '#')
}

fn nonempty_string(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn extracts_slack_channel_name_from_profile_result() {
        let result = CallToolResult {
            content: Vec::new(),
            structured_content: Some(json!({
                "result": {
                    "id": "U123",
                    "name": "Mason Zeng",
                    "nickname": "mzeng",
                }
            })),
            is_error: Some(false),
            meta: None,
        };

        assert_eq!(
            slack_channel_name_from_profile_result(
                Some("Slack Codex App"),
                Some("get_profile"),
                &result
            ),
            Some(SlackChannelName {
                channel_id: "U123".to_string(),
                channel_name: "@mzeng".to_string(),
            })
        );
    }

    #[test]
    fn falls_back_to_text_content_when_structured_content_is_missing() {
        let result = CallToolResult {
            content: vec![json!({
                "type": "text",
                "text": "{\"result\":\"mzeng (mzeng)\\nOpenAI\\nCodexing\\nUsers (1 results)\\n### Result 1 of 1\\nName: Matthew\\nZeng\\nUser ID: U07B9LBRPST\\nTitle: Codexing\"}",
            })],
            structured_content: None,
            is_error: Some(false),
            meta: None,
        };

        assert_eq!(
            slack_channel_name_from_profile_result(
                Some("Slack"),
                Some("slack_read_user_profile"),
                &result
            ),
            Some(SlackChannelName {
                channel_id: "U07B9LBRPST".to_string(),
                channel_name: "@mzeng".to_string(),
            })
        );
    }

    #[test]
    fn extracts_slack_channel_name_from_structured_text_result() {
        let result = CallToolResult {
            content: Vec::new(),
            structured_content: Some(json!({
                "result": "mzeng (mzeng)\nOpenAI\nCodexing\nUsers (1 results)\n### Result 1 of 1\nName: Matthew\nZeng\nUser ID: U07B9LBRPST\nTitle: Codexing",
            })),
            is_error: Some(false),
            meta: None,
        };

        assert_eq!(
            slack_channel_name_from_profile_result(
                Some("Slack"),
                Some("slack_read_user_profile"),
                &result
            ),
            Some(SlackChannelName {
                channel_id: "U07B9LBRPST".to_string(),
                channel_name: "@mzeng".to_string(),
            })
        );
    }

    #[test]
    fn ignores_non_slack_profile_tools() {
        let result = CallToolResult {
            content: Vec::new(),
            structured_content: Some(json!({
                "result": {
                    "id": "U123",
                    "name": "Mason Zeng",
                }
            })),
            is_error: Some(false),
            meta: None,
        };

        assert_eq!(
            slack_channel_name_from_profile_result(Some("Linear"), Some("get_profile"), &result),
            None
        );
    }

    #[test]
    fn translates_slack_send_message_tool_params() {
        assert_eq!(
            translated_slack_send_message_tool_params(
                Some("Slack"),
                Some("slack_send_message"),
                Some(&json!({
                    "channel_id": "U123",
                    "message": "hi",
                })),
                Some("@mzeng"),
            ),
            Some(json!({
                "channel_id": "U123",
                "message": "hi",
                "to": "@mzeng",
            }))
        );
    }

    #[test]
    fn leaves_non_slack_send_message_tool_params_unchanged() {
        assert_eq!(
            translated_slack_send_message_tool_params(
                Some("Slack"),
                Some("slack_search_channels"),
                Some(&json!({
                    "query": "eng",
                })),
                Some("#eng"),
            ),
            Some(json!({
                "query": "eng",
            }))
        );
    }
}
