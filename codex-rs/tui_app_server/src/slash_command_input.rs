use crate::slash_command::ParsedSlashCommand;
use crate::slash_command::SlashCommand;
use crate::slash_command::SlashCommandUsageErrorKind;

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

        spec.args_parser
            .parse(trimmed)
            .map_err(|kind| SlashCommandUsageError {
                command: self,
                kind,
            })
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::slash_command::FastSlashCommandArgs;
    use crate::slash_command::SlashCommandBareBehavior;

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
