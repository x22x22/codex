pub mod code_mode;
pub(crate) mod code_mode_description;
pub mod context;
pub(crate) mod discoverable;
pub mod events;
pub(crate) mod handlers;
pub mod js_repl;
pub(crate) mod network_approval;
pub mod orchestrator;
pub mod parallel;
pub mod registry;
pub mod router;
pub mod runtimes;
pub mod sandboxing;
pub mod spec;

use crate::exec::ExecToolCallOutput;
use crate::truncate::TruncationPolicy;
use crate::truncate::formatted_truncate_text;
use crate::truncate::truncate_text;
use codex_shell_command::parse_command::shlex_join;
pub use router::ToolRouter;
use serde::Serialize;

// Telemetry preview limits: keep log events smaller than model budgets.
pub(crate) const TELEMETRY_PREVIEW_MAX_BYTES: usize = 2 * 1024; // 2 KiB
pub(crate) const TELEMETRY_PREVIEW_MAX_LINES: usize = 64; // lines
pub(crate) const TELEMETRY_PREVIEW_TRUNCATION_NOTICE: &str =
    "[... telemetry preview truncated ...]";

/// Format the combined exec output for sending back to the model.
/// Includes exit code and duration metadata; truncates large bodies safely.
pub fn format_exec_output_for_model_structured(
    exec_output: &ExecToolCallOutput,
    truncation_policy: TruncationPolicy,
    executed_command: Option<&[String]>,
) -> String {
    let ExecToolCallOutput {
        exit_code,
        duration,
        ..
    } = exec_output;

    #[derive(Serialize)]
    struct ExecMetadata {
        exit_code: i32,
        duration_seconds: f32,
    }

    #[derive(Serialize)]
    struct ExecOutput<'a> {
        output: &'a str,
        metadata: ExecMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        executed_command: Option<&'a [String]>,
    }

    // round to 1 decimal place
    let duration_seconds = ((duration.as_secs_f32()) * 10.0).round() / 10.0;

    let formatted_output = format_exec_output_str(exec_output, truncation_policy);

    let payload = ExecOutput {
        output: &formatted_output,
        metadata: ExecMetadata {
            exit_code: *exit_code,
            duration_seconds,
        },
        executed_command,
    };

    #[expect(clippy::expect_used)]
    serde_json::to_string(&payload).expect("serialize ExecOutput")
}

pub fn format_exec_output_for_model_freeform(
    exec_output: &ExecToolCallOutput,
    truncation_policy: TruncationPolicy,
    executed_command: Option<&[String]>,
) -> String {
    // round to 1 decimal place
    let duration_seconds = ((exec_output.duration.as_secs_f32()) * 10.0).round() / 10.0;

    let content = build_content_with_timeout(exec_output);

    let total_lines = content.lines().count();

    let formatted_output = truncate_text(&content, truncation_policy);

    let mut sections = Vec::new();

    if let Some(command) = executed_command {
        sections.push(format!("Executed command: {}", shlex_join(command)));
    }
    sections.push(format!("Exit code: {}", exec_output.exit_code));
    sections.push(format!("Wall time: {duration_seconds} seconds"));
    if total_lines != formatted_output.lines().count() {
        sections.push(format!("Total output lines: {total_lines}"));
    }

    sections.push("Output:".to_string());
    sections.push(formatted_output);

    sections.join("\n")
}

pub fn format_exec_output_str(
    exec_output: &ExecToolCallOutput,
    truncation_policy: TruncationPolicy,
) -> String {
    let content = build_content_with_timeout(exec_output);

    // Truncate for model consumption before serialization.
    formatted_truncate_text(&content, truncation_policy)
}

/// Extracts exec output content and prepends a timeout message if the command timed out.
fn build_content_with_timeout(exec_output: &ExecToolCallOutput) -> String {
    if exec_output.timed_out {
        format!(
            "command timed out after {} milliseconds\n{}",
            exec_output.duration.as_millis(),
            exec_output.aggregated_output.text
        )
    } else {
        exec_output.aggregated_output.text.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn format_exec_output_for_model_structured_includes_executed_command_when_present() {
        let exec_output = ExecToolCallOutput {
            aggregated_output: crate::exec::StreamOutput::new("override-ok\n".to_string()),
            duration: Duration::from_millis(100),
            ..ExecToolCallOutput::default()
        };

        let formatted = format_exec_output_for_model_structured(
            &exec_output,
            TruncationPolicy::Bytes(1024),
            Some(&["/bin/echo".to_string(), "override-ok".to_string()]),
        );

        assert_eq!(
            formatted,
            r#"{"output":"override-ok\n","metadata":{"exit_code":0,"duration_seconds":0.1},"executed_command":["/bin/echo","override-ok"]}"#
        );
    }

    #[test]
    fn format_exec_output_for_model_freeform_includes_executed_command_when_present() {
        let exec_output = ExecToolCallOutput {
            aggregated_output: crate::exec::StreamOutput::new("override-ok\n".to_string()),
            duration: Duration::from_millis(100),
            ..ExecToolCallOutput::default()
        };

        let formatted = format_exec_output_for_model_freeform(
            &exec_output,
            TruncationPolicy::Bytes(1024),
            Some(&["/bin/echo".to_string(), "override-ok".to_string()]),
        );

        assert_eq!(
            formatted,
            "Executed command: /bin/echo override-ok\nExit code: 0\nWall time: 0.1 seconds\nOutput:\noverride-ok\n"
        );
    }
}
