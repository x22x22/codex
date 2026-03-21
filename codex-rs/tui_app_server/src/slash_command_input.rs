use crate::slash_command::SlashCommand;
use crate::slash_command::SlashCommandBareBehavior;
use crate::slash_command::SlashCommandParseKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FastSlashCommandArgs {
    On,
    Off,
    Status,
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
    pub(crate) fn parse_invocation(
        self,
        args: &str,
    ) -> Result<ParsedSlashCommand, SlashCommandUsageError> {
        let trimmed = args.trim();
        let spec = self.spec();
        if trimmed.is_empty() {
            return Ok(ParsedSlashCommand::Bare(spec.bare_behavior));
        }

        match spec.parse_kind {
            SlashCommandParseKind::NoArgs => Err(SlashCommandUsageError {
                command: self,
                kind: SlashCommandUsageErrorKind::UnexpectedInlineArgs,
            }),
            SlashCommandParseKind::Fast => match trimmed.to_ascii_lowercase().as_str() {
                "on" => Ok(ParsedSlashCommand::Fast(FastSlashCommandArgs::On)),
                "off" => Ok(ParsedSlashCommand::Fast(FastSlashCommandArgs::Off)),
                "status" => Ok(ParsedSlashCommand::Fast(FastSlashCommandArgs::Status)),
                _ => Err(SlashCommandUsageError {
                    command: self,
                    kind: SlashCommandUsageErrorKind::InvalidInlineArgs,
                }),
            },
            SlashCommandParseKind::Rename => Ok(ParsedSlashCommand::Rename),
            SlashCommandParseKind::Plan => Ok(ParsedSlashCommand::Plan),
            SlashCommandParseKind::Review => Ok(ParsedSlashCommand::Review),
            SlashCommandParseKind::SandboxReadRoot => Ok(ParsedSlashCommand::SandboxReadRoot),
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
