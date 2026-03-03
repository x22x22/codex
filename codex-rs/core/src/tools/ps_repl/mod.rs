use std::collections::HashMap;
use std::collections::VecDeque;
use std::fmt;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use codex_protocol::ThreadId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseInputItem;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::Child;
use tokio::process::ChildStdin;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing::trace;
use tracing::warn;
use uuid::Uuid;

use crate::client_common::tools::ToolSpec;
use crate::codex::Session;
use crate::codex::TurnContext;
use crate::exec::ExecExpiration;
use crate::exec_env::create_env;
use crate::function_tool::FunctionCallError;
use crate::sandboxing::CommandSpec;
use crate::sandboxing::SandboxManager;
use crate::sandboxing::SandboxPermissions;
use crate::tools::ToolRouter;
use crate::tools::context::SharedTurnDiffTracker;
use crate::tools::sandboxing::SandboxablePreference;
use crate::truncate::TruncationPolicy;
use crate::truncate::truncate_text;

pub(crate) const PS_REPL_PRAGMA_PREFIX: &str = "# codex-ps-repl:";
const KERNEL_SOURCE: &str = include_str!("kernel.ps1");
const PS_REPL_MIN_PWSH_MAJOR: u64 = 7;
const PS_REPL_STDERR_TAIL_LINE_LIMIT: usize = 20;
const PS_REPL_STDERR_TAIL_LINE_MAX_BYTES: usize = 512;
const PS_REPL_STDERR_TAIL_MAX_BYTES: usize = 4_096;
const PS_REPL_STDERR_TAIL_SEPARATOR: &str = " | ";
const PS_REPL_EXEC_ID_LOG_LIMIT: usize = 8;
const PS_REPL_MODEL_DIAG_STDERR_MAX_BYTES: usize = 1_024;
const PS_REPL_MODEL_DIAG_ERROR_MAX_BYTES: usize = 256;
const PS_REPL_TOOL_RESPONSE_TEXT_PREVIEW_MAX_BYTES: usize = 512;

/// Per-task ps_repl handle stored on the turn context.
pub(crate) struct PsReplHandle {
    pwsh_path: Option<PathBuf>,
    cell: OnceCell<Arc<PsReplManager>>,
}

impl fmt::Debug for PsReplHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PsReplHandle").finish_non_exhaustive()
    }
}

impl PsReplHandle {
    pub(crate) fn with_pwsh_path(pwsh_path: Option<PathBuf>) -> Self {
        Self {
            pwsh_path,
            cell: OnceCell::new(),
        }
    }

