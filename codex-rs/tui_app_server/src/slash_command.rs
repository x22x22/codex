use strum_macros::AsRefStr;
use strum_macros::EnumString;
use strum_macros::IntoStaticStr;

use crate::app_event::FeedbackCategory;
use crate::bottom_pane::StatusLineItem;
use crate::slash_command_protocol::SlashArgsParser;
use crate::slash_command_protocol::SlashArgsSerializer;
use crate::slash_command_protocol::SlashCommandArgs;
use crate::slash_command_protocol::SlashCommandParseInput;
use crate::slash_command_protocol::SlashCommandUsageErrorKind;
use crate::slash_command_protocol::SlashSerializedText;
pub(crate) use crate::slash_command_protocol::SlashTextArg;
use crate::slash_command_protocol::SlashTokenArg;
use crate::slash_command_protocol::SlashTokenValue;

/// Commands that can be invoked by starting a message with a leading slash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, AsRefStr, IntoStaticStr)]
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
    Diff,
    Copy,
    Mention,
    Status,
    DebugConfig,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FastSlashCommandArgs {
    On,
    Off,
    Status,
}

impl SlashTokenValue for FastSlashCommandArgs {
    fn parse_token(token: SlashTokenArg) -> Result<Self, SlashCommandUsageErrorKind> {
        match token.text.as_str() {
            value if value.eq_ignore_ascii_case("on") => Ok(Self::On),
            value if value.eq_ignore_ascii_case("off") => Ok(Self::Off),
            value if value.eq_ignore_ascii_case("status") => Ok(Self::Status),
            _ => Err(SlashCommandUsageErrorKind::InvalidInlineArgs),
        }
    }

    fn serialize_token(&self) -> SlashTokenArg {
        let text = match self {
            Self::On => "on",
            Self::Off => "off",
            Self::Status => "status",
        };
        SlashTokenArg::new(text.to_string(), Vec::new())
    }
}

impl SlashTokenValue for FeedbackCategory {
    fn parse_token(token: SlashTokenArg) -> Result<Self, SlashCommandUsageErrorKind> {
        match token.text.as_str() {
            "bad-result" => Ok(Self::BadResult),
            "good-result" => Ok(Self::GoodResult),
            "bug" => Ok(Self::Bug),
            "safety-check" => Ok(Self::SafetyCheck),
            "other" => Ok(Self::Other),
            _ => Err(SlashCommandUsageErrorKind::InvalidInlineArgs),
        }
    }

    fn serialize_token(&self) -> SlashTokenArg {
        let text = match self {
            Self::BadResult => "bad-result",
            Self::GoodResult => "good-result",
            Self::Bug => "bug",
            Self::SafetyCheck => "safety-check",
            Self::Other => "other",
        };
        SlashTokenArg::new(text.to_string(), Vec::new())
    }
}

impl SlashTokenValue for StatusLineItem {
    fn parse_token(token: SlashTokenArg) -> Result<Self, SlashCommandUsageErrorKind> {
        token
            .text
            .parse()
            .map_err(|_| SlashCommandUsageErrorKind::InvalidInlineArgs)
    }

    fn serialize_token(&self) -> SlashTokenArg {
        SlashTokenArg::new(self.to_string(), Vec::new())
    }
}

pub(crate) trait SlashCommandInlineArgs: SlashCommandArgs + Sized {
    const USAGE_LINES: &'static [&'static str];

    fn into_invocation(self) -> SlashCommandInvocation;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FastArgs {
    pub(crate) mode: FastSlashCommandArgs,
}

impl SlashCommandArgs for FastArgs {
    fn parse(input: SlashCommandParseInput<'_>) -> Result<Self, SlashCommandUsageErrorKind> {
        let mut parser = SlashArgsParser::new(input)?;
        let mode = parser.positional::<FastSlashCommandArgs>()?;
        parser.finish()?;
        Ok(Self { mode })
    }

