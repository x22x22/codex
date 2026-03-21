use strum::IntoEnumIterator;
use strum_macros::AsRefStr;
use strum_macros::EnumIter;
use strum_macros::EnumString;
use strum_macros::IntoStaticStr;

/// Commands that can be invoked by starting a message with a leading slash.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, EnumIter, AsRefStr, IntoStaticStr,
)]
#[strum(serialize_all = "kebab-case")]
pub enum SlashCommand {
    // DO NOT ALPHA-SORT! Enum order is presentation order in the popup, so
    // more frequently used commands should be listed first.
    Model,
    Fast,
    Approvals,
    Permissions,
    #[strum(serialize = "setup-default-sandbox")]
    ElevateSandbox,
    #[strum(serialize = "sandbox-add-read-dir")]
    SandboxReadRoot,
    Experimental,
    Skills,
    Review,
    Rename,
    New,
    Resume,
    Fork,
    Init,
    Compact,
    Plan,
    Collab,
    Agent,
    // Undo,
    Diff,
    Copy,
    Mention,
    Status,
    DebugConfig,
    Title,
    Statusline,
    Theme,
    Mcp,
    Apps,
    Plugins,
    Logout,
    Quit,
    Exit,
    Feedback,
    Rollout,
    Ps,
    #[strum(to_string = "stop", serialize = "clean")]
    Stop,
    Clear,
    Personality,
    Realtime,
    Settings,
    TestApproval,
    #[strum(serialize = "subagents")]
    MultiAgents,
    // Debugging commands.
    #[strum(serialize = "debug-m-drop")]
    MemoryDrop,
    #[strum(serialize = "debug-m-update")]
    MemoryUpdate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlashCommandBareBehavior {
    DispatchesDirectly,
    OpensUi,
}

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
pub(crate) enum SlashCommandArgsParser {
    NoArgs,
    Remainder(ParsedSlashCommand),
    Tokens(&'static [SlashCommandTokenChoice]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SlashCommandTokenChoice {
    pub(crate) token: &'static str,
    pub(crate) parsed: ParsedSlashCommand,
}

const FAST_TOKEN_CHOICES: &[SlashCommandTokenChoice] = &[
    SlashCommandTokenChoice {
        token: "on",
        parsed: ParsedSlashCommand::Fast(FastSlashCommandArgs::On),
    },
    SlashCommandTokenChoice {
        token: "off",
        parsed: ParsedSlashCommand::Fast(FastSlashCommandArgs::Off),
    },
    SlashCommandTokenChoice {
        token: "status",
        parsed: ParsedSlashCommand::Fast(FastSlashCommandArgs::Status),
    },
];

impl SlashCommandArgsParser {
    pub(crate) fn parse(
        self,
        args: &str,
    ) -> Result<ParsedSlashCommand, SlashCommandUsageErrorKind> {
        match self {
            SlashCommandArgsParser::NoArgs => Err(SlashCommandUsageErrorKind::UnexpectedInlineArgs),
            SlashCommandArgsParser::Remainder(parsed) => Ok(parsed),
            SlashCommandArgsParser::Tokens(choices) => choices
                .iter()
                .find(|choice| choice.token.eq_ignore_ascii_case(args))
                .map(|choice| choice.parsed)
                .ok_or(SlashCommandUsageErrorKind::InvalidInlineArgs),
        }
    }
}

impl SlashCommand {
    pub(crate) fn spec(self) -> SlashCommandSpec {
        match self {
            SlashCommand::Model => SlashCommandSpec {
                description: "choose what model and reasoning effort to use",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/model"],
                bare_behavior: SlashCommandBareBehavior::OpensUi,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Fast => SlashCommandSpec {
                description: "toggle Fast mode to enable fastest inference at 2X plan usage",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/fast", "/fast [on|off|status]"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::Tokens(FAST_TOKEN_CHOICES),
            },
            SlashCommand::Approvals => SlashCommandSpec {
                description: "choose what Codex is allowed to do",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: true,
                usage_lines: &["/approvals"],
                bare_behavior: SlashCommandBareBehavior::OpensUi,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Permissions => SlashCommandSpec {
                description: "choose what Codex is allowed to do",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/permissions"],
                bare_behavior: SlashCommandBareBehavior::OpensUi,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::ElevateSandbox => SlashCommandSpec {
                description: "set up elevated agent sandbox",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/setup-default-sandbox"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::SandboxReadRoot => SlashCommandSpec {
                description: "let sandbox read a directory: /sandbox-add-read-dir <absolute_path>",
                available_during_task: false,
                is_disabled: !cfg!(target_os = "windows"),
                hide_in_command_popup: false,
                usage_lines: &["/sandbox-add-read-dir <absolute-path>"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::Remainder(ParsedSlashCommand::SandboxReadRoot),
            },
            SlashCommand::Experimental => SlashCommandSpec {
                description: "toggle experimental features",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/experimental"],
                bare_behavior: SlashCommandBareBehavior::OpensUi,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Skills => SlashCommandSpec {
                description: "use skills to improve how Codex performs specific tasks",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/skills"],
                bare_behavior: SlashCommandBareBehavior::OpensUi,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Review => SlashCommandSpec {
                description: "review my current changes and find issues",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/review", "/review <instructions>"],
                bare_behavior: SlashCommandBareBehavior::OpensUi,
                args_parser: SlashCommandArgsParser::Remainder(ParsedSlashCommand::Review),
            },
            SlashCommand::Rename => SlashCommandSpec {
                description: "rename the current thread",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/rename", "/rename <title>"],
                bare_behavior: SlashCommandBareBehavior::OpensUi,
                args_parser: SlashCommandArgsParser::Remainder(ParsedSlashCommand::Rename),
            },
            SlashCommand::New => SlashCommandSpec {
                description: "start a new chat during a conversation",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/new"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Resume => SlashCommandSpec {
                description: "resume a saved chat",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/resume"],
                bare_behavior: SlashCommandBareBehavior::OpensUi,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Fork => SlashCommandSpec {
                description: "fork the current chat",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/fork"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Init => SlashCommandSpec {
                description: "create an AGENTS.md file with instructions for Codex",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/init"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Compact => SlashCommandSpec {
                description: "summarize conversation to prevent hitting the context limit",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/compact"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Plan => SlashCommandSpec {
                description: "switch to Plan mode",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/plan", "/plan <prompt>"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::Remainder(ParsedSlashCommand::Plan),
            },
            SlashCommand::Collab => SlashCommandSpec {
                description: "change collaboration mode (experimental)",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/collab"],
                bare_behavior: SlashCommandBareBehavior::OpensUi,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Agent => SlashCommandSpec {
                description: "switch the active agent thread",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/agent"],
                bare_behavior: SlashCommandBareBehavior::OpensUi,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Diff => SlashCommandSpec {
                description: "show git diff (including untracked files)",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/diff"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Copy => SlashCommandSpec {
                description: "copy the latest Codex output to your clipboard",
                available_during_task: true,
                is_disabled: cfg!(target_os = "android"),
                hide_in_command_popup: false,
                usage_lines: &["/copy"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Mention => SlashCommandSpec {
                description: "mention a file",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/mention"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Status => SlashCommandSpec {
                description: "show current session configuration and token usage",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/status"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::DebugConfig => SlashCommandSpec {
                description: "show config layers and requirement sources for debugging",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/debug-config"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Title => SlashCommandSpec {
                description: "configure which items appear in the terminal title",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/title"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Statusline => SlashCommandSpec {
                description: "configure which items appear in the status line",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/statusline"],
                bare_behavior: SlashCommandBareBehavior::OpensUi,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Theme => SlashCommandSpec {
                description: "choose a syntax highlighting theme",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/theme"],
                bare_behavior: SlashCommandBareBehavior::OpensUi,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Mcp => SlashCommandSpec {
                description: "list configured MCP tools",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/mcp"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Apps => SlashCommandSpec {
                description: "manage apps",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/apps"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Plugins => SlashCommandSpec {
                description: "browse plugins",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/plugins"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Logout => SlashCommandSpec {
                description: "log out of Codex",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/logout"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Quit => SlashCommandSpec {
                description: "exit Codex",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: true,
                usage_lines: &["/quit"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Exit => SlashCommandSpec {
                description: "exit Codex",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/exit"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Feedback => SlashCommandSpec {
                description: "send logs to maintainers",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/feedback"],
                bare_behavior: SlashCommandBareBehavior::OpensUi,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Rollout => SlashCommandSpec {
                description: "print the rollout file path",
                available_during_task: true,
                is_disabled: !cfg!(debug_assertions),
                hide_in_command_popup: false,
                usage_lines: &["/rollout"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Ps => SlashCommandSpec {
                description: "list background terminals",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/ps"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Stop => SlashCommandSpec {
                description: "stop all background terminals",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/stop"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Clear => SlashCommandSpec {
                description: "clear the terminal and start a new chat",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/clear"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Personality => SlashCommandSpec {
                description: "choose a communication style for Codex",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/personality"],
                bare_behavior: SlashCommandBareBehavior::OpensUi,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Realtime => SlashCommandSpec {
                description: "toggle realtime voice mode (experimental)",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/realtime"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::Settings => SlashCommandSpec {
                description: "configure realtime microphone/speaker",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/settings"],
                bare_behavior: SlashCommandBareBehavior::OpensUi,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::TestApproval => SlashCommandSpec {
                description: "test approval request",
                available_during_task: true,
                is_disabled: !cfg!(debug_assertions),
                hide_in_command_popup: false,
                usage_lines: &["/test-approval"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::MultiAgents => SlashCommandSpec {
                description: "switch the active agent thread",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/subagents"],
                bare_behavior: SlashCommandBareBehavior::OpensUi,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::MemoryDrop => SlashCommandSpec {
                description: "DO NOT USE",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/debug-m-drop"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
            SlashCommand::MemoryUpdate => SlashCommandSpec {
                description: "DO NOT USE",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
                usage_lines: &["/debug-m-update"],
                bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
                args_parser: SlashCommandArgsParser::NoArgs,
            },
        }
    }

    /// User-visible description shown in the popup.
    pub fn description(self) -> &'static str {
        self.spec().description
    }

    /// Command string without the leading '/'. Provided for compatibility with
    /// existing code that expects a method named `command()`.
    pub fn command(self) -> &'static str {
        self.into()
    }

    /// User-visible usage forms for this command.
    pub(crate) fn usage_lines(self) -> &'static [&'static str] {
        self.spec().usage_lines
    }

    /// Whether this command can be run while a task is in progress.
    pub fn available_during_task(self) -> bool {
        self.spec().available_during_task
    }

    pub(crate) fn hide_in_command_popup(self) -> bool {
        self.spec().hide_in_command_popup
    }

    /// Whether this command is disabled for the current build target.
    ///
    /// This is used for OS-specific or build-specific commands that still belong in the shared
    /// enum, such as Windows-only or debug-only slash commands.
    fn is_disabled(self) -> bool {
        self.spec().is_disabled
    }
}

/// Return all built-in commands in a Vec paired with their command string.
pub fn built_in_slash_commands() -> Vec<(&'static str, SlashCommand)> {
    SlashCommand::iter()
        .filter(|command| !command.is_disabled())
        .map(|c| (c.command(), c))
        .collect()
}

pub(crate) struct SlashCommandSpec {
    pub(crate) description: &'static str,
    pub(crate) available_during_task: bool,
    pub(crate) is_disabled: bool,
    pub(crate) hide_in_command_popup: bool,
    pub(crate) usage_lines: &'static [&'static str],
    pub(crate) bare_behavior: SlashCommandBareBehavior,
    pub(crate) args_parser: SlashCommandArgsParser,
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use std::str::FromStr;

    use super::SlashCommand;

    #[test]
    fn approvals_alias_is_hidden_from_command_popup() {
        assert!(SlashCommand::Approvals.hide_in_command_popup());
    }

    #[test]
    fn clean_alias_parses_to_stop_command() {
        assert_eq!(SlashCommand::from_str("clean"), Ok(SlashCommand::Stop));
    }

    #[test]
    fn stop_command_is_canonical_name() {
        assert_eq!(SlashCommand::Stop.command(), "stop");
    }

    #[test]
    fn fast_usage_lists_bare_and_arg_forms() {
        assert_eq!(
            SlashCommand::Fast.usage_lines(),
            ["/fast", "/fast [on|off|status]"]
        );
    }

    #[test]
    fn clear_usage_is_bare_only() {
        assert_eq!(SlashCommand::Clear.usage_lines(), ["/clear"]);
    }
}
