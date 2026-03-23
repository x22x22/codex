use codex_client::current_trace_headers;

pub(crate) const X_CODEX_TRACEPARENT_META_KEY: &str = "x-codex-traceparent";
pub(crate) const X_CODEX_TRACESTATE_META_KEY: &str = "x-codex-tracestate";

pub(crate) fn inject_current_trace_into_meta(
    meta: Option<rmcp::model::Meta>,
) -> Option<rmcp::model::Meta> {
    let headers = current_trace_headers();
    let traceparent = headers
        .get("traceparent")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let tracestate = headers
        .get("tracestate")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    if traceparent.is_none() && tracestate.is_none() {
        return meta;
    }

    let mut meta = meta.unwrap_or_else(|| rmcp::model::Meta(Default::default()));
    if let Some(traceparent) = traceparent {
        meta.insert(
            X_CODEX_TRACEPARENT_META_KEY.to_string(),
            serde_json::Value::String(traceparent),
        );
    }
    if let Some(tracestate) = tracestate {
        meta.insert(
            X_CODEX_TRACESTATE_META_KEY.to_string(),
            serde_json::Value::String(tracestate),
        );
    }
    Some(meta)
}
