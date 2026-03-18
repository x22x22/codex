use std::path::PathBuf;

use crate::ModelClient;
use crate::error::Result;
pub use codex_memories::memory_trace::BuiltMemory;
use codex_otel::SessionTelemetry;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;

/// Loads raw trace files, normalizes items, and builds memory summaries.
///
/// The request/response wiring mirrors the memory summarize E2E flow:
/// `/v1/memories/trace_summarize` with one output object per input raw memory.
///
/// The caller provides the model selection, reasoning effort, and telemetry context explicitly so
/// the session-scoped [`ModelClient`] can be reused across turns.
pub async fn build_memories_from_trace_files(
    client: &ModelClient,
    trace_paths: &[PathBuf],
    model_info: &ModelInfo,
    effort: Option<ReasoningEffortConfig>,
    session_telemetry: &SessionTelemetry,
) -> Result<Vec<BuiltMemory>> {
    if trace_paths.is_empty() {
        return Ok(Vec::new());
    }

    let prepared = codex_memories::memory_trace::load_trace_requests(trace_paths)
        .await
        .map_err(|err| crate::error::CodexErr::InvalidRequest(err.to_string()))?;
    let raw_memories = prepared.iter().map(|trace| trace.payload.clone()).collect();
    let output = client
        .summarize_memories(raw_memories, model_info, effort, session_telemetry)
        .await?;
    codex_memories::memory_trace::build_memories_from_output(prepared, output)
        .map_err(|err| crate::error::CodexErr::InvalidRequest(err.to_string()))
}