    fn serialize(&self) -> SlashSerializedText {
        let mut serializer = SlashArgsSerializer::default();
        serializer.positional(&self.mode);
        serializer.finish()
    }
}

impl SlashCommandInlineArgs for FastArgs {
    const USAGE_LINES: &'static [&'static str] = &["/fast", "/fast [on|off|status]"];

    fn into_invocation(self) -> SlashCommandInvocation {
        SlashCommandInvocation::Fast(self)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RenameArgs {
    pub(crate) title: SlashTextArg,
}

impl SlashCommandArgs for RenameArgs {
    fn parse(input: SlashCommandParseInput<'_>) -> Result<Self, SlashCommandUsageErrorKind> {
        let parser = SlashArgsParser::new(input)?;
        let title = parser.required_remainder()?;
        Ok(Self { title })
    }

    fn serialize(&self) -> SlashSerializedText {
        let mut serializer = SlashArgsSerializer::default();
        serializer.remainder(&self.title);
        serializer.finish()
    }
}

impl SlashCommandInlineArgs for RenameArgs {
    const USAGE_LINES: &'static [&'static str] = &["/rename", "/rename <title>"];

    fn into_invocation(self) -> SlashCommandInvocation {
        SlashCommandInvocation::Rename(self)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlanArgs {
    pub(crate) prompt: SlashTextArg,
}

impl SlashCommandArgs for PlanArgs {
    fn parse(input: SlashCommandParseInput<'_>) -> Result<Self, SlashCommandUsageErrorKind> {
        let parser = SlashArgsParser::new(input)?;
        let prompt = parser.required_remainder()?;
        Ok(Self { prompt })
    }

    fn serialize(&self) -> SlashSerializedText {
        let mut serializer = SlashArgsSerializer::default();
        serializer.remainder(&self.prompt);
        serializer.finish()
    }
}

impl SlashCommandInlineArgs for PlanArgs {
    const USAGE_LINES: &'static [&'static str] = &["/plan", "/plan <prompt>"];

    fn into_invocation(self) -> SlashCommandInvocation {
        SlashCommandInvocation::Plan(self)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReviewArgs {
    pub(crate) instructions: SlashTextArg,
}

impl SlashCommandArgs for ReviewArgs {
    fn parse(input: SlashCommandParseInput<'_>) -> Result<Self, SlashCommandUsageErrorKind> {
        let parser = SlashArgsParser::new(input)?;
        let instructions = parser.required_remainder()?;
        Ok(Self { instructions })
    }

    fn serialize(&self) -> SlashSerializedText {
        let mut serializer = SlashArgsSerializer::default();
        serializer.remainder(&self.instructions);
        serializer.finish()
    }
}

impl SlashCommandInlineArgs for ReviewArgs {
    const USAGE_LINES: &'static [&'static str] = &["/review", "/review <instructions>"];

    fn into_invocation(self) -> SlashCommandInvocation {
        SlashCommandInvocation::Review(self)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SandboxReadRootArgs {
    pub(crate) path: SlashTokenArg,
}

impl SlashCommandArgs for SandboxReadRootArgs {
    fn parse(input: SlashCommandParseInput<'_>) -> Result<Self, SlashCommandUsageErrorKind> {
        let mut parser = SlashArgsParser::new(input)?;
        let path = match parser.named::<SlashTokenArg>("path")? {
            Some(path) => path,
            None => parser.positional::<SlashTokenArg>()?,
        };
        parser.finish()?;
        Ok(Self { path })
    }

    fn serialize(&self) -> SlashSerializedText {
        let mut serializer = SlashArgsSerializer::default();
        serializer.positional(&self.path);
        serializer.finish()
    }
}

impl SlashCommandInlineArgs for SandboxReadRootArgs {
    const USAGE_LINES: &'static [&'static str] = &[
        "/sandbox-add-read-dir <absolute-path>",
        "/sandbox-add-read-dir --path=<absolute-path>",
    ];

    fn into_invocation(self) -> SlashCommandInvocation {
        SlashCommandInvocation::SandboxReadRoot(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FeedbackArgs {
    pub(crate) category: FeedbackCategory,
}

impl SlashCommandArgs for FeedbackArgs {
    fn parse(input: SlashCommandParseInput<'_>) -> Result<Self, SlashCommandUsageErrorKind> {
        let mut parser = SlashArgsParser::new(input)?;
        let category = parser.positional::<FeedbackCategory>()?;
        parser.finish()?;
        Ok(Self { category })
    }

    fn serialize(&self) -> SlashSerializedText {
        let mut serializer = SlashArgsSerializer::default();
        serializer.positional(&self.category);
        serializer.finish()
    }
}

impl SlashCommandInlineArgs for FeedbackArgs {
    const USAGE_LINES: &'static [&'static str] = &[
        "/feedback",
        "/feedback <bad-result|good-result|bug|safety-check|other>",
    ];

    fn into_invocation(self) -> SlashCommandInvocation {
        SlashCommandInvocation::Feedback(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatuslineArgs {
    pub(crate) items: Vec<StatusLineItem>,
}

impl SlashCommandArgs for StatuslineArgs {
    fn parse(input: SlashCommandParseInput<'_>) -> Result<Self, SlashCommandUsageErrorKind> {
        let mut parser = SlashArgsParser::new(input)?;
        let items = parser.positional_list::<StatusLineItem>()?;
        parser.finish()?;
        if items.is_empty() {
            return Err(SlashCommandUsageErrorKind::InvalidInlineArgs);
        }
        Ok(Self { items })
    }

    fn serialize(&self) -> SlashSerializedText {
        let mut serializer = SlashArgsSerializer::default();
        serializer.list::<StatusLineItem, _>(self.items.iter().cloned());
        serializer.finish()
    }
}

impl SlashCommandInlineArgs for StatuslineArgs {
    const USAGE_LINES: &'static [&'static str] = &["/statusline", "/statusline <item>..."];

    fn into_invocation(self) -> SlashCommandInvocation {
        SlashCommandInvocation::Statusline(self)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SlashCommandInvocation {
    Bare(SlashCommand),
    Fast(FastArgs),
    Rename(RenameArgs),
    Plan(PlanArgs),
    Review(ReviewArgs),
    SandboxReadRoot(SandboxReadRootArgs),
    Feedback(FeedbackArgs),
    Statusline(StatuslineArgs),
}

impl SlashCommandInvocation {
    pub(crate) fn bare(command: SlashCommand) -> Self {
        Self::Bare(command)
    }

    pub(crate) fn command(&self) -> SlashCommand {
        match self {
            Self::Bare(command) => *command,
            Self::Fast(_) => SlashCommand::Fast,
            Self::Rename(_) => SlashCommand::Rename,
            Self::Plan(_) => SlashCommand::Plan,
            Self::Review(_) => SlashCommand::Review,
            Self::SandboxReadRoot(_) => SlashCommand::SandboxReadRoot,
            Self::Feedback(_) => SlashCommand::Feedback,
            Self::Statusline(_) => SlashCommand::Statusline,
        }
    }

    pub(crate) fn serialize(&self) -> SlashSerializedText {
        let prefix = format!("/{}", self.command().command());
        match self {
            Self::Bare(_) => SlashSerializedText::empty().with_prefix(&prefix),
            Self::Fast(args) => args.serialize().with_prefix(&prefix),
            Self::Rename(args) => args.serialize().with_prefix(&prefix),
            Self::Plan(args) => args.serialize().with_prefix(&prefix),
            Self::Review(args) => args.serialize().with_prefix(&prefix),
            Self::SandboxReadRoot(args) => args.serialize().with_prefix(&prefix),
            Self::Feedback(args) => args.serialize().with_prefix(&prefix),
            Self::Statusline(args) => args.serialize().with_prefix(&prefix),
        }
    }

    pub(crate) fn into_prefixed_string(self) -> String {
        self.serialize().text
    }
}

pub(crate) type SlashCommandInlineParser =
    for<'a> fn(
        SlashCommandParseInput<'a>,
    ) -> Result<SlashCommandInvocation, SlashCommandUsageErrorKind>;

pub(crate) struct SlashCommandSpec {
    pub(crate) command: SlashCommand,
    pub(crate) description: &'static str,
    pub(crate) available_during_task: bool,
    pub(crate) is_disabled: bool,
    pub(crate) hide_in_command_popup: bool,
    pub(crate) usage_lines: &'static [&'static str],
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) bare_behavior: SlashCommandBareBehavior,
    pub(crate) parse_inline: SlashCommandInlineParser,
}

fn reject_inline_args(
    _input: SlashCommandParseInput<'_>,
) -> Result<SlashCommandInvocation, SlashCommandUsageErrorKind> {
    Err(SlashCommandUsageErrorKind::UnexpectedInlineArgs)
}

fn parse_typed_inline<T>(
    input: SlashCommandParseInput<'_>,
) -> Result<SlashCommandInvocation, SlashCommandUsageErrorKind>
where
    T: SlashCommandInlineArgs,
{
    T::parse(input).map(T::into_invocation)
}

// ===== /model =====
const MODEL_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Model,
    description: "choose what model and reasoning effort to use",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/model"],
    bare_behavior: SlashCommandBareBehavior::OpensUi,
    parse_inline: reject_inline_args,
};

// ===== /fast =====
const FAST_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Fast,
    description: "toggle Fast mode to enable fastest inference at 2X plan usage",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: FastArgs::USAGE_LINES,
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: parse_typed_inline::<FastArgs>,
};

// ===== /approvals =====
const APPROVALS_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Approvals,
    description: "choose what Codex is allowed to do",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: true,
    usage_lines: &["/approvals"],
    bare_behavior: SlashCommandBareBehavior::OpensUi,
    parse_inline: reject_inline_args,
};

// ===== /permissions =====
const PERMISSIONS_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Permissions,
    description: "choose what Codex is allowed to do",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/permissions"],
    bare_behavior: SlashCommandBareBehavior::OpensUi,
    parse_inline: reject_inline_args,
};

// ===== /setup-default-sandbox =====
const ELEVATE_SANDBOX_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::ElevateSandbox,
    description: "set up elevated agent sandbox",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/setup-default-sandbox"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /sandbox-add-read-dir =====
const SANDBOX_READ_ROOT_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::SandboxReadRoot,
    description: "let sandbox read a directory: /sandbox-add-read-dir <absolute_path>",
    available_during_task: false,
    is_disabled: !cfg!(target_os = "windows"),
    hide_in_command_popup: false,
    usage_lines: SandboxReadRootArgs::USAGE_LINES,
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: parse_typed_inline::<SandboxReadRootArgs>,
};

// ===== /experimental =====
const EXPERIMENTAL_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Experimental,
    description: "toggle experimental features",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/experimental"],
    bare_behavior: SlashCommandBareBehavior::OpensUi,
    parse_inline: reject_inline_args,
};