    pub(crate) async fn manager(&self) -> Result<Arc<PsReplManager>, FunctionCallError> {
        self.cell
            .get_or_try_init(|| async { PsReplManager::new(self.pwsh_path.clone()).await })
            .await
            .cloned()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PsReplArgs {
    pub code: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct PsExecResult {
    pub output: String,
    pub content_items: Vec<FunctionCallOutputContentItem>,
}

struct KernelState {
    child: Arc<Mutex<Child>>,
    recent_stderr: Arc<Mutex<VecDeque<String>>>,
    stdin: Arc<Mutex<ChildStdin>>,
    pending_execs: Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<ExecResultMessage>>>>,
    exec_contexts: Arc<Mutex<HashMap<String, ExecContext>>>,
    shutdown: CancellationToken,
}

#[derive(Clone)]
struct ExecContext {
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    tracker: SharedTurnDiffTracker,
}

#[derive(Default)]
struct ExecToolCalls {
    in_flight: usize,
    content_items: Vec<FunctionCallOutputContentItem>,
    notify: Arc<Notify>,
    cancel: CancellationToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
enum PsReplToolCallPayloadKind {
    MessageContent,
    FunctionText,
    FunctionContentItems,
    CustomText,
    CustomContentItems,
    McpResult,
    McpErrorResult,
    Error,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PsReplToolCallResponseSummary {
    response_type: Option<String>,
    payload_kind: Option<PsReplToolCallPayloadKind>,
    payload_text_preview: Option<String>,
    payload_text_length: Option<usize>,
    payload_item_count: Option<usize>,
    text_item_count: Option<usize>,
    image_item_count: Option<usize>,
    structured_content_present: Option<bool>,
    result_is_error: Option<bool>,
}

enum KernelStreamEnd {
    Shutdown,
    StdoutEof,
    StdoutReadError(String),
}

impl KernelStreamEnd {
    fn reason(&self) -> &'static str {
        match self {
            Self::Shutdown => "shutdown",
            Self::StdoutEof => "stdout_eof",
            Self::StdoutReadError(_) => "stdout_read_error",
        }
    }

    fn error(&self) -> Option<&str> {
        match self {
            Self::StdoutReadError(err) => Some(err),
            _ => None,
        }
    }
}

struct KernelDebugSnapshot {
    pid: Option<u32>,
    status: String,
    stderr_tail: String,
}

fn format_exit_status(status: std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("code={code}");
    }
    #[cfg(unix)]
    if let Some(signal) = status.signal() {
        return format!("signal={signal}");
    }
    "unknown".to_string()
}

fn format_stderr_tail(lines: &VecDeque<String>) -> String {
    if lines.is_empty() {
        return "<empty>".to_string();
    }
    lines
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join(PS_REPL_STDERR_TAIL_SEPARATOR)
}

fn truncate_utf8_prefix_by_bytes(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_string();
    }
    if max_bytes == 0 {
        return String::new();
    }
    let mut end = max_bytes;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    input[..end].to_string()
}

fn stderr_tail_formatted_bytes(lines: &VecDeque<String>) -> usize {
    if lines.is_empty() {
        return 0;
    }
    let payload_bytes: usize = lines.iter().map(String::len).sum();
    let separator_bytes = PS_REPL_STDERR_TAIL_SEPARATOR.len() * (lines.len() - 1);
    payload_bytes + separator_bytes
}

fn stderr_tail_bytes_with_candidate(lines: &VecDeque<String>, line: &str) -> usize {
    if lines.is_empty() {
        return line.len();
    }
    stderr_tail_formatted_bytes(lines) + PS_REPL_STDERR_TAIL_SEPARATOR.len() + line.len()
}

fn push_stderr_tail_line(lines: &mut VecDeque<String>, line: &str) -> String {
    let max_line_bytes = PS_REPL_STDERR_TAIL_LINE_MAX_BYTES.min(PS_REPL_STDERR_TAIL_MAX_BYTES);
    let bounded_line = truncate_utf8_prefix_by_bytes(line, max_line_bytes);
    if bounded_line.is_empty() {
        return bounded_line;
    }

    while !lines.is_empty()
        && (lines.len() >= PS_REPL_STDERR_TAIL_LINE_LIMIT
            || stderr_tail_bytes_with_candidate(lines, &bounded_line)
                > PS_REPL_STDERR_TAIL_MAX_BYTES)
    {
        lines.pop_front();
    }

    lines.push_back(bounded_line.clone());
    bounded_line
}

fn is_kernel_status_exited(status: &str) -> bool {
    status.starts_with("exited(")
}

fn should_include_model_diagnostics_for_write_error(
    err_message: &str,
    snapshot: &KernelDebugSnapshot,
) -> bool {
    is_kernel_status_exited(&snapshot.status)
        || err_message.to_ascii_lowercase().contains("broken pipe")
}

fn format_model_kernel_failure_details(
    reason: &str,
    stream_error: Option<&str>,
    snapshot: &KernelDebugSnapshot,
) -> String {
    let payload = serde_json::json!({
        "reason": reason,
        "stream_error": stream_error
            .map(|err| truncate_utf8_prefix_by_bytes(err, PS_REPL_MODEL_DIAG_ERROR_MAX_BYTES)),
        "kernel_pid": snapshot.pid,
        "kernel_status": snapshot.status,
        "kernel_stderr_tail": truncate_utf8_prefix_by_bytes(
            &snapshot.stderr_tail,
            PS_REPL_MODEL_DIAG_STDERR_MAX_BYTES,
        ),
    });
    let encoded = serde_json::to_string(&payload)
        .unwrap_or_else(|err| format!(r#"{{"reason":"serialization_error","error":"{err}"}}"#));
    format!("ps_repl diagnostics: {encoded}")
}

fn with_model_kernel_failure_message(
    base_message: &str,
    reason: &str,
    stream_error: Option<&str>,
    snapshot: &KernelDebugSnapshot,
) -> String {
    format!(
        "{base_message}\n\n{}",
        format_model_kernel_failure_details(reason, stream_error, snapshot)
    )
}

pub struct PsReplManager {
    pwsh_path: Option<PathBuf>,
    tmp_dir: tempfile::TempDir,
    kernel: Arc<Mutex<Option<KernelState>>>,
    exec_lock: Arc<tokio::sync::Semaphore>,
    exec_tool_calls: Arc<Mutex<HashMap<String, ExecToolCalls>>>,
}

impl PsReplManager {
    async fn new(pwsh_path: Option<PathBuf>) -> Result<Arc<Self>, FunctionCallError> {
        let tmp_dir = tempfile::tempdir().map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to create ps_repl temp dir: {err}"))
        })?;

        let manager = Arc::new(Self {
            pwsh_path,
            tmp_dir,
            kernel: Arc::new(Mutex::new(None)),
            exec_lock: Arc::new(tokio::sync::Semaphore::new(1)),
            exec_tool_calls: Arc::new(Mutex::new(HashMap::new())),
        });

        Ok(manager)
    }

    async fn register_exec_tool_calls(&self, exec_id: &str) {
        self.exec_tool_calls
            .lock()
            .await
            .insert(exec_id.to_string(), ExecToolCalls::default());
    }

    async fn clear_exec_tool_calls(&self, exec_id: &str) {
        if let Some(state) = self.exec_tool_calls.lock().await.remove(exec_id) {
            state.cancel.cancel();
            state.notify.notify_waiters();
        }
    }

    async fn wait_for_exec_tool_calls(&self, exec_id: &str) {
        loop {
            let notified = {
                let calls = self.exec_tool_calls.lock().await;
                calls
                    .get(exec_id)
                    .filter(|state| state.in_flight > 0)
                    .map(|state| Arc::clone(&state.notify).notified_owned())
            };
            match notified {
                Some(notified) => notified.await,
                None => return,
            }
        }
    }

    async fn begin_exec_tool_call(
        exec_tool_calls: &Arc<Mutex<HashMap<String, ExecToolCalls>>>,
        exec_id: &str,
    ) -> Option<CancellationToken> {
        let mut calls = exec_tool_calls.lock().await;
        let state = calls.get_mut(exec_id)?;
        state.in_flight += 1;
        Some(state.cancel.clone())
    }

    async fn record_exec_tool_call_content_items(
        exec_tool_calls: &Arc<Mutex<HashMap<String, ExecToolCalls>>>,
        exec_id: &str,
        content_items: Vec<FunctionCallOutputContentItem>,
    ) {
        if content_items.is_empty() {
            return;
        }

        let mut calls = exec_tool_calls.lock().await;
        if let Some(state) = calls.get_mut(exec_id) {
            state.content_items.extend(content_items);
        }
    }

    async fn finish_exec_tool_call(
        exec_tool_calls: &Arc<Mutex<HashMap<String, ExecToolCalls>>>,
        exec_id: &str,
    ) {
        let notify = {
            let mut calls = exec_tool_calls.lock().await;
            let Some(state) = calls.get_mut(exec_id) else {
                return;
            };
            if state.in_flight == 0 {
                return;
            }
            state.in_flight -= 1;
            if state.in_flight == 0 {
                Some(Arc::clone(&state.notify))
            } else {
                None
            }
        };
        if let Some(notify) = notify {
            notify.notify_waiters();
        }
    }

    async fn wait_for_exec_tool_calls_map(
        exec_tool_calls: &Arc<Mutex<HashMap<String, ExecToolCalls>>>,
        exec_id: &str,
    ) {
        loop {
            let notified = {
                let calls = exec_tool_calls.lock().await;
                calls
                    .get(exec_id)
                    .filter(|state| state.in_flight > 0)
                    .map(|state| Arc::clone(&state.notify).notified_owned())
            };
            match notified {
                Some(notified) => notified.await,
                None => return,
            }
        }
    }

    async fn clear_exec_tool_calls_map(
        exec_tool_calls: &Arc<Mutex<HashMap<String, ExecToolCalls>>>,
        exec_id: &str,
    ) {
        if let Some(state) = exec_tool_calls.lock().await.remove(exec_id) {
            state.cancel.cancel();
            state.notify.notify_waiters();
        }
    }

    async fn clear_all_exec_tool_calls_map(
        exec_tool_calls: &Arc<Mutex<HashMap<String, ExecToolCalls>>>,
    ) {
        let states = {
            let mut calls = exec_tool_calls.lock().await;
            calls.drain().map(|(_, state)| state).collect::<Vec<_>>()
        };
        for state in states {
            state.cancel.cancel();
            state.notify.notify_waiters();
        }
    }

    fn log_tool_call_response(
        req: &RunToolRequest,
        ok: bool,
        summary: &PsReplToolCallResponseSummary,
        response: Option<&JsonValue>,
        error: Option<&str>,
    ) {
        info!(
            exec_id = %req.exec_id,
            tool_call_id = %req.id,
            tool_name = %req.tool_name,
            ok,
            summary = ?summary,
            "ps_repl nested tool call completed"
        );
        if let Some(response) = response {
            trace!(
                exec_id = %req.exec_id,
                tool_call_id = %req.id,
                tool_name = %req.tool_name,
                response_json = %response,
                "ps_repl nested tool call raw response"
            );
        }
        if let Some(error) = error {
            trace!(
                exec_id = %req.exec_id,
                tool_call_id = %req.id,
                tool_name = %req.tool_name,
                error = %error,
                "ps_repl nested tool call raw error"
            );
        }
    }

    fn summarize_text_payload(
        response_type: Option<&str>,
        payload_kind: PsReplToolCallPayloadKind,
        text: &str,
    ) -> PsReplToolCallResponseSummary {
        PsReplToolCallResponseSummary {
            response_type: response_type.map(str::to_owned),
            payload_kind: Some(payload_kind),
            payload_text_preview: (!text.is_empty()).then(|| {
                truncate_text(
                    text,
                    TruncationPolicy::Bytes(PS_REPL_TOOL_RESPONSE_TEXT_PREVIEW_MAX_BYTES),
                )
            }),
            payload_text_length: Some(text.len()),
            ..Default::default()
        }
    }

    fn summarize_function_output_payload(
        response_type: &str,
        payload_kind: PsReplToolCallPayloadKind,
        output: &FunctionCallOutputPayload,
    ) -> PsReplToolCallResponseSummary {
        let (payload_item_count, text_item_count, image_item_count) =
            if let Some(items) = output.content_items() {
                let text_item_count = items
                    .iter()
                    .filter(|item| matches!(item, FunctionCallOutputContentItem::InputText { .. }))
                    .count();
                let image_item_count = items.len().saturating_sub(text_item_count);
                (
                    Some(items.len()),
                    Some(text_item_count),
                    Some(image_item_count),
                )
            } else {
                (None, None, None)
            };
        let payload_text = output.body.to_text();
        PsReplToolCallResponseSummary {
            response_type: Some(response_type.to_string()),
            payload_kind: Some(payload_kind),
            payload_text_preview: payload_text.as_deref().and_then(|text| {
                (!text.is_empty()).then(|| {
                    truncate_text(
                        text,
                        TruncationPolicy::Bytes(PS_REPL_TOOL_RESPONSE_TEXT_PREVIEW_MAX_BYTES),
                    )
                })
            }),
            payload_text_length: payload_text.as_ref().map(String::len),
            payload_item_count,
            text_item_count,
            image_item_count,
            ..Default::default()
        }
    }

    fn summarize_message_payload(content: &[ContentItem]) -> PsReplToolCallResponseSummary {
        let text_item_count = content
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    ContentItem::InputText { .. } | ContentItem::OutputText { .. }
                )
            })
            .count();
        let image_item_count = content.len().saturating_sub(text_item_count);
        let payload_text = content
            .iter()
            .filter_map(|item| match item {
                ContentItem::InputText { text } | ContentItem::OutputText { text }
                    if !text.trim().is_empty() =>
                {
                    Some(text.as_str())
                }
                ContentItem::InputText { .. }
                | ContentItem::InputImage { .. }
                | ContentItem::OutputText { .. } => None,
            })
            .collect::<Vec<_>>();
        let payload_text = if payload_text.is_empty() {
            None
        } else {
            Some(payload_text.join("\n"))
        };
        PsReplToolCallResponseSummary {
            response_type: Some("message".to_string()),
            payload_kind: Some(PsReplToolCallPayloadKind::MessageContent),
            payload_text_preview: payload_text.as_deref().and_then(|text| {
                (!text.is_empty()).then(|| {
                    truncate_text(
                        text,
                        TruncationPolicy::Bytes(PS_REPL_TOOL_RESPONSE_TEXT_PREVIEW_MAX_BYTES),
                    )
                })
            }),
            payload_text_length: payload_text.as_ref().map(String::len),
            payload_item_count: Some(content.len()),
            text_item_count: Some(text_item_count),
            image_item_count: Some(image_item_count),
            ..Default::default()
        }
    }

    fn summarize_tool_call_response(response: &ResponseInputItem) -> PsReplToolCallResponseSummary {
        match response {
            ResponseInputItem::Message { content, .. } => Self::summarize_message_payload(content),
            ResponseInputItem::FunctionCallOutput { output, .. } => {
                let payload_kind = if output.content_items().is_some() {
                    PsReplToolCallPayloadKind::FunctionContentItems
                } else {
                    PsReplToolCallPayloadKind::FunctionText
                };
                Self::summarize_function_output_payload(
                    "function_call_output",
                    payload_kind,
                    output,
                )
            }
            ResponseInputItem::CustomToolCallOutput { output, .. } => {
                let payload_kind = if output.content_items().is_some() {
                    PsReplToolCallPayloadKind::CustomContentItems
                } else {
                    PsReplToolCallPayloadKind::CustomText
                };
                Self::summarize_function_output_payload(
                    "custom_tool_call_output",
                    payload_kind,
                    output,
                )
            }
            ResponseInputItem::McpToolCallOutput { result, .. } => match result {
                Ok(result) => {
                    let output = FunctionCallOutputPayload::from(result);
                    let mut summary = Self::summarize_function_output_payload(
                        "mcp_tool_call_output",
                        PsReplToolCallPayloadKind::McpResult,
                        &output,
                    );
                    summary.payload_item_count = Some(result.content.len());
                    summary.structured_content_present = Some(result.structured_content.is_some());
                    summary.result_is_error = Some(result.is_error.unwrap_or(false));
                    summary
                }
                Err(error) => {
                    let mut summary = Self::summarize_text_payload(
                        Some("mcp_tool_call_output"),
                        PsReplToolCallPayloadKind::McpErrorResult,
                        error,
                    );
                    summary.result_is_error = Some(true);
                    summary
                }
            },
        }
    }

    fn summarize_tool_call_error(error: &str) -> PsReplToolCallResponseSummary {
        Self::summarize_text_payload(None, PsReplToolCallPayloadKind::Error, error)
    }

    pub async fn reset(&self) -> Result<(), FunctionCallError> {
        let _permit = self.exec_lock.clone().acquire_owned().await.map_err(|_| {
            FunctionCallError::RespondToModel("ps_repl execution unavailable".to_string())
        })?;
        self.reset_kernel().await;
        Self::clear_all_exec_tool_calls_map(&self.exec_tool_calls).await;
        Ok(())
    }

    async fn reset_kernel(&self) {
        let state = {
            let mut guard = self.kernel.lock().await;
            guard.take()
        };
        if let Some(state) = state {
            state.shutdown.cancel();
            Self::kill_kernel_child(&state.child, "reset").await;
        }
    }

    pub async fn execute(
        &self,
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        tracker: SharedTurnDiffTracker,
        args: PsReplArgs,
    ) -> Result<PsExecResult, FunctionCallError> {
        let _permit = self.exec_lock.clone().acquire_owned().await.map_err(|_| {
            FunctionCallError::RespondToModel("ps_repl execution unavailable".to_string())
        })?;

        let (stdin, pending_execs, exec_contexts, child, recent_stderr) = {
            let mut kernel = self.kernel.lock().await;
            if kernel.is_none() {
                let state = self
                    .start_kernel(Arc::clone(&turn), Some(session.conversation_id))
                    .await
                    .map_err(FunctionCallError::RespondToModel)?;
                *kernel = Some(state);
            }

            let state = match kernel.as_ref() {
                Some(state) => state,
                None => {
                    return Err(FunctionCallError::RespondToModel(
                        "ps_repl kernel unavailable".to_string(),
                    ));
                }
            };
            (
                Arc::clone(&state.stdin),
                Arc::clone(&state.pending_execs),
                Arc::clone(&state.exec_contexts),
                Arc::clone(&state.child),
                Arc::clone(&state.recent_stderr),
            )
        };

        let (req_id, rx) = {
            let req_id = Uuid::new_v4().to_string();
            let mut pending = pending_execs.lock().await;
            let (tx, rx) = tokio::sync::oneshot::channel();
            pending.insert(req_id.clone(), tx);
            exec_contexts.lock().await.insert(
                req_id.clone(),
                ExecContext {
                    session: Arc::clone(&session),
                    turn: Arc::clone(&turn),
                    tracker,
                },
            );
            (req_id, rx)
        };
        self.register_exec_tool_calls(&req_id).await;

        let payload = HostToKernel::Exec {
            id: req_id.clone(),
            code: args.code,
            timeout_ms: args.timeout_ms,
        };

        if let Err(err) = Self::write_message(&stdin, &payload).await {
            pending_execs.lock().await.remove(&req_id);
            exec_contexts.lock().await.remove(&req_id);
            self.clear_exec_tool_calls(&req_id).await;
            let snapshot = Self::kernel_debug_snapshot(&child, &recent_stderr).await;
            let err_message = err.to_string();
            warn!(
                exec_id = %req_id,
                error = %err_message,
                kernel_pid = ?snapshot.pid,
                kernel_status = %snapshot.status,
                kernel_stderr_tail = %snapshot.stderr_tail,
                "failed to submit ps_repl exec request to kernel"
            );
            let message =
                if should_include_model_diagnostics_for_write_error(&err_message, &snapshot) {
                    with_model_kernel_failure_message(
                        &err_message,
                        "write_failed",
                        Some(&err_message),
                        &snapshot,
                    )
                } else {
                    err_message
                };
            return Err(FunctionCallError::RespondToModel(message));
        }

        let timeout_ms = args.timeout_ms.unwrap_or(30_000);
        let response = match tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await {
            Ok(Ok(msg)) => msg,
            Ok(Err(_)) => {
                let mut pending = pending_execs.lock().await;
                pending.remove(&req_id);
                exec_contexts.lock().await.remove(&req_id);
                self.wait_for_exec_tool_calls(&req_id).await;
                self.clear_exec_tool_calls(&req_id).await;
                let snapshot = Self::kernel_debug_snapshot(&child, &recent_stderr).await;
                let message = if is_kernel_status_exited(&snapshot.status) {
                    with_model_kernel_failure_message(
                        "ps_repl kernel closed unexpectedly",
                        "response_channel_closed",
                        None,
                        &snapshot,
                    )
                } else {
                    "ps_repl kernel closed unexpectedly".to_string()
                };
                return Err(FunctionCallError::RespondToModel(message));
            }
            Err(_) => {
                self.reset_kernel().await;
                self.wait_for_exec_tool_calls(&req_id).await;
                self.exec_tool_calls.lock().await.clear();
                return Err(FunctionCallError::RespondToModel(
                    "ps_repl execution timed out; kernel reset, rerun your request".to_string(),
                ));
            }
        };

        match response {
            ExecResultMessage::Ok { content_items } => {
                let (output, content_items) = split_exec_result_content_items(content_items);
                Ok(PsExecResult {
                    output,
                    content_items,
                })
            }
            ExecResultMessage::Err { message } => Err(FunctionCallError::RespondToModel(message)),
        }
    }

    async fn start_kernel(
        &self,
        turn: Arc<TurnContext>,
        thread_id: Option<ThreadId>,
    ) -> Result<KernelState, String> {
        let pwsh_path = resolve_compatible_pwsh(self.pwsh_path.as_deref()).await?;

        let kernel_path = self
            .write_kernel_script()
            .await
            .map_err(|err| err.to_string())?;

        let mut env = create_env(&turn.shell_environment_policy, thread_id);
        env.insert(
            "CODEX_PS_TMP_DIR".to_string(),
            self.tmp_dir.path().to_string_lossy().to_string(),
        );

        let spec = CommandSpec {
            program: pwsh_path.to_string_lossy().to_string(),
            args: vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-File".to_string(),
                kernel_path.to_string_lossy().to_string(),
            ],
            cwd: turn.cwd.clone(),
            env,
            expiration: ExecExpiration::DefaultTimeout,
            sandbox_permissions: SandboxPermissions::UseDefault,
            additional_permissions: None,
            justification: None,
        };

        let sandbox = SandboxManager::new();
        let has_managed_network_requirements = turn
            .config
            .config_layer_stack
            .requirements_toml()
            .network
            .is_some();
        let sandbox_type = sandbox.select_initial(
            &turn.sandbox_policy,
            SandboxablePreference::Auto,
            turn.windows_sandbox_level,
            has_managed_network_requirements,
        );
        let exec_env = sandbox
            .transform(crate::sandboxing::SandboxTransformRequest {
                spec,
                policy: &turn.sandbox_policy,
                sandbox: sandbox_type,
                enforce_managed_network: has_managed_network_requirements,
                network: None,
                sandbox_policy_cwd: &turn.cwd,
                #[cfg(target_os = "macos")]
                macos_seatbelt_profile_extensions: None,
                codex_linux_sandbox_exe: turn.codex_linux_sandbox_exe.as_ref(),
                use_linux_sandbox_bwrap: turn
                    .features
                    .enabled(crate::features::Feature::UseLinuxSandboxBwrap),
                windows_sandbox_level: turn.windows_sandbox_level,
            })
            .map_err(|err| format!("failed to configure sandbox for ps_repl: {err}"))?;

        let mut cmd =
            tokio::process::Command::new(exec_env.command.first().cloned().unwrap_or_default());
        if exec_env.command.len() > 1 {
            cmd.args(&exec_env.command[1..]);
        }
        #[cfg(unix)]
        cmd.arg0(
            exec_env
                .arg0
                .clone()
                .unwrap_or_else(|| exec_env.command.first().cloned().unwrap_or_default()),
        );
        cmd.current_dir(&exec_env.cwd);
        cmd.env_clear();
        cmd.envs(exec_env.env);
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|err| format!("failed to start PowerShell runtime: {err}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "ps_repl kernel missing stdout".to_string())?;
        let stderr = child.stderr.take();
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ps_repl kernel missing stdin".to_string())?;

        let shutdown = CancellationToken::new();
        let pending_execs: Arc<
            Mutex<HashMap<String, tokio::sync::oneshot::Sender<ExecResultMessage>>>,
        > = Arc::new(Mutex::new(HashMap::new()));
        let exec_contexts: Arc<Mutex<HashMap<String, ExecContext>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let stdin_arc = Arc::new(Mutex::new(stdin));
        let child = Arc::new(Mutex::new(child));
        let recent_stderr = Arc::new(Mutex::new(VecDeque::with_capacity(
            PS_REPL_STDERR_TAIL_LINE_LIMIT,
        )));

        tokio::spawn(Self::read_stdout(
            stdout,
            Arc::clone(&child),
            Arc::clone(&self.kernel),
            Arc::clone(&recent_stderr),
            Arc::clone(&pending_execs),
            Arc::clone(&exec_contexts),
            Arc::clone(&self.exec_tool_calls),
            Arc::clone(&stdin_arc),
            shutdown.clone(),
        ));
        if let Some(stderr) = stderr {
            tokio::spawn(Self::read_stderr(
                stderr,
                Arc::clone(&recent_stderr),
                shutdown.clone(),
            ));
        } else {
            warn!("ps_repl kernel missing stderr");
        }

        Ok(KernelState {
            child,
            recent_stderr,
            stdin: stdin_arc,
            pending_execs,
            exec_contexts,
            shutdown,
        })
    }

    async fn write_kernel_script(&self) -> Result<PathBuf, std::io::Error> {
        let dir = self.tmp_dir.path();
        let kernel_path = dir.join("ps_repl_kernel.ps1");
        tokio::fs::write(&kernel_path, KERNEL_SOURCE).await?;
        Ok(kernel_path)
    }

    async fn write_message(
        stdin: &Arc<Mutex<ChildStdin>>,
        msg: &HostToKernel,
    ) -> Result<(), FunctionCallError> {
        let encoded = serde_json::to_string(msg).map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to serialize kernel message: {err}"))
        })?;
        let mut guard = stdin.lock().await;
        guard.write_all(encoded.as_bytes()).await.map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to write to kernel: {err}"))
        })?;
        guard.write_all(b"\n").await.map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to flush kernel message: {err}"))
        })?;
        Ok(())
    }

    async fn kernel_stderr_tail_snapshot(recent_stderr: &Arc<Mutex<VecDeque<String>>>) -> String {
        let tail = recent_stderr.lock().await;
        format_stderr_tail(&tail)
    }

    async fn kernel_debug_snapshot(
        child: &Arc<Mutex<Child>>,
        recent_stderr: &Arc<Mutex<VecDeque<String>>>,
    ) -> KernelDebugSnapshot {
        let (pid, status) = {
            let mut guard = child.lock().await;
            let pid = guard.id();
            let status = match guard.try_wait() {
                Ok(Some(status)) => format!("exited({})", format_exit_status(status)),
                Ok(None) => "running".to_string(),
                Err(err) => format!("unknown ({err})"),
            };
            (pid, status)
        };
        let stderr_tail = {
            let tail = recent_stderr.lock().await;
            format_stderr_tail(&tail)
        };
        KernelDebugSnapshot {
            pid,
            status,
            stderr_tail,
        }
    }

    async fn kill_kernel_child(child: &Arc<Mutex<Child>>, reason: &'static str) {
        let mut guard = child.lock().await;
        let pid = guard.id();
        match guard.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => {}
            Err(err) => {
                warn!(
                    kernel_pid = ?pid,
                    kill_reason = reason,
                    error = %err,
                    "failed to inspect ps_repl kernel before kill"
                );
            }
        }

        if let Err(err) = guard.start_kill() {
            warn!(
                kernel_pid = ?pid,
                kill_reason = reason,
                error = %err,
                "failed to send kill signal to ps_repl kernel"
            );
            return;
        }

        match tokio::time::timeout(Duration::from_secs(2), guard.wait()).await {
            Ok(Ok(_status)) => {}
            Ok(Err(err)) => {
                warn!(
                    kernel_pid = ?pid,
                    kill_reason = reason,
                    error = %err,
                    "failed while waiting for ps_repl kernel exit"
                );
            }
            Err(_) => {
                warn!(
                    kernel_pid = ?pid,
                    kill_reason = reason,
                    "timed out waiting for ps_repl kernel to exit after kill"
                );
            }
        }
    }

    fn truncate_id_list(ids: &[String]) -> Vec<String> {
        if ids.len() <= PS_REPL_EXEC_ID_LOG_LIMIT {
            return ids.to_vec();
        }
        let mut output = ids[..PS_REPL_EXEC_ID_LOG_LIMIT].to_vec();
        output.push(format!("...+{}", ids.len() - PS_REPL_EXEC_ID_LOG_LIMIT));
        output
    }

    #[allow(clippy::too_many_arguments)]
    async fn read_stdout(
        stdout: tokio::process::ChildStdout,
        child: Arc<Mutex<Child>>,
        manager_kernel: Arc<Mutex<Option<KernelState>>>,
        recent_stderr: Arc<Mutex<VecDeque<String>>>,
        pending_execs: Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<ExecResultMessage>>>>,
        exec_contexts: Arc<Mutex<HashMap<String, ExecContext>>>,
        exec_tool_calls: Arc<Mutex<HashMap<String, ExecToolCalls>>>,
        stdin: Arc<Mutex<ChildStdin>>,
        shutdown: CancellationToken,
    ) {
        let mut reader = BufReader::new(stdout).lines();
        let end_reason = loop {
            let line = tokio::select! {
                _ = shutdown.cancelled() => break KernelStreamEnd::Shutdown,
                res = reader.next_line() => match res {
                    Ok(Some(line)) => line,
                    Ok(None) => break KernelStreamEnd::StdoutEof,
                    Err(err) => break KernelStreamEnd::StdoutReadError(err.to_string()),
                },
            };

            let parsed: Result<KernelToHost, _> = serde_json::from_str(&line);
            let msg = match parsed {
                Ok(m) => m,
                Err(err) => {
                    warn!("ps_repl kernel sent invalid json: {err} (line: {line})");
                    continue;
                }
            };

            match msg {
                KernelToHost::ExecResult {
                    id,
                    ok,
                    output,
                    error,
                } => {
                    PsReplManager::wait_for_exec_tool_calls_map(&exec_tool_calls, &id).await;
                    let content_items = {
                        let calls = exec_tool_calls.lock().await;
                        calls
                            .get(&id)
                            .map(|state| state.content_items.clone())
                            .unwrap_or_default()
                    };
                    let mut pending = pending_execs.lock().await;
                    if let Some(tx) = pending.remove(&id) {
                        let payload = if ok {
                            ExecResultMessage::Ok {
                                content_items: build_exec_result_content_items(
                                    output,
                                    content_items,
                                ),
                            }
                        } else {
                            ExecResultMessage::Err {
                                message: error
                                    .unwrap_or_else(|| "ps_repl execution failed".to_string()),
                            }
                        };
                        let _ = tx.send(payload);
                    }
                    exec_contexts.lock().await.remove(&id);
                    PsReplManager::clear_exec_tool_calls_map(&exec_tool_calls, &id).await;
                }
                KernelToHost::RunTool(req) => {
                    let Some(reset_cancel) =
                        PsReplManager::begin_exec_tool_call(&exec_tool_calls, &req.exec_id).await
                    else {
                        let exec_id = req.exec_id.clone();
                        let tool_call_id = req.id.clone();
                        let payload = HostToKernel::RunToolResult(RunToolResult {
                            id: req.id,
                            ok: false,
                            response: None,
                            error: Some("ps_repl exec context not found".to_string()),
                        });
                        if let Err(err) = PsReplManager::write_message(&stdin, &payload).await {
                            let snapshot =
                                PsReplManager::kernel_debug_snapshot(&child, &recent_stderr).await;
                            warn!(
                                exec_id = %exec_id,
                                tool_call_id = %tool_call_id,
                                error = %err,
                                kernel_pid = ?snapshot.pid,
                                kernel_status = %snapshot.status,
                                kernel_stderr_tail = %snapshot.stderr_tail,
                                "failed to reply to kernel run_tool request"
                            );
                        }
                        continue;
                    };
                    let stdin_clone = Arc::clone(&stdin);
                    let exec_contexts = Arc::clone(&exec_contexts);
                    let exec_tool_calls_for_task = Arc::clone(&exec_tool_calls);
                    let recent_stderr = Arc::clone(&recent_stderr);
                    tokio::spawn(async move {
                        let exec_id = req.exec_id.clone();
                        let tool_call_id = req.id.clone();
                        let tool_name = req.tool_name.clone();
                        let context = { exec_contexts.lock().await.get(&exec_id).cloned() };
                        let result = match context {
                            Some(ctx) => {
                                tokio::select! {
                                    _ = reset_cancel.cancelled() => RunToolResult {
                                        id: tool_call_id.clone(),
                                        ok: false,
                                        response: None,
                                        error: Some("ps_repl execution reset".to_string()),
                                    },
                                    result = PsReplManager::run_tool_request(
                                        ctx,
                                        req,
                                        Arc::clone(&exec_tool_calls_for_task),
                                    ) => result,
                                }
                            }
                            None => RunToolResult {
                                id: tool_call_id.clone(),
                                ok: false,
                                response: None,
                                error: Some("ps_repl exec context not found".to_string()),
                            },
                        };
                        PsReplManager::finish_exec_tool_call(&exec_tool_calls_for_task, &exec_id)
                            .await;
                        let payload = HostToKernel::RunToolResult(result);
                        if let Err(err) = PsReplManager::write_message(&stdin_clone, &payload).await
                        {
                            let stderr_tail =
                                PsReplManager::kernel_stderr_tail_snapshot(&recent_stderr).await;
                            warn!(
                                exec_id = %exec_id,
                                tool_call_id = %tool_call_id,
                                tool_name = %tool_name,
                                error = %err,
                                kernel_stderr_tail = %stderr_tail,
                                "failed to reply to kernel run_tool request"
                            );
                        }
                    });
                }
            }
        };

        let exec_ids = {
            let mut contexts = exec_contexts.lock().await;
            let ids = contexts.keys().cloned().collect::<Vec<_>>();
            contexts.clear();
            ids
        };
        for exec_id in exec_ids {
            PsReplManager::wait_for_exec_tool_calls_map(&exec_tool_calls, &exec_id).await;
            PsReplManager::clear_exec_tool_calls_map(&exec_tool_calls, &exec_id).await;
        }
        let unexpected_snapshot = if matches!(end_reason, KernelStreamEnd::Shutdown) {
            None
        } else {
            Some(Self::kernel_debug_snapshot(&child, &recent_stderr).await)
        };
        let kernel_failure_message = unexpected_snapshot.as_ref().map(|snapshot| {
            with_model_kernel_failure_message(
                "ps_repl kernel exited unexpectedly",
                end_reason.reason(),
                end_reason.error(),
                snapshot,
            )
        });
        let kernel_exit_message = kernel_failure_message
            .clone()
            .unwrap_or_else(|| "ps_repl kernel exited unexpectedly".to_string());

        {
            let mut kernel = manager_kernel.lock().await;
            let should_clear = kernel
                .as_ref()
                .is_some_and(|state| Arc::ptr_eq(&state.child, &child));
            if should_clear {
                kernel.take();
            }
        }

        let mut pending = pending_execs.lock().await;
        let pending_exec_ids = pending.keys().cloned().collect::<Vec<_>>();
        for (_id, tx) in pending.drain() {
            let _ = tx.send(ExecResultMessage::Err {
                message: kernel_exit_message.clone(),
            });
        }
        drop(pending);

        if !matches!(end_reason, KernelStreamEnd::Shutdown) {
            let mut pending_exec_ids = pending_exec_ids;
            pending_exec_ids.sort_unstable();
            let snapshot = Self::kernel_debug_snapshot(&child, &recent_stderr).await;
            warn!(
                reason = %end_reason.reason(),
                stream_error = %end_reason.error().unwrap_or(""),
                kernel_pid = ?snapshot.pid,
                kernel_status = %snapshot.status,
                pending_exec_count = pending_exec_ids.len(),
                pending_exec_ids = ?Self::truncate_id_list(&pending_exec_ids),
                kernel_stderr_tail = %snapshot.stderr_tail,
                "ps_repl kernel terminated unexpectedly"
            );
        }
    }

    async fn run_tool_request(
        exec: ExecContext,
        req: RunToolRequest,
        exec_tool_calls: Arc<Mutex<HashMap<String, ExecToolCalls>>>,
    ) -> RunToolResult {
        if is_ps_repl_internal_tool(&req.tool_name) {
            let error = "ps_repl cannot invoke itself".to_string();
            let summary = Self::summarize_tool_call_error(&error);
            Self::log_tool_call_response(&req, false, &summary, None, Some(&error));
            return RunToolResult {
                id: req.id,
                ok: false,
                response: None,
                error: Some(error),
            };
        }

        let mcp_tools = exec
            .session
            .services
            .mcp_connection_manager
            .read()
            .await
            .list_all_tools()
            .await;

        let router = ToolRouter::from_config(
            &exec.turn.tools_config,
            Some(
                mcp_tools
                    .into_iter()
                    .map(|(name, tool)| (name, tool.tool))
                    .collect(),
            ),
            None,
            exec.turn.dynamic_tools.as_slice(),
        );

        let payload =
            if let Some((server, tool)) = exec.session.parse_mcp_tool_name(&req.tool_name).await {
                crate::tools::context::ToolPayload::Mcp {
                    server,
                    tool,
                    raw_arguments: req.arguments.clone(),
                }
            } else if is_freeform_tool(&router.specs(), &req.tool_name) {
                crate::tools::context::ToolPayload::Custom {
                    input: req.arguments.clone(),
                }
            } else {
                crate::tools::context::ToolPayload::Function {
                    arguments: req.arguments.clone(),
                }
            };

        let tool_name = req.tool_name.clone();
        let call = crate::tools::router::ToolCall {
            tool_name: tool_name.clone(),
            call_id: req.id.clone(),
            payload,
        };

        let session = Arc::clone(&exec.session);
        let turn = Arc::clone(&exec.turn);
        let tracker = Arc::clone(&exec.tracker);

        match router
            .dispatch_tool_call(
                session.clone(),
                turn,
                tracker,
                call,
                crate::tools::router::ToolCallSource::PsRepl,
            )
            .await
        {
            Ok(response) => {
                if let Some(items) = response_content_items(&response) {
                    Self::record_exec_tool_call_content_items(
                        &exec_tool_calls,
                        &req.exec_id,
                        items,
                    )
                    .await;
                }

                let summary = Self::summarize_tool_call_response(&response);
                match serde_json::to_value(response) {
                    Ok(value) => {
                        Self::log_tool_call_response(&req, true, &summary, Some(&value), None);
                        RunToolResult {
                            id: req.id,
                            ok: true,
                            response: Some(value),
                            error: None,
                        }
                    }
                    Err(err) => {
                        let error = format!("failed to serialize tool output: {err}");
                        let summary = Self::summarize_tool_call_error(&error);
                        Self::log_tool_call_response(&req, false, &summary, None, Some(&error));
                        RunToolResult {
                            id: req.id,
                            ok: false,
                            response: None,
                            error: Some(error),
                        }
                    }
                }
            }
            Err(err) => {
                let error = err.to_string();
                let summary = Self::summarize_tool_call_error(&error);
                Self::log_tool_call_response(&req, false, &summary, None, Some(&error));
                RunToolResult {
                    id: req.id,
                    ok: false,
                    response: None,
                    error: Some(error),
                }
            }
        }
    }

    async fn read_stderr(
        stderr: tokio::process::ChildStderr,
        recent_stderr: Arc<Mutex<VecDeque<String>>>,
        shutdown: CancellationToken,
    ) {
        let mut reader = BufReader::new(stderr).lines();

        loop {
            let line = tokio::select! {
                _ = shutdown.cancelled() => break,
                res = reader.next_line() => match res {
                    Ok(Some(line)) => line,
                    Ok(None) => break,
                    Err(err) => {
                        warn!("ps_repl kernel stderr ended: {err}");
                        break;
                    }
                },
            };
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let bounded_line = {
                    let mut tail = recent_stderr.lock().await;
                    push_stderr_tail_line(&mut tail, trimmed)
                };
                if bounded_line.is_empty() {
                    continue;
                }
                warn!("ps_repl stderr: {bounded_line}");
            }
        }
    }
}

