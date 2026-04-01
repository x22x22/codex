use anyhow::Result;
use codex_core::ThreadConfigSnapshot;
use codex_core::config::AgentRoleConfig;
use codex_features::Feature;
use codex_protocol::ThreadId;
use codex_protocol::openai_models::ReasoningEffort;
use core_test_support::responses::ResponsesRequest;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_response_once_match;
use core_test_support::responses::mount_sse_once_match;
use core_test_support::responses::sse;
use core_test_support::responses::sse_response;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::time::Duration;
use tokio::time::Instant;
use tokio::time::sleep;
use wiremock::MockServer;

const SPAWN_CALL_ID: &str = "spawn-call-1";
const FORKED_SPAWN_AGENT_OUTPUT_MESSAGE: &str = "You are the newly spawned agent. The prior conversation history was forked from your parent agent. Treat the next user message as your new task, and use the forked history only as background context.";
const TURN_0_FORK_PROMPT: &str = "seed fork context";
const TURN_1_PROMPT: &str = "spawn a child and continue";
const TURN_2_NO_WAIT_PROMPT: &str = "follow up without wait";
const CHILD_PROMPT: &str = "child: do work";
const INHERITED_MODEL: &str = "gpt-5.2-codex";
const INHERITED_REASONING_EFFORT: ReasoningEffort = ReasoningEffort::XHigh;
const REQUESTED_MODEL: &str = "gpt-5.1";
const REQUESTED_REASONING_EFFORT: ReasoningEffort = ReasoningEffort::Low;
const ROLE_MODEL: &str = "gpt-5.1-codex-max";
const ROLE_REASONING_EFFORT: ReasoningEffort = ReasoningEffort::High;

fn body_contains(req: &wiremock::Request, text: &str) -> bool {
    let is_zstd = req
        .headers
        .get("content-encoding")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|entry| entry.trim().eq_ignore_ascii_case("zstd"))
        });
    let bytes = if is_zstd {
        zstd::stream::decode_all(std::io::Cursor::new(&req.body)).ok()
    } else {
        Some(req.body.clone())
    };
    bytes
        .and_then(|body| String::from_utf8(body).ok())
        .is_some_and(|body| body.contains(text))
}

fn has_subagent_notification(req: &ResponsesRequest) -> bool {
    req.message_input_texts("user")
        .iter()
        .any(|text| text.contains("<subagent_notification>"))
}

fn cache_prefix_request_body(request: &ResponsesRequest, call_id: &str) -> Result<Value> {
    let mut body = request.body_json();
    let Some(object) = body.as_object_mut() else {
        anyhow::bail!("expected JSON object request body");
    };

    let input = object
        .get_mut("input")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow::anyhow!("expected request input array"))?;
    let spawn_call_index = input
        .iter()
        .rposition(|item| {
            item.get("type").and_then(Value::as_str) == Some("function_call")
                && item.get("call_id").and_then(Value::as_str) == Some(call_id)
        })
        .ok_or_else(|| {
            anyhow::anyhow!("expected request input to include function_call {call_id}: {input:?}")
        })?;
    // The cache-preservation contract is only for the shared request prefix up to
    // and including the forked `spawn_agent` call. The `FunctionCallOutput` for that
    // call is the first legal divergence point between parent and child requests,
    // so truncate immediately before it.
    input.truncate(spawn_call_index + 1);

    if let Some(tools) = object.get_mut("tools") {
        *tools = normalize_tools_for_cache_prefix(tools);
    }

    Ok(body)
}