// ===== /skills =====
const SKILLS_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Skills,
    description: "use skills to improve how Codex performs specific tasks",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/skills"],
    bare_behavior: SlashCommandBareBehavior::OpensUi,
    parse_inline: reject_inline_args,
};

// ===== /review =====
const REVIEW_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Review,
    description: "review my current changes and find issues",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: ReviewArgs::USAGE_LINES,
    bare_behavior: SlashCommandBareBehavior::OpensUi,
    parse_inline: parse_typed_inline::<ReviewArgs>,
};

// ===== /rename =====
const RENAME_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Rename,
    description: "rename the current thread",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: RenameArgs::USAGE_LINES,
    bare_behavior: SlashCommandBareBehavior::OpensUi,
    parse_inline: parse_typed_inline::<RenameArgs>,
};

// ===== /new =====
const NEW_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::New,
    description: "start a new chat during a conversation",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/new"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /resume =====
const RESUME_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Resume,
    description: "resume a saved chat",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/resume"],
    bare_behavior: SlashCommandBareBehavior::OpensUi,
    parse_inline: reject_inline_args,
};

// ===== /fork =====
const FORK_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Fork,
    description: "fork the current chat",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/fork"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /init =====
const INIT_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Init,
    description: "create an AGENTS.md file with instructions for Codex",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/init"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /compact =====
