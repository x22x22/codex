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

impl SlashCommand {
    fn spec(self) -> SlashCommandSpec {
        match self {
            SlashCommand::Model => SlashCommandSpec {
                description: "choose what model and reasoning effort to use",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Fast => SlashCommandSpec {
                description: "toggle Fast mode to enable fastest inference at 2X plan usage",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Approvals => SlashCommandSpec {
                description: "choose what Codex is allowed to do",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: true,
            },
            SlashCommand::Permissions => SlashCommandSpec {
                description: "choose what Codex is allowed to do",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::ElevateSandbox => SlashCommandSpec {
                description: "set up elevated agent sandbox",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::SandboxReadRoot => SlashCommandSpec {
                description: "let sandbox read a directory: /sandbox-add-read-dir <absolute_path>",
                available_during_task: false,
                is_disabled: !cfg!(target_os = "windows"),
                hide_in_command_popup: false,
            },
            SlashCommand::Experimental => SlashCommandSpec {
                description: "toggle experimental features",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Skills => SlashCommandSpec {
                description: "use skills to improve how Codex performs specific tasks",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Review => SlashCommandSpec {
                description: "review my current changes and find issues",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Rename => SlashCommandSpec {
                description: "rename the current thread",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::New => SlashCommandSpec {
                description: "start a new chat during a conversation",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Resume => SlashCommandSpec {
                description: "resume a saved chat",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Fork => SlashCommandSpec {
                description: "fork the current chat",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Init => SlashCommandSpec {
                description: "create an AGENTS.md file with instructions for Codex",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Compact => SlashCommandSpec {
                description: "summarize conversation to prevent hitting the context limit",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Plan => SlashCommandSpec {
                description: "switch to Plan mode",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Collab => SlashCommandSpec {
                description: "change collaboration mode (experimental)",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Agent => SlashCommandSpec {
                description: "switch the active agent thread",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Diff => SlashCommandSpec {
                description: "show git diff (including untracked files)",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Copy => SlashCommandSpec {
                description: "copy the latest Codex output to your clipboard",
                available_during_task: true,
                is_disabled: cfg!(target_os = "android"),
                hide_in_command_popup: false,
            },
            SlashCommand::Mention => SlashCommandSpec {
                description: "mention a file",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Status => SlashCommandSpec {
                description: "show current session configuration and token usage",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::DebugConfig => SlashCommandSpec {
                description: "show config layers and requirement sources for debugging",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Title => SlashCommandSpec {
                description: "configure which items appear in the terminal title",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Statusline => SlashCommandSpec {
                description: "configure which items appear in the status line",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Theme => SlashCommandSpec {
                description: "choose a syntax highlighting theme",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Mcp => SlashCommandSpec {
                description: "list configured MCP tools",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Apps => SlashCommandSpec {
                description: "manage apps",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Plugins => SlashCommandSpec {
                description: "browse plugins",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Logout => SlashCommandSpec {
                description: "log out of Codex",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Quit => SlashCommandSpec {
                description: "exit Codex",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: true,
            },
            SlashCommand::Exit => SlashCommandSpec {
                description: "exit Codex",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Feedback => SlashCommandSpec {
                description: "send logs to maintainers",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Rollout => SlashCommandSpec {
                description: "print the rollout file path",
                available_during_task: true,
                is_disabled: !cfg!(debug_assertions),
                hide_in_command_popup: false,
            },
            SlashCommand::Ps => SlashCommandSpec {
                description: "list background terminals",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Stop => SlashCommandSpec {
                description: "stop all background terminals",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Clear => SlashCommandSpec {
                description: "clear the terminal and start a new chat",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Personality => SlashCommandSpec {
                description: "choose a communication style for Codex",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Realtime => SlashCommandSpec {
                description: "toggle realtime voice mode (experimental)",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::Settings => SlashCommandSpec {
                description: "configure realtime microphone/speaker",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::TestApproval => SlashCommandSpec {
                description: "test approval request",
                available_during_task: true,
                is_disabled: !cfg!(debug_assertions),
                hide_in_command_popup: false,
            },
            SlashCommand::MultiAgents => SlashCommandSpec {
                description: "switch the active agent thread",
                available_during_task: true,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::MemoryDrop => SlashCommandSpec {
                description: "DO NOT USE",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
            },
            SlashCommand::MemoryUpdate => SlashCommandSpec {
                description: "DO NOT USE",
                available_during_task: false,
                is_disabled: false,
                hide_in_command_popup: false,
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
        match self {
            SlashCommand::Model => &["/model"],
            SlashCommand::Fast => &["/fast", "/fast [on|off|status]"],
            SlashCommand::Approvals => &["/approvals"],
            SlashCommand::Permissions => &["/permissions"],
            SlashCommand::ElevateSandbox => &["/setup-default-sandbox"],
            SlashCommand::SandboxReadRoot => &["/sandbox-add-read-dir <absolute-path>"],
            SlashCommand::Experimental => &["/experimental"],
            SlashCommand::Skills => &["/skills"],
            SlashCommand::Review => &["/review", "/review <instructions>"],
            SlashCommand::Rename => &["/rename", "/rename <title>"],
            SlashCommand::New => &["/new"],
            SlashCommand::Resume => &["/resume"],
            SlashCommand::Fork => &["/fork"],
            SlashCommand::Init => &["/init"],
            SlashCommand::Compact => &["/compact"],
            SlashCommand::Plan => &["/plan", "/plan <prompt>"],
            SlashCommand::Collab => &["/collab"],
            SlashCommand::Agent => &["/agent"],
            SlashCommand::Diff => &["/diff"],
            SlashCommand::Copy => &["/copy"],
            SlashCommand::Mention => &["/mention"],
            SlashCommand::Status => &["/status"],
            SlashCommand::DebugConfig => &["/debug-config"],
            SlashCommand::Title => &["/title"],
            SlashCommand::Statusline => &["/statusline"],
            SlashCommand::Theme => &["/theme"],
            SlashCommand::Mcp => &["/mcp"],
            SlashCommand::Apps => &["/apps"],
            SlashCommand::Plugins => &["/plugins"],
            SlashCommand::Logout => &["/logout"],
            SlashCommand::Quit => &["/quit"],
            SlashCommand::Exit => &["/exit"],
            SlashCommand::Feedback => &["/feedback"],
            SlashCommand::Rollout => &["/rollout"],
            SlashCommand::Ps => &["/ps"],
            SlashCommand::Stop => &["/stop"],
            SlashCommand::Clear => &["/clear"],
            SlashCommand::Personality => &["/personality"],
            SlashCommand::Realtime => &["/realtime"],
            SlashCommand::Settings => &["/settings"],
            SlashCommand::TestApproval => &["/test-approval"],
            SlashCommand::MultiAgents => &["/subagents"],
            SlashCommand::MemoryDrop => &["/debug-m-drop"],
            SlashCommand::MemoryUpdate => &["/debug-m-update"],
        }
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

struct SlashCommandSpec {
    description: &'static str,
    available_during_task: bool,
    is_disabled: bool,
    hide_in_command_popup: bool,
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