fn prompt_cache_key(request: &ResponsesRequest) -> Option<String> {
    request
        .body_json()
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn normalize_tools_for_cache_prefix(tools: &Value) -> Value {
    let normalized_tools = tools
        .as_array()
        .unwrap_or_else(|| panic!("expected tools array: {tools:?}"))
        .iter()
        .filter_map(normalize_tool_for_cache_prefix)
        .collect::<Vec<_>>();
    Value::Array(normalized_tools)
}

fn normalize_tool_for_cache_prefix(tool: &Value) -> Option<Value> {
    let mut normalized = tool
        .as_object()
        .unwrap_or_else(|| panic!("expected tool object: {tool:?}"))
        .clone();

    if normalized.get("type").and_then(Value::as_str) == Some("namespace")
        && let Some(namespace_tools) = normalized.get("tools")
    {
        normalized.insert(
            "tools".to_string(),
            normalize_namespace_tools_for_cache_prefix(namespace_tools),
        );
    }

    if normalized
        .get("defer_loading")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && normalized.get("type").and_then(Value::as_str) == Some("function")
    {
        normalized.remove("parameters");
    }

    Some(Value::Object(normalized))
}

fn normalize_namespace_tools_for_cache_prefix(tools: &Value) -> Value {
    let normalized_tools = tools
        .as_array()
        .unwrap_or_else(|| panic!("expected namespace tools array: {tools:?}"))
        .iter()
        .filter_map(|tool| {
            let tool_object = tool
                .as_object()
                .unwrap_or_else(|| panic!("expected namespace tool object: {tool:?}"))
                .clone();
            if tool_object
                .get("defer_loading")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                && tool_object.get("type").and_then(Value::as_str) == Some("function")
            {
                None
            } else {
                normalize_tool_for_cache_prefix(&Value::Object(tool_object))
            }
        })
        .collect::<Vec<_>>();
    Value::Array(normalized_tools)
}

fn tool_parameter_description(
    req: &ResponsesRequest,
    tool_name: &str,
    parameter_name: &str,
) -> Option<String> {
    req.body_json()
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .and_then(|tools| {
            tools.iter().find_map(|tool| {
                if tool.get("name").and_then(serde_json::Value::as_str) == Some(tool_name) {
                    tool.get("parameters")
                        .and_then(|parameters| parameters.get("properties"))
                        .and_then(|properties| properties.get(parameter_name))
                        .and_then(|parameter| parameter.get("description"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                } else {
                    None
                }
            })
        })
}

fn role_block(description: &str, role_name: &str) -> Option<String> {
    let role_header = format!("{role_name}: {{");
    let mut lines = description.lines().skip_while(|line| *line != role_header);
    let first_line = lines.next()?;
    let mut block = vec![first_line];
    for line in lines {
        if line.ends_with(": {") {
            break;
        }
        block.push(line);
    }
    Some(block.join("\n"))
}

async fn wait_for_spawned_thread_id(test: &TestCodex) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let ids = test.thread_manager.list_thread_ids().await;
        if let Some(spawned_id) = ids
            .iter()
            .find(|id| **id != test.session_configured.session_id)
        {
            return Ok(spawned_id.to_string());
        }
        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for spawned thread id");
        }
        sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_requests(
    mock: &core_test_support::responses::ResponseMock,
) -> Result<Vec<ResponsesRequest>> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let requests = mock.requests();
        if !requests.is_empty() {
            return Ok(requests);
        }
        if Instant::now() >= deadline {
            anyhow::bail!("expected at least 1 request, got {}", requests.len());
        }
        sleep(Duration::from_millis(10)).await;
    }
}

async fn setup_turn_one_with_spawned_child(
    server: &MockServer,
    child_response_delay: Option<Duration>,
) -> Result<(TestCodex, String)> {
    setup_turn_one_with_custom_spawned_child(
        server,
        json!({
            "message": CHILD_PROMPT,
        }),
        child_response_delay,
        /*wait_for_parent_notification*/ true,
        |builder| builder,
    )
    .await
}