const COMPACT_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Compact,
    description: "summarize conversation to prevent hitting the context limit",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/compact"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /plan =====
const PLAN_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Plan,
    description: "switch to Plan mode",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: PlanArgs::USAGE_LINES,
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: parse_typed_inline::<PlanArgs>,
};

// ===== /collab =====
const COLLAB_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Collab,
    description: "change collaboration mode (experimental)",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/collab"],
    bare_behavior: SlashCommandBareBehavior::OpensUi,
    parse_inline: reject_inline_args,
};

// ===== /agent =====
const AGENT_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Agent,
    description: "switch the active agent thread",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/agent"],
    bare_behavior: SlashCommandBareBehavior::OpensUi,
    parse_inline: reject_inline_args,
};

// ===== /diff =====
const DIFF_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Diff,
    description: "show git diff (including untracked files)",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/diff"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /copy =====
const COPY_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Copy,
    description: "copy the latest Codex output to your clipboard",
    available_during_task: true,
    is_disabled: cfg!(target_os = "android"),
    hide_in_command_popup: false,
    usage_lines: &["/copy"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /mention =====
const MENTION_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Mention,
    description: "mention a file",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/mention"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /status =====
const STATUS_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Status,
    description: "show current session configuration and token usage",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/status"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /debug-config =====
const DEBUG_CONFIG_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::DebugConfig,
    description: "show config layers and requirement sources for debugging",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/debug-config"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /statusline =====
const STATUSLINE_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Statusline,
    description: "configure which items appear in the status line",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: StatuslineArgs::USAGE_LINES,
    bare_behavior: SlashCommandBareBehavior::OpensUi,
    parse_inline: parse_typed_inline::<StatuslineArgs>,
};

