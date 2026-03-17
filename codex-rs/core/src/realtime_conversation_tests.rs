use super::RealtimeHandoffState;
use super::RealtimeSessionKind;
use super::realtime_api_key;
use super::realtime_text_from_handoff_request;
use crate::CodexAuth;
use crate::ModelProviderInfo;
use async_channel::bounded;
use codex_protocol::protocol::RealtimeHandoffRequested;
use codex_protocol::protocol::RealtimeTranscriptEntry;
use pretty_assertions::assert_eq;

#[test]
fn extracts_text_from_handoff_request_active_transcript() {
    let handoff = RealtimeHandoffRequested {
        handoff_id: "handoff_1".to_string(),
        item_id: "item_1".to_string(),
        input_transcript: "ignored".to_string(),
        active_transcript: vec![
            RealtimeTranscriptEntry {
                role: "user".to_string(),
                text: "hello".to_string(),
            },
            RealtimeTranscriptEntry {
                role: "assistant".to_string(),
                text: "hi there".to_string(),
            },
        ],
    };
    assert_eq!(
        realtime_text_from_handoff_request(&handoff),
        Some("user: hello\nassistant: hi there".to_string())
    );
}

#[test]
fn extracts_text_from_handoff_request_input_transcript_if_messages_missing() {
    let handoff = RealtimeHandoffRequested {
        handoff_id: "handoff_1".to_string(),
        item_id: "item_1".to_string(),
        input_transcript: "ignored".to_string(),
        active_transcript: vec![],
    };
    assert_eq!(
        realtime_text_from_handoff_request(&handoff),
        Some("ignored".to_string())
    );
}

#[test]
fn ignores_empty_handoff_request_input_transcript() {
    let handoff = RealtimeHandoffRequested {
        handoff_id: "handoff_1".to_string(),
        item_id: "item_1".to_string(),
        input_transcript: String::new(),
        active_transcript: vec![],
    };
    assert_eq!(realtime_text_from_handoff_request(&handoff), None);
}

#[tokio::test]
async fn clears_active_handoff_explicitly() {
    let (tx, _rx) = bounded(1);
    let state = RealtimeHandoffState::new(tx, RealtimeSessionKind::V1);

    *state.active_handoff.lock().await = Some("handoff_1".to_string());
    assert_eq!(
        state.active_handoff.lock().await.clone(),
        Some("handoff_1".to_string())
    );

    *state.active_handoff.lock().await = None;
    assert_eq!(state.active_handoff.lock().await.clone(), None);
}

#[test]
fn uses_openai_env_fallback_for_chatgpt_auth() {
    let auth = CodexAuth::create_dummy_chatgpt_auth_for_testing();
    let provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None);

    let api_key = realtime_api_key(Some(&auth), &provider, || {
        Some("env-realtime-key".to_string())
    })
    .expect("openai env fallback should provide realtime api key");

    assert_eq!(api_key, "env-realtime-key".to_string());
}

#[test]
fn errors_without_api_key_for_non_openai_chatgpt_auth() {
    let auth = CodexAuth::create_dummy_chatgpt_auth_for_testing();
    let mut provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None);
    provider.name = "Test Provider".to_string();

    let err = realtime_api_key(Some(&auth), &provider, || Some("ignored".to_string()))
        .expect_err("non-openai provider should not use openai env fallback");

    assert_eq!(
        err.to_string(),
        "realtime conversation requires API key auth"
    );
}