async fn setup_turn_one_with_custom_spawned_child(
    server: &MockServer,
    spawn_args: serde_json::Value,
    child_response_delay: Option<Duration>,
    wait_for_parent_notification: bool,
    configure_test: impl FnOnce(
        core_test_support::test_codex::TestCodexBuilder,
    ) -> core_test_support::test_codex::TestCodexBuilder,
) -> Result<(TestCodex, String)> {
    let spawn_args = serde_json::to_string(&spawn_args)?;

    mount_sse_once_match(
        server,
        |req: &wiremock::Request| body_contains(req, TURN_1_PROMPT),
        sse(vec![
            ev_response_created("resp-turn1-1"),
            ev_function_call(SPAWN_CALL_ID, "spawn_agent", &spawn_args),
            ev_completed("resp-turn1-1"),
        ]),
    )
    .await;

    let child_sse = sse(vec![
        ev_response_created("resp-child-1"),
        ev_assistant_message("msg-child-1", "child done"),
        ev_completed("resp-child-1"),
    ]);
    let child_request_log = if let Some(delay) = child_response_delay {
        mount_response_once_match(
            server,
            |req: &wiremock::Request| {
                body_contains(req, CHILD_PROMPT) && !body_contains(req, SPAWN_CALL_ID)
            },
            sse_response(child_sse).set_delay(delay),
        )
        .await
    } else {
        mount_sse_once_match(
            server,
            |req: &wiremock::Request| {
                body_contains(req, CHILD_PROMPT) && !body_contains(req, SPAWN_CALL_ID)
            },
            child_sse,
        )
        .await
    };

    let _turn1_followup = mount_sse_once_match(
        server,
        |req: &wiremock::Request| body_contains(req, SPAWN_CALL_ID),
        sse(vec![
            ev_response_created("resp-turn1-2"),
            ev_assistant_message("msg-turn1-2", "parent done"),
            ev_completed("resp-turn1-2"),
        ]),
    )
    .await;

    #[allow(clippy::expect_used)]
    let mut builder = configure_test(test_codex().with_config(|config| {
        config
            .features
            .enable(Feature::Collab)
            .expect("test config should allow feature update");
        config.model = Some(INHERITED_MODEL.to_string());
        config.model_reasoning_effort = Some(INHERITED_REASONING_EFFORT);
    }));
    let test = builder.build(server).await?;
    test.submit_turn(TURN_1_PROMPT).await?;
    if child_response_delay.is_none() && wait_for_parent_notification {
        let _ = wait_for_requests(&child_request_log).await?;
        let rollout_path = test
            .codex
            .rollout_path()
            .ok_or_else(|| anyhow::anyhow!("expected parent rollout path"))?;
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            let has_notification = tokio::fs::read_to_string(&rollout_path)
                .await
                .is_ok_and(|rollout| rollout.contains("<subagent_notification>"));
            if has_notification {
                break;
            }
            if Instant::now() >= deadline {
                anyhow::bail!(
                    "timed out waiting for parent rollout to include subagent notification"
                );
            }
            sleep(Duration::from_millis(10)).await;
        }
    }
    let spawned_id = wait_for_spawned_thread_id(&test).await?;

    Ok((test, spawned_id))
}