// ===== /theme =====
const THEME_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Theme,
    description: "choose a syntax highlighting theme",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/theme"],
    bare_behavior: SlashCommandBareBehavior::OpensUi,
    parse_inline: reject_inline_args,
};

// ===== /mcp =====
const MCP_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Mcp,
    description: "list configured MCP tools",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/mcp"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /apps =====
const APPS_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Apps,
    description: "manage apps",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/apps"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /plugins =====
const PLUGINS_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Plugins,
    description: "browse plugins",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/plugins"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /logout =====
const LOGOUT_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Logout,
    description: "log out of Codex",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/logout"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /quit =====
const QUIT_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Quit,
    description: "exit Codex",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: true,
    usage_lines: &["/quit"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /exit =====
const EXIT_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Exit,
    description: "exit Codex",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/exit"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /feedback =====
const FEEDBACK_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Feedback,
    description: "send logs to maintainers",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: FeedbackArgs::USAGE_LINES,
    bare_behavior: SlashCommandBareBehavior::OpensUi,
    parse_inline: parse_typed_inline::<FeedbackArgs>,
};

// ===== /rollout =====
const ROLLOUT_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Rollout,
    description: "print the rollout file path",
    available_during_task: true,
    is_disabled: !cfg!(debug_assertions),
    hide_in_command_popup: false,
    usage_lines: &["/rollout"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /ps =====
const PS_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Ps,
    description: "list background terminals",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/ps"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /stop =====
const STOP_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Stop,
    description: "stop all background terminals",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/stop"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /clear =====
const CLEAR_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Clear,
    description: "clear the terminal and start a new chat",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/clear"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /personality =====
const PERSONALITY_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Personality,
    description: "choose a communication style for Codex",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/personality"],
    bare_behavior: SlashCommandBareBehavior::OpensUi,
    parse_inline: reject_inline_args,
};

