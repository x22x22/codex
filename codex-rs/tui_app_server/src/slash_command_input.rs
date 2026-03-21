use crate::slash_command::SlashCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FastSlashCommandArgs {
    On,
    Off,
    Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlashCommandBareBehavior {
    DispatchesDirectly,
    OpensUi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParsedSlashCommand {
    Bare(SlashCommandBareBehavior),
    Fast(FastSlashCommandArgs),
    Rename,
    Plan,
    Review,
    SandboxReadRoot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlashCommandUsageErrorKind {
    UnexpectedInlineArgs,
    InvalidInlineArgs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SlashCommandUsageError {
    command: SlashCommand,
    kind: SlashCommandUsageErrorKind,
}

impl SlashCommandUsageError {
    pub(crate) fn message(self) -> String {
        let usage = self.command.usage_lines().join(" | ");
        match self.kind {
            SlashCommandUsageErrorKind::UnexpectedInlineArgs => format!(
                "'/{}' does not accept inline arguments. Usage: {usage}",
                self.command.command()
            ),
            SlashCommandUsageErrorKind::InvalidInlineArgs => format!("Usage: {usage}"),
        }
    }
}

impl SlashCommand {
    pub(crate) fn bare_behavior(self) -> SlashCommandBareBehavior {
        match self {
            SlashCommand::Model
            | SlashCommand::Approvals
            | SlashCommand::Permissions
            | SlashCommand::Experimental
            | SlashCommand::Skills
            | SlashCommand::Review
            | SlashCommand::Rename
            | SlashCommand::Resume
            | SlashCommand::Collab
            | SlashCommand::Agent
            | SlashCommand::Statusline
            | SlashCommand::Theme
            | SlashCommand::Feedback
            | SlashCommand::Personality
            | SlashCommand::Settings
            | SlashCommand::MultiAgents => SlashCommandBareBehavior::OpensUi,
            SlashCommand::Fast
            | SlashCommand::ElevateSandbox
            | SlashCommand::SandboxReadRoot
            | SlashCommand::New
            | SlashCommand::Fork
            | SlashCommand::Init
            | SlashCommand::Compact
            | SlashCommand::Plan
            | SlashCommand::Diff
            | SlashCommand::Copy
            | SlashCommand::Mention
            | SlashCommand::Status
            | SlashCommand::DebugConfig
            | SlashCommand::Mcp
            | SlashCommand::Apps
            | SlashCommand::Logout
            | SlashCommand::Quit
            | SlashCommand::Exit
            | SlashCommand::Rollout
            | SlashCommand::Ps
            | SlashCommand::Stop
            | SlashCommand::Clear
            | SlashCommand::Realtime
            | SlashCommand::TestApproval
            | SlashCommand::MemoryDrop
            | SlashCommand::MemoryUpdate => SlashCommandBareBehavior::DispatchesDirectly,
        }
    }

    pub(crate) fn parse_invocation(
        self,
        args: &str,
    ) -> Result<ParsedSlashCommand, SlashCommandUsageError> {
        let trimmed = args.trim();
        if trimmed.is_empty() {
            return Ok(ParsedSlashCommand::Bare(self.bare_behavior()));
        }

        match self {
            SlashCommand::Fast => match trimmed.to_ascii_lowercase().as_str() {
                "on" => Ok(ParsedSlashCommand::Fast(FastSlashCommandArgs::On)),
                "off" => Ok(ParsedSlashCommand::Fast(FastSlashCommandArgs::Off)),
                "status" => Ok(ParsedSlashCommand::Fast(FastSlashCommandArgs::Status)),
                _ => Err(SlashCommandUsageError {
                    command: self,
                    kind: SlashCommandUsageErrorKind::InvalidInlineArgs,
                }),
            },
            SlashCommand::Rename => Ok(ParsedSlashCommand::Rename),
            SlashCommand::Plan => Ok(ParsedSlashCommand::Plan),
            SlashCommand::Review => Ok(ParsedSlashCommand::Review),
            SlashCommand::SandboxReadRoot => Ok(ParsedSlashCommand::SandboxReadRoot),
            SlashCommand::Model
            | SlashCommand::Approvals
            | SlashCommand::Permissions
            | SlashCommand::ElevateSandbox
            | SlashCommand::Experimental
            | SlashCommand::Skills
            | SlashCommand::New
            | SlashCommand::Resume
            | SlashCommand::Fork
            | SlashCommand::Init
            | SlashCommand::Compact
            | SlashCommand::Collab
            | SlashCommand::Agent
            | SlashCommand::Diff
            | SlashCommand::Copy
            | SlashCommand::Mention
            | SlashCommand::Status
            | SlashCommand::DebugConfig
            | SlashCommand::Statusline
            | SlashCommand::Theme
            | SlashCommand::Mcp
            | SlashCommand::Apps
            | SlashCommand::Logout
            | SlashCommand::Quit
            | SlashCommand::Exit
            | SlashCommand::Feedback
            | SlashCommand::Rollout
            | SlashCommand::Ps
            | SlashCommand::Stop
            | SlashCommand::Clear
            | SlashCommand::Personality
            | SlashCommand::Realtime
            | SlashCommand::Settings
            | SlashCommand::TestApproval
            | SlashCommand::MultiAgents
            | SlashCommand::MemoryDrop
            | SlashCommand::MemoryUpdate => Err(SlashCommandUsageError {
                command: self,
                kind: SlashCommandUsageErrorKind::UnexpectedInlineArgs,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn review_bare_form_is_marked_as_ui_driven() {
        assert_eq!(
            SlashCommand::Review.parse_invocation(""),
            Ok(ParsedSlashCommand::Bare(SlashCommandBareBehavior::OpensUi))
        );
    }

    #[test]
    fn fast_accepts_nonempty_inline_args() {
        assert_eq!(
            SlashCommand::Fast.parse_invocation("status"),
            Ok(ParsedSlashCommand::Fast(FastSlashCommandArgs::Status))
        );
    }

    #[test]
    fn clear_rejects_unexpected_inline_args() {
        assert_eq!(
            SlashCommand::Clear
                .parse_invocation("now")
                .unwrap_err()
                .message(),
            "'/clear' does not accept inline arguments. Usage: /clear"
        );
    }
}
