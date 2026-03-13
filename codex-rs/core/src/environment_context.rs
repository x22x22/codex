use crate::codex::TurnContext;
use crate::model_visible_context::ContextualUserContextRole;
use crate::model_visible_context::ContextualUserFragment;
use crate::model_visible_context::ContextualUserFragmentMarkers;
use crate::model_visible_context::ModelVisibleContextFragment;
use crate::model_visible_context::TurnContextDiffFragment;
use crate::model_visible_context::TurnContextDiffParams;
use crate::shell::Shell;
use codex_protocol::protocol::ENVIRONMENT_CONTEXT_CLOSE_TAG;
use codex_protocol::protocol::ENVIRONMENT_CONTEXT_OPEN_TAG;
use codex_protocol::protocol::TurnContextItem;
use codex_protocol::protocol::TurnContextNetworkItem;
use serde::Deserialize;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename = "environment_context", rename_all = "snake_case")]
pub(crate) struct EnvironmentContext {
    pub cwd: Option<PathBuf>,
    pub shell: Shell,
    pub current_date: Option<String>,
    pub timezone: Option<String>,
    pub network: Option<NetworkContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct NetworkContext {
    allowed_domains: Vec<String>,
    denied_domains: Vec<String>,
}

impl EnvironmentContext {
    const MARKERS: ContextualUserFragmentMarkers = ContextualUserFragmentMarkers::new(
        ENVIRONMENT_CONTEXT_OPEN_TAG,
        ENVIRONMENT_CONTEXT_CLOSE_TAG,
    );

    pub fn new(
        cwd: Option<PathBuf>,
        shell: Shell,
        current_date: Option<String>,
        timezone: Option<String>,
        network: Option<NetworkContext>,
    ) -> Self {
        Self {
            cwd,
            shell,
            current_date,
            timezone,
            network,
        }
    }

    /// Compares two environment contexts, ignoring the shell. Useful when
    /// comparing turn to turn, since the initial environment_context will
    /// include the shell, and then it is not configurable from turn to turn.
    pub fn equals_except_shell(&self, other: &EnvironmentContext) -> bool {
        let EnvironmentContext {
            cwd,
            current_date,
            timezone,
            network,
            shell: _,
        } = other;
        self.cwd == *cwd
            && self.current_date == *current_date
            && self.timezone == *timezone
            && self.network == *network
    }

    fn network_from_turn_context(turn_context: &TurnContext) -> Option<NetworkContext> {
        let network = turn_context
            .config
            .config_layer_stack
            .requirements()
            .network
            .as_ref()?;

        Some(NetworkContext {
            allowed_domains: network.allowed_domains.clone().unwrap_or_default(),
            denied_domains: network.denied_domains.clone().unwrap_or_default(),
        })
    }

    fn network_from_turn_context_item(
        turn_context_item: &TurnContextItem,
    ) -> Option<NetworkContext> {
        let TurnContextNetworkItem {
            allowed_domains,
            denied_domains,
        } = turn_context_item.network.as_ref()?;
        Some(NetworkContext {
            allowed_domains: allowed_domains.clone(),
            denied_domains: denied_domains.clone(),
        })
    }
}

impl ModelVisibleContextFragment for EnvironmentContext {
    type Role = ContextualUserContextRole;

    fn render_text(&self) -> String {
        let mut lines = Vec::new();
        if let Some(cwd) = &self.cwd {
            lines.push(format!("  <cwd>{}</cwd>", cwd.to_string_lossy()));
        }

        let shell_name = self.shell.name();
        lines.push(format!("  <shell>{shell_name}</shell>"));
        if let Some(current_date) = &self.current_date {
            lines.push(format!("  <current_date>{current_date}</current_date>"));
        }
        if let Some(timezone) = &self.timezone {
            lines.push(format!("  <timezone>{timezone}</timezone>"));
        }
        match &self.network {
            Some(network) => {
                lines.push("  <network enabled=\"true\">".to_string());
                for allowed in &network.allowed_domains {
                    lines.push(format!("    <allowed>{allowed}</allowed>"));
                }
                for denied in &network.denied_domains {
                    lines.push(format!("    <denied>{denied}</denied>"));
                }
                lines.push("  </network>".to_string());
            }
            None => {
                // TODO(mbolin): Include this line if it helps the model.
                // lines.push("  <network enabled=\"false\" />".to_string());
            }
        }
        Self::MARKERS.wrap_body(lines.join("\n"))
    }
}

impl ContextualUserFragment for EnvironmentContext {
    fn markers() -> Option<ContextualUserFragmentMarkers> {
        Some(Self::MARKERS)
    }
}

impl TurnContextDiffFragment for EnvironmentContext {
    fn build(
        turn_context: &TurnContext,
        reference_context_item: Option<&TurnContextItem>,
        params: &TurnContextDiffParams<'_>,
    ) -> Option<Self> {
        let current_network = Self::network_from_turn_context(turn_context);
        let current_context = Self::new(
            Some(turn_context.cwd.clone()),
            params.shell.clone(),
            turn_context.current_date.clone(),
            turn_context.timezone.clone(),
            current_network.clone(),
        );

        let Some(previous) = reference_context_item else {
            return Some(current_context);
        };

        let previous_network = Self::network_from_turn_context_item(previous);
        let previous_context = Self::new(
            Some(previous.cwd.clone()),
            params.shell.clone(),
            previous.current_date.clone(),
            previous.timezone.clone(),
            previous_network.clone(),
        );

        if previous_context.equals_except_shell(&current_context) {
            return None;
        }

        let cwd = if previous.cwd != turn_context.cwd {
            Some(turn_context.cwd.clone())
        } else {
            None
        };
        let network = if previous_network != current_network {
            current_network
        } else {
            previous_network
        };

        Some(Self::new(
            cwd,
            params.shell.clone(),
            turn_context.current_date.clone(),
            turn_context.timezone.clone(),
            network,
        ))
    }
}

#[cfg(test)]
#[path = "environment_context_tests.rs"]
mod tests;