// ===== /realtime =====
const REALTIME_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Realtime,
    description: "toggle realtime voice mode (experimental)",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/realtime"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /settings =====
const SETTINGS_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::Settings,
    description: "configure realtime microphone/speaker",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/settings"],
    bare_behavior: SlashCommandBareBehavior::OpensUi,
    parse_inline: reject_inline_args,
};

// ===== /test-approval =====
const TEST_APPROVAL_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::TestApproval,
    description: "test approval request",
    available_during_task: true,
    is_disabled: !cfg!(debug_assertions),
    hide_in_command_popup: false,
    usage_lines: &["/test-approval"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /subagents =====
const MULTI_AGENTS_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::MultiAgents,
    description: "switch the active agent thread",
    available_during_task: true,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/subagents"],
    bare_behavior: SlashCommandBareBehavior::OpensUi,
    parse_inline: reject_inline_args,
};

// ===== /debug-m-drop =====
const MEMORY_DROP_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::MemoryDrop,
    description: "DO NOT USE",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/debug-m-drop"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

// ===== /debug-m-update =====
const MEMORY_UPDATE_SPEC: SlashCommandSpec = SlashCommandSpec {
    command: SlashCommand::MemoryUpdate,
    description: "DO NOT USE",
    available_during_task: false,
    is_disabled: false,
    hide_in_command_popup: false,
    usage_lines: &["/debug-m-update"],
    bare_behavior: SlashCommandBareBehavior::DispatchesDirectly,
    parse_inline: reject_inline_args,
};

const SLASH_COMMAND_SPECS: &[SlashCommandSpec] = &[
    MODEL_SPEC,
    FAST_SPEC,
    APPROVALS_SPEC,
    PERMISSIONS_SPEC,
    ELEVATE_SANDBOX_SPEC,
    SANDBOX_READ_ROOT_SPEC,
    EXPERIMENTAL_SPEC,
    SKILLS_SPEC,
    REVIEW_SPEC,
    RENAME_SPEC,
    NEW_SPEC,
    RESUME_SPEC,
    FORK_SPEC,
    INIT_SPEC,
    COMPACT_SPEC,
    PLAN_SPEC,
    COLLAB_SPEC,
    AGENT_SPEC,
    DIFF_SPEC,
    COPY_SPEC,
    MENTION_SPEC,
    STATUS_SPEC,
    DEBUG_CONFIG_SPEC,
    STATUSLINE_SPEC,
    THEME_SPEC,
    MCP_SPEC,
    APPS_SPEC,
    PLUGINS_SPEC,
    LOGOUT_SPEC,
    QUIT_SPEC,
    EXIT_SPEC,
    FEEDBACK_SPEC,
    ROLLOUT_SPEC,
    PS_SPEC,
    STOP_SPEC,
    CLEAR_SPEC,
    PERSONALITY_SPEC,
    REALTIME_SPEC,
    SETTINGS_SPEC,
    TEST_APPROVAL_SPEC,
    MULTI_AGENTS_SPEC,
    MEMORY_DROP_SPEC,
    MEMORY_UPDATE_SPEC,
];

impl SlashCommand {
    fn spec(self) -> &'static SlashCommandSpec {
        match SLASH_COMMAND_SPECS.iter().find(|spec| spec.command == self) {
            Some(spec) => spec,
            None => panic!("every slash command must have a registered spec"),
        }
    }

    pub(crate) fn parse_invocation(
        self,
        args: &str,
        text_elements: &[codex_protocol::user_input::TextElement],
    ) -> Result<SlashCommandInvocation, SlashCommandUsageError> {
        if args.trim().is_empty() {
            return Ok(SlashCommandInvocation::Bare(self));
        }

        (self.spec().parse_inline)(SlashCommandParseInput {
            args,
            text_elements,
        })
        .map_err(|kind| SlashCommandUsageError {
            command: self,
            kind,
        })
    }

    /// User-visible description shown in the popup.
    pub fn description(self) -> &'static str {
        self.spec().description
    }

    /// Command string without the leading '/'.
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
}

