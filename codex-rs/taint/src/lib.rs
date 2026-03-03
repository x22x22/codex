use std::collections::BTreeSet;
use std::fmt;

const MAX_RECENT_SOURCES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TaintLabel {
    WorkspaceContent,
    ExternalContent,
    AgentContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaintSource {
    ReadFile,
    GrepFiles,
    ListDir,
    ViewImage,
    ShellOutput,
    UserShellOutput,
    McpTool,
    McpResource,
    DynamicTool,
    WebSearch,
    AgentResult,
}

impl TaintSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::ReadFile => "read_file",
            Self::GrepFiles => "grep_files",
            Self::ListDir => "list_dir",
            Self::ViewImage => "view_image",
            Self::ShellOutput => "shell_output",
            Self::UserShellOutput => "user_shell_output",
            Self::McpTool => "mcp_tool",
            Self::McpResource => "mcp_resource",
            Self::DynamicTool => "dynamic_tool",
            Self::WebSearch => "web_search",
            Self::AgentResult => "agent_result",
        }
    }
}

impl fmt::Display for TaintSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaintSink {
    ShellExec,
    ExternalDispatch,
    AgentForward,
}

impl TaintSink {
    pub const fn shell_exec() -> Self {
        Self::ShellExec
    }

    pub const fn external_dispatch() -> Self {
        Self::ExternalDispatch
    }

    pub const fn agent_forward() -> Self {
        Self::AgentForward
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::ShellExec => "shell execution",
            Self::ExternalDispatch => "external tool dispatch",
            Self::AgentForward => "agent forwarding",
        }
    }

    fn blocked_labels(self) -> &'static [TaintLabel] {
        match self {
            Self::ShellExec => &[
                TaintLabel::WorkspaceContent,
                TaintLabel::ExternalContent,
                TaintLabel::AgentContent,
            ],
            Self::ExternalDispatch => &[TaintLabel::ExternalContent, TaintLabel::AgentContent],
            Self::AgentForward => &[TaintLabel::ExternalContent, TaintLabel::AgentContent],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaintEffect {
    None,
    Mark {
        label: TaintLabel,
        source: TaintSource,
    },
    Reset,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaintState {
    labels: BTreeSet<TaintLabel>,
    recent_sources: Vec<TaintSource>,
}

impl TaintState {
    pub fn apply(&mut self, effect: TaintEffect) {
        match effect {
            TaintEffect::None => {}
            TaintEffect::Reset => self.reset(),
            TaintEffect::Mark { label, source } => {
                self.labels.insert(label);
                if self.recent_sources.last().copied() != Some(source) {
                    self.recent_sources.push(source);
                    if self.recent_sources.len() > MAX_RECENT_SOURCES {
                        let overflow = self.recent_sources.len() - MAX_RECENT_SOURCES;
                        self.recent_sources.drain(0..overflow);
                    }
                }
            }
        }
    }

    pub fn reset(&mut self) {
        self.labels.clear();
        self.recent_sources.clear();
    }

    pub fn labels(&self) -> &BTreeSet<TaintLabel> {
        &self.labels
    }

    pub fn recent_sources(&self) -> &[TaintSource] {
        &self.recent_sources
    }

    pub fn check_sink(&self, sink: TaintSink) -> Result<(), TaintViolation> {
        let blocked_labels = self
            .labels
            .iter()
            .copied()
            .filter(|label| sink.blocked_labels().contains(label))
            .collect::<Vec<_>>();
        if blocked_labels.is_empty() {
            Ok(())
        } else {
            Err(TaintViolation {
                sink,
                labels: blocked_labels,
                recent_sources: self.recent_sources.clone(),
            })
        }
    }

    pub fn is_clean(&self) -> bool {
        self.labels.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaintViolation {
    pub sink: TaintSink,
    pub labels: Vec<TaintLabel>,
    pub recent_sources: Vec<TaintSource>,
}

impl fmt::Display for TaintViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sources = if self.recent_sources.is_empty() {
            "an unknown source".to_string()
        } else {
            self.recent_sources
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        write!(
            f,
            "Refusing to use {} because this turn includes recent untrusted content from {}. Ask the user to confirm the next action in a new message if they want to proceed.",
            self.sink.as_str(),
            sources
        )
    }
}

impl std::error::Error for TaintViolation {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_adds_labels_and_sources_deterministically() {
        let mut state = TaintState::default();
        state.apply(TaintEffect::Mark {
            label: TaintLabel::ExternalContent,
            source: TaintSource::McpTool,
        });
        state.apply(TaintEffect::Mark {
            label: TaintLabel::WorkspaceContent,
            source: TaintSource::ReadFile,
        });

        assert_eq!(
            state.labels().iter().copied().collect::<Vec<_>>(),
            vec![TaintLabel::WorkspaceContent, TaintLabel::ExternalContent]
        );
        assert_eq!(
            state.recent_sources(),
            &[TaintSource::McpTool, TaintSource::ReadFile]
        );
    }

    #[test]
    fn repeated_mark_does_not_repeat_same_source_back_to_back() {
        let mut state = TaintState::default();
        state.apply(TaintEffect::Mark {
            label: TaintLabel::WorkspaceContent,
            source: TaintSource::ReadFile,
        });
        state.apply(TaintEffect::Mark {
            label: TaintLabel::WorkspaceContent,
            source: TaintSource::ReadFile,
        });

        assert_eq!(state.recent_sources(), &[TaintSource::ReadFile]);
    }

    #[test]
    fn reset_clears_labels_and_sources() {
        let mut state = TaintState::default();
        state.apply(TaintEffect::Mark {
            label: TaintLabel::AgentContent,
            source: TaintSource::AgentResult,
        });

        state.apply(TaintEffect::Reset);

        assert!(state.is_clean());
        assert!(state.recent_sources().is_empty());
    }

    #[test]
    fn shell_exec_blocks_any_active_label() {
        let mut state = TaintState::default();
        state.apply(TaintEffect::Mark {
            label: TaintLabel::WorkspaceContent,
            source: TaintSource::ReadFile,
        });

        let violation = state
            .check_sink(TaintSink::shell_exec())
            .expect_err("shell should be blocked");
        assert_eq!(violation.labels, vec![TaintLabel::WorkspaceContent]);
        assert_eq!(violation.recent_sources, vec![TaintSource::ReadFile]);
    }

    #[test]
    fn external_dispatch_blocks_external_and_agent_content_only() {
        let mut workspace_only = TaintState::default();
        workspace_only.apply(TaintEffect::Mark {
            label: TaintLabel::WorkspaceContent,
            source: TaintSource::ReadFile,
        });
        assert!(
            workspace_only
                .check_sink(TaintSink::external_dispatch())
                .is_ok()
        );

        let mut external = TaintState::default();
        external.apply(TaintEffect::Mark {
            label: TaintLabel::ExternalContent,
            source: TaintSource::McpTool,
        });
        assert!(external.check_sink(TaintSink::external_dispatch()).is_err());
    }

    #[test]
    fn violation_messages_include_sink_and_sources() {
        let mut state = TaintState::default();
        state.apply(TaintEffect::Mark {
            label: TaintLabel::ExternalContent,
            source: TaintSource::WebSearch,
        });

        let message = state
            .check_sink(TaintSink::agent_forward())
            .expect_err("agent forwarding should be blocked")
            .to_string();

        assert!(message.contains("agent forwarding"));
        assert!(message.contains("web_search"));
    }
}