async fn spawn_child_and_capture_snapshot(
    server: &MockServer,
    spawn_args: serde_json::Value,
    configure_test: impl FnOnce(
        core_test_support::test_codex::TestCodexBuilder,
    ) -> core_test_support::test_codex::TestCodexBuilder,
) -> Result<ThreadConfigSnapshot> {
    let (test, spawned_id) = setup_turn_one_with_custom_spawned_child(
        server,
        spawn_args,
        /*child_response_delay*/ None,
        /*wait_for_parent_notification*/ false,
        configure_test,
    )
    .await?;
    let thread_id = ThreadId::from_string(&spawned_id)?;
    Ok(test
        .thread_manager
        .get_thread(thread_id)
        .await?
        .config_snapshot()
        .await)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_notification_is_included_without_wait() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let (test, _spawned_id) =
        setup_turn_one_with_spawned_child(&server, /*child_response_delay*/ None).await?;

    let turn2 = mount_sse_once_match(
        &server,
        |req: &wiremock::Request| body_contains(req, TURN_2_NO_WAIT_PROMPT),
        sse(vec![
            ev_response_created("resp-turn2-1"),
            ev_assistant_message("msg-turn2-1", "no wait path"),
            ev_completed("resp-turn2-1"),
        ]),
    )
    .await;
    test.submit_turn(TURN_2_NO_WAIT_PROMPT).await?;

    let turn2_requests = wait_for_requests(&turn2).await?;
    assert!(turn2_requests.iter().any(has_subagent_notification));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawned_child_receives_forked_parent_context() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let seed_turn = mount_sse_once_match(
        &server,
        |req: &wiremock::Request| body_contains(req, TURN_0_FORK_PROMPT),
        sse(vec![
            ev_response_created("resp-seed-1"),
            ev_assistant_message("msg-seed-1", "seeded"),
            ev_completed("resp-seed-1"),
        ]),
    )
    .await;

    let spawn_args = serde_json::to_string(&json!({
        "message": CHILD_PROMPT,
        "fork_context": true,
    }))?;
    let spawn_turn = mount_sse_once_match(
        &server,
        |req: &wiremock::Request| body_contains(req, TURN_1_PROMPT),
        sse(vec![
            ev_response_created("resp-turn1-1"),
            ev_function_call(SPAWN_CALL_ID, "spawn_agent", &spawn_args),
            ev_completed("resp-turn1-1"),
        ]),
    )
    .await;

    let child_request_log = mount_sse_once_match(
        &server,
        |req: &wiremock::Request| {
            body_contains(req, CHILD_PROMPT)
                && body_contains(req, FORKED_SPAWN_AGENT_OUTPUT_MESSAGE)
        },
        sse(vec![
            ev_response_created("resp-child-1"),
            ev_assistant_message("msg-child-1", "child done"),
            ev_completed("resp-child-1"),
        ]),
    )
    .await;

    let turn1_followup = mount_sse_once_match(
        &server,
        |req: &wiremock::Request| {
            body_contains(req, SPAWN_CALL_ID) && !body_contains(req, CHILD_PROMPT)
        },
        sse(vec![
            ev_response_created("resp-turn1-2"),
            ev_assistant_message("msg-turn1-2", "parent done"),
            ev_completed("resp-turn1-2"),
        ]),
    )
    .await;

    let mut builder = test_codex().with_config(|config| {
        config
            .features
            .enable(Feature::Collab)
            .expect("test config should allow feature update");
    });
    let test = builder.build(&server).await?;

    test.submit_turn(TURN_0_FORK_PROMPT).await?;
    let _ = seed_turn.single_request();

    test.submit_turn(TURN_1_PROMPT).await?;
    let _ = spawn_turn.single_request();
    let parent_followup_requests = wait_for_requests(&turn1_followup).await?;
    let parent_followup_request = parent_followup_requests
        .first()
        .expect("parent follow-up request should be captured");

    let deadline = Instant::now() + Duration::from_secs(2);
    let child_request = loop {
        if let Some(request) = child_request_log
            .requests()
            .into_iter()
            .find(|request| request.body_contains_text(CHILD_PROMPT))
        {
            break request;
        }
        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for forked child request");
        }
        sleep(Duration::from_millis(10)).await;
    };
    assert!(child_request.body_contains_text(TURN_0_FORK_PROMPT));
    assert!(child_request.body_contains_text("seeded"));
    let parent_cache_prefix = cache_prefix_request_body(parent_followup_request, SPAWN_CALL_ID)?;
    let child_cache_prefix = cache_prefix_request_body(&child_request, SPAWN_CALL_ID)?;
    assert_eq!(
        prompt_cache_key(parent_followup_request),
        prompt_cache_key(&child_request),
        "forked parent and child requests must reuse the same prompt_cache_key so backend sharding can colocate them for KV cache reuse"
    );
    assert_eq!(
        parent_cache_prefix, child_cache_prefix,
        "forked child requests must preserve every cache-relevant request field and the conversation-item prefix exactly through the shared spawn_agent call; namespace shells and non-deferred tools must stay stable, while deferred namespace members may only appear after tool_search_output"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_agent_requested_model_and_reasoning_override_inherited_settings_without_role()
-> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let child_snapshot = spawn_child_and_capture_snapshot(
        &server,
        json!({
            "message": CHILD_PROMPT,
            "model": REQUESTED_MODEL,
            "reasoning_effort": REQUESTED_REASONING_EFFORT,
        }),
        |builder| builder,
    )
    .await?;

    assert_eq!(child_snapshot.model, REQUESTED_MODEL);
    assert_eq!(
        child_snapshot.reasoning_effort,
        Some(REQUESTED_REASONING_EFFORT)
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_agent_role_overrides_requested_model_and_reasoning_settings() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let child_snapshot = spawn_child_and_capture_snapshot(
        &server,
        json!({
            "message": CHILD_PROMPT,
            "agent_type": "custom",
            "model": REQUESTED_MODEL,
            "reasoning_effort": REQUESTED_REASONING_EFFORT,
        }),
        |builder| {
            builder.with_config(|config| {
                let role_path = config.codex_home.join("custom-role.toml");
                std::fs::write(
                    &role_path,
                    format!(
                        "model = \"{ROLE_MODEL}\"\nmodel_reasoning_effort = \"{ROLE_REASONING_EFFORT}\"\n",
                    ),
                )
                .expect("write role config");
                config.agent_roles.insert(
                    "custom".to_string(),
                    AgentRoleConfig {
                        description: Some("Custom role".to_string()),
                        config_file: Some(role_path),
                        nickname_candidates: None,
                    },
                );
            })
        },
    )
    .await?;

    assert_eq!(child_snapshot.model, ROLE_MODEL);
    assert_eq!(child_snapshot.reasoning_effort, Some(ROLE_REASONING_EFFORT));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_agent_tool_description_mentions_role_locked_settings() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let resp_mock = mount_sse_once_match(
        &server,
        |req: &wiremock::Request| body_contains(req, TURN_1_PROMPT),
        sse(vec![
            ev_response_created("resp-turn1-1"),
            ev_assistant_message("msg-turn1-1", "done"),
            ev_completed("resp-turn1-1"),
        ]),
    )
    .await;

    let mut builder = test_codex().with_config(|config| {
        config
            .features
            .enable(Feature::Collab)
            .expect("test config should allow feature update");
        let role_path = config.codex_home.join("custom-role.toml");
        std::fs::write(
            &role_path,
            format!(
                "developer_instructions = \"Stay focused\"\nmodel = \"{ROLE_MODEL}\"\nmodel_reasoning_effort = \"{ROLE_REASONING_EFFORT}\"\n",
            ),
        )
        .expect("write role config");
        config.agent_roles.insert(
            "custom".to_string(),
            AgentRoleConfig {
                description: Some("Custom role".to_string()),
                config_file: Some(role_path),
                nickname_candidates: None,
            },
        );
    });
    let test = builder.build(&server).await?;

    test.submit_turn(TURN_1_PROMPT).await?;

    let request = resp_mock.single_request();
    let agent_type_description = tool_parameter_description(&request, "spawn_agent", "agent_type")
        .expect("spawn_agent agent_type description");
    let custom_role_description =
        role_block(&agent_type_description, "custom").expect("custom role description");
    assert_eq!(
        custom_role_description,
        "custom: {\nCustom role\n- This role's model is set to `gpt-5.1-codex-max` and its reasoning effort is set to `high`. These settings cannot be changed.\n}"
    );

    Ok(())
}