/// Return all built-in commands in a Vec paired with their command string.
pub fn built_in_slash_commands() -> Vec<(&'static str, SlashCommand)> {
    SLASH_COMMAND_SPECS
        .iter()
        .filter(|spec| !spec.is_disabled)
        .map(|spec| (spec.command.command(), spec.command))
        .collect()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use std::str::FromStr;

    use super::*;

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

    #[test]
    fn review_bare_form_is_marked_as_ui_driven() {
        assert_eq!(
            SlashCommand::Review.parse_invocation("", &[]),
            Ok(SlashCommandInvocation::Bare(SlashCommand::Review))
        );
        assert_eq!(
            SlashCommand::Review.spec().bare_behavior,
            SlashCommandBareBehavior::OpensUi
        );
    }

    #[test]
    fn fast_accepts_nonempty_inline_args() {
        assert_eq!(
            SlashCommand::Fast.parse_invocation("status", &[]),
            Ok(SlashCommandInvocation::Fast(FastArgs {
                mode: FastSlashCommandArgs::Status,
            }))
        );
    }

    #[test]
    fn feedback_accepts_category_arg() {
        assert_eq!(
            SlashCommand::Feedback.parse_invocation("bug", &[]),
            Ok(SlashCommandInvocation::Feedback(FeedbackArgs {
                category: FeedbackCategory::Bug,
            }))
        );
    }

    #[test]
    fn statusline_accepts_variadic_item_list() {
        assert_eq!(
            SlashCommand::Statusline.parse_invocation("model-name current-dir", &[]),
            Ok(SlashCommandInvocation::Statusline(StatuslineArgs {
                items: vec![StatusLineItem::ModelName, StatusLineItem::CurrentDir],
            }))
        );
    }

    #[test]
    fn sandbox_read_root_accepts_named_path_arg() {
        let invocation = SlashCommand::SandboxReadRoot
            .parse_invocation("--path='/tmp/test dir'", &[])
            .unwrap();

        assert_eq!(
            invocation,
            SlashCommandInvocation::SandboxReadRoot(SandboxReadRootArgs {
                path: SlashTokenArg::new("/tmp/test dir".to_string(), Vec::new()),
            })
        );
        assert_eq!(
            invocation.serialize().text,
            "/sandbox-add-read-dir '/tmp/test dir'"
        );
    }

    #[test]
    fn review_preserves_placeholder_elements() {
        let placeholder = "[Image #1]".to_string();
        let text_elements = vec![codex_protocol::user_input::TextElement::new(
            (0..placeholder.len()).into(),
            Some(placeholder.clone()),
        )];

        assert_eq!(
            SlashCommand::Review.parse_invocation(&placeholder, &text_elements),
            Ok(SlashCommandInvocation::Review(ReviewArgs {
                instructions: SlashTextArg::new(placeholder, text_elements),
            }))
        );
    }

    #[test]
    fn clear_rejects_unexpected_inline_args() {
        assert_eq!(
            SlashCommand::Clear
                .parse_invocation("now", &[])
                .unwrap_err()
                .message(),
            "'/clear' does not accept inline arguments. Usage: /clear"
        );
    }

    #[test]
    fn plan_serialization_preserves_placeholder_ranges() {
        let placeholder = "[Image #1]".to_string();
        let invocation = SlashCommandInvocation::Plan(PlanArgs {
            prompt: SlashTextArg::new(
                format!("review {placeholder}"),
                vec![codex_protocol::user_input::TextElement::new(
                    (7..18).into(),
                    Some(placeholder.clone()),
                )],
            ),
        });

        assert_eq!(
            invocation.serialize(),
            SlashSerializedText {
                text: format!("/plan review {placeholder}"),
                text_elements: vec![codex_protocol::user_input::TextElement::new(
                    (13..24).into(),
                    Some(placeholder),
                )],
            }
        );
    }
}
