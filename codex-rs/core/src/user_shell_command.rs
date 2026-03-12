use std::time::Duration;

use codex_protocol::models::ResponseItem;

use crate::codex::TurnContext;
use crate::exec::ExecToolCallOutput;
use crate::model_visible_context::ContextualUserContextRole;
use crate::model_visible_context::ContextualUserFragmentMarkers;
use crate::model_visible_context::ModelVisibleContextFragment;
use crate::model_visible_context::TaggedContextualUserFragment;
use crate::model_visible_context::USER_SHELL_COMMAND_CLOSE_TAG;
use crate::model_visible_context::USER_SHELL_COMMAND_OPEN_TAG;
use crate::tools::format_exec_output_str;

fn format_duration_line(duration: Duration) -> String {
    let duration_seconds = duration.as_secs_f64();
    format!("Duration: {duration_seconds:.4} seconds")
}

pub(crate) struct UserShellCommandFragment;

impl TaggedContextualUserFragment for UserShellCommandFragment {
    const MARKERS: ContextualUserFragmentMarkers = ContextualUserFragmentMarkers::new(
        USER_SHELL_COMMAND_OPEN_TAG,
        USER_SHELL_COMMAND_CLOSE_TAG,
    );
}

struct UserShellCommandRecord<'a> {
    command: &'a str,
    exec_output: &'a ExecToolCallOutput,
    turn_context: &'a TurnContext,
}

impl ModelVisibleContextFragment for UserShellCommandRecord<'_> {
    type Role = ContextualUserContextRole;

    fn render_text(&self) -> String {
        let mut sections = Vec::new();
        sections.push("<command>".to_string());
        sections.push(self.command.to_string());
        sections.push("</command>".to_string());
        sections.push("<result>".to_string());
        sections.push(format!("Exit code: {}", self.exec_output.exit_code));
        sections.push(format_duration_line(self.exec_output.duration));
        sections.push("Output:".to_string());
        sections.push(format_exec_output_str(
            self.exec_output,
            self.turn_context.truncation_policy,
        ));
        sections.push("</result>".to_string());
        UserShellCommandFragment::wrap_contextual_user_body(sections.join("\n"))
    }
}

#[cfg(test)]
pub fn format_user_shell_command_record(
    command: &str,
    exec_output: &ExecToolCallOutput,
    turn_context: &TurnContext,
) -> String {
    UserShellCommandRecord {
        command,
        exec_output,
        turn_context,
    }
    .render_text()
}

pub fn user_shell_command_record_item(
    command: &str,
    exec_output: &ExecToolCallOutput,
    turn_context: &TurnContext,
) -> ResponseItem {
    UserShellCommandRecord {
        command,
        exec_output,
        turn_context,
    }
    .into_message()
}

#[cfg(test)]
#[path = "user_shell_command_tests.rs"]
mod tests;