fn response_content_items(
    response: &ResponseInputItem,
) -> Option<Vec<FunctionCallOutputContentItem>> {
    match response {
        ResponseInputItem::FunctionCallOutput { output, .. }
        | ResponseInputItem::CustomToolCallOutput { output, .. } => output
            .content_items()
            .map(<[FunctionCallOutputContentItem]>::to_vec),
        ResponseInputItem::McpToolCallOutput { result, .. } => match result {
            Ok(result) => FunctionCallOutputPayload::from(result)
                .content_items()
                .map(<[FunctionCallOutputContentItem]>::to_vec),
            Err(_) => None,
        },
        ResponseInputItem::Message { .. } => None,
    }
}

fn build_exec_result_content_items(
    output: String,
    content_items: Vec<FunctionCallOutputContentItem>,
) -> Vec<FunctionCallOutputContentItem> {
    let mut all_content_items = Vec::with_capacity(content_items.len() + 1);
    all_content_items.push(FunctionCallOutputContentItem::InputText { text: output });
    all_content_items.extend(content_items);
    all_content_items
}

fn split_exec_result_content_items(
    mut content_items: Vec<FunctionCallOutputContentItem>,
) -> (String, Vec<FunctionCallOutputContentItem>) {
    match content_items.first() {
        Some(FunctionCallOutputContentItem::InputText { .. }) => {
            let FunctionCallOutputContentItem::InputText { text } = content_items.remove(0) else {
                unreachable!("first content item should be input_text");
            };
            (text, content_items)
        }
        Some(FunctionCallOutputContentItem::InputImage { .. }) | None => {
            (String::new(), content_items)
        }
    }
}

fn is_freeform_tool(specs: &[ToolSpec], name: &str) -> bool {
    specs
        .iter()
        .any(|spec| spec.name() == name && matches!(spec, ToolSpec::Freeform(_)))
}

fn is_ps_repl_internal_tool(name: &str) -> bool {
    matches!(name, "ps_repl" | "ps_repl_reset")
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum KernelToHost {
    ExecResult {
        id: String,
        ok: bool,
        output: String,
        #[serde(default)]
        error: Option<String>,
    },
    RunTool(RunToolRequest),
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum HostToKernel {
    Exec {
        id: String,
        code: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    RunToolResult(RunToolResult),
}

#[derive(Clone, Debug, Deserialize)]
struct RunToolRequest {
    id: String,
    exec_id: String,
    tool_name: String,
    arguments: String,
}

#[derive(Clone, Debug, Serialize)]
struct RunToolResult {
    id: String,
    ok: bool,
    #[serde(default)]
    response: Option<JsonValue>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug)]
enum ExecResultMessage {
    Ok {
        content_items: Vec<FunctionCallOutputContentItem>,
    },
    Err {
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PwshVersion {
    major: u64,
    minor: u64,
    patch: u64,
}

impl fmt::Display for PwshVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl PwshVersion {
    fn parse(input: &str) -> Result<Self, String> {
        let trimmed = input
            .trim()
            .trim_start_matches("PowerShell")
            .trim()
            .trim_start_matches('v');
        let mut parts = trimmed.split(['.', '-', '+']);
        let major = parts
            .next()
            .ok_or_else(|| "missing major version".to_string())?
            .parse::<u64>()
            .map_err(|err| format!("invalid major version: {err}"))?;
        let minor = parts
            .next()
            .ok_or_else(|| "missing minor version".to_string())?
            .parse::<u64>()
            .map_err(|err| format!("invalid minor version: {err}"))?;
        let patch = parts
            .next()
            .ok_or_else(|| "missing patch version".to_string())?
            .parse::<u64>()
            .map_err(|err| format!("invalid patch version: {err}"))?;
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

async fn read_pwsh_version(pwsh_path: &Path) -> Result<PwshVersion, String> {
    let output = tokio::process::Command::new(pwsh_path)
        .arg("--version")
        .output()
        .await
        .map_err(|err| format!("failed to execute pwsh: {err}"))?;

    if !output.status.success() {
        let mut details = String::new();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = stdout.trim();
        let stderr = stderr.trim();
        if !stdout.is_empty() {
            details.push_str(" stdout: ");
            details.push_str(stdout);
        }
        if !stderr.is_empty() {
            details.push_str(" stderr: ");
            details.push_str(stderr);
        }
        let details = if details.is_empty() {
            String::new()
        } else {
            format!(" ({details})")
        };
        return Err(format!(
            "failed to read pwsh version (status {status}){details}",
            status = output.status
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout = stdout.trim();
    PwshVersion::parse(stdout)
        .map_err(|err| format!("failed to parse pwsh version output `{stdout}`: {err}"))
}

async fn ensure_pwsh_version(pwsh_path: &Path) -> Result<(), String> {
    let found = read_pwsh_version(pwsh_path).await?;
    if found.major < PS_REPL_MIN_PWSH_MAJOR {
        return Err(format!(
            "PowerShell runtime too old for ps_repl (resolved {pwsh_path}): found v{found}, requires >= v{required}. Install/update PowerShell or set ps_repl_path to a newer runtime.",
            pwsh_path = pwsh_path.display(),
            required = PS_REPL_MIN_PWSH_MAJOR,
        ));
    }
    Ok(())
}

pub(crate) async fn resolve_compatible_pwsh(config_path: Option<&Path>) -> Result<PathBuf, String> {
    let pwsh_path = resolve_pwsh(config_path).ok_or_else(|| {
        "PowerShell 7 runtime not found; install pwsh or set CODEX_PS_REPL_PATH".to_string()
    })?;
    ensure_pwsh_version(&pwsh_path).await?;
    Ok(pwsh_path)
}

pub(crate) fn resolve_pwsh(config_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CODEX_PS_REPL_PATH") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    if let Some(path) = config_path
        && path.exists()
    {
        return Some(path.to_path_buf());
    }

    #[cfg(windows)]
    if let Some(path) = crate::powershell::try_find_pwsh_executable_blocking() {
        return Some(path.as_path().to_path_buf());
    }

    if let Ok(path) = which::which("pwsh") {
        return Some(path);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn pwsh_version_parses_leading_label() {
        let version = PwshVersion::parse("PowerShell 7.5.4").unwrap();
        assert_eq!(
            version,
            PwshVersion {
                major: 7,
                minor: 5,
                patch: 4,
            }
        );
    }

    #[test]
    fn pwsh_version_parses_v_prefix_and_suffix() {
        let version = PwshVersion::parse("v7.5.4-preview.1").unwrap();
        assert_eq!(
            version,
            PwshVersion {
                major: 7,
                minor: 5,
                patch: 4,
            }
        );
    }

    #[test]
    fn ps_repl_internal_tool_guard_matches_expected_names() {
        assert!(is_ps_repl_internal_tool("ps_repl"));
        assert!(is_ps_repl_internal_tool("ps_repl_reset"));
        assert!(!is_ps_repl_internal_tool("shell_command"));
        assert!(!is_ps_repl_internal_tool("list_mcp_resources"));
    }
}
