use crate::config_wizard_recommendation::ConfigWizardRecommendation;
use crate::config_wizard_recommendation::detect_config_wizard_recommendation_for_codex_home;
use anyhow::Result;
use codex_core::config::Config;
use codex_core::config::edit::ConfigEdit as CoreConfigEdit;
use codex_core::config::edit::set_path_toml_edit;
use codex_protocol::config_types::SandboxMode;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::SandboxPolicy;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::path::Path;
use std::path::PathBuf;
use toml::Value as TomlValue;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfigWizardAccessMode {
    ReadOnly,
    WorkspaceWrite,
    FullAccess,
}

impl ConfigWizardAccessMode {
    fn from_sandbox_policy(policy: &SandboxPolicy) -> Self {
        match policy {
            SandboxPolicy::DangerFullAccess => Self::FullAccess,
            SandboxPolicy::ReadOnly { .. } => Self::ReadOnly,
            SandboxPolicy::WorkspaceWrite { .. } | SandboxPolicy::ExternalSandbox { .. } => {
                Self::WorkspaceWrite
            }
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "Read only",
            Self::WorkspaceWrite => "Workspace write",
            Self::FullAccess => "Full access",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::ReadOnly => {
                "Writes `approval_policy = \"on-request\"` and `sandbox_mode = \"read-only\"`."
            }
            Self::WorkspaceWrite => {
                "Writes `approval_policy = \"on-request\"`, `sandbox_mode = \"workspace-write\"`, and lets you configure `[sandbox_workspace_write]`."
            }
            Self::FullAccess => {
                "Writes `approval_policy = \"never\"` and `sandbox_mode = \"danger-full-access\"`."
            }
        }
    }

    fn approval_policy(self) -> AskForApproval {
        match self {
            Self::ReadOnly | Self::WorkspaceWrite => AskForApproval::OnRequest,
            Self::FullAccess => AskForApproval::Never,
        }
    }

    fn sandbox_mode(self) -> SandboxMode {
        match self {
            Self::ReadOnly => SandboxMode::ReadOnly,
            Self::WorkspaceWrite => SandboxMode::WorkspaceWrite,
            Self::FullAccess => SandboxMode::DangerFullAccess,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfigWizardTextStep {
    WritableRoots,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfigWizardWorkspaceWriteOption {
    NetworkAccess,
    ExcludeTmpdirEnvVar,
    ExcludeSlashTmp,
}

impl ConfigWizardWorkspaceWriteOption {
    pub(crate) const ALL: [Self; 3] = [
        Self::NetworkAccess,
        Self::ExcludeTmpdirEnvVar,
        Self::ExcludeSlashTmp,
    ];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::NetworkAccess => "network-access",
            Self::ExcludeTmpdirEnvVar => "exclude-tmpdir-env-var",
            Self::ExcludeSlashTmp => "exclude-slash-tmp",
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|option| option.key() == key)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NetworkAccess => "Allow network access",
            Self::ExcludeTmpdirEnvVar => "Exclude $TMPDIR",
            Self::ExcludeSlashTmp => "Exclude /tmp",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::NetworkAccess => "Sets `[sandbox_workspace_write].network_access = true`.",
            Self::ExcludeTmpdirEnvVar => {
                "Sets `[sandbox_workspace_write].exclude_tmpdir_env_var = true`."
            }
            Self::ExcludeSlashTmp => "Sets `[sandbox_workspace_write].exclude_slash_tmp = true`.",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ConfigWizardState {
    pub(crate) access_mode: ConfigWizardAccessMode,
    pub(crate) writable_roots: Vec<AbsolutePathBuf>,
    pub(crate) network_access: bool,
    pub(crate) exclude_tmpdir_env_var: bool,
    pub(crate) exclude_slash_tmp: bool,
    recommendation: Option<ConfigWizardRecommendation>,
}

#[derive(Clone, Debug)]
pub(crate) struct ConfigWizardApplyRequest {
    pub(crate) edits: Vec<CoreConfigEdit>,
    pub(crate) summary: String,
}

impl ConfigWizardState {
    pub(crate) fn detect_with_config(config: &Config) -> Self {
        let recommendation = detect_config_wizard_recommendation_for_codex_home(
            config.codex_home.as_path(),
            Some(config.cwd.as_path()),
        );
        let mut state = Self::new(recommendation.clone());
        state.apply_current_policy(
            config.permissions.sandbox_policy.get(),
            config.codex_home.as_path(),
        );
        if let Some(recommendation) = recommendation.as_ref() {
            if !matches!(
                config.permissions.sandbox_policy.get(),
                SandboxPolicy::WorkspaceWrite { .. }
            ) {
                state.network_access = recommendation.network_access;
            }
            state.extend_writable_roots(&recommendation.directories);
        }
        state
    }

    fn new(recommendation: Option<ConfigWizardRecommendation>) -> Self {
        Self {
            access_mode: ConfigWizardAccessMode::WorkspaceWrite,
            writable_roots: Vec::new(),
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
            recommendation,
        }
    }

    fn apply_current_policy(&mut self, policy: &SandboxPolicy, codex_home: &Path) {
        self.access_mode = ConfigWizardAccessMode::from_sandbox_policy(policy);
        match policy {
            SandboxPolicy::WorkspaceWrite {
                writable_roots,
                network_access,
                exclude_tmpdir_env_var,
                exclude_slash_tmp,
                ..
            } => {
                self.writable_roots = visible_writable_roots(writable_roots, codex_home);
                self.network_access = *network_access;
                self.exclude_tmpdir_env_var = *exclude_tmpdir_env_var;
                self.exclude_slash_tmp = *exclude_slash_tmp;
            }
            SandboxPolicy::ExternalSandbox { network_access } => {
                self.network_access = network_access.is_enabled();
            }
            SandboxPolicy::ReadOnly { .. } | SandboxPolicy::DangerFullAccess => {}
        }
    }

    fn extend_writable_roots(&mut self, roots: &[AbsolutePathBuf]) {
        for root in roots {
            if !self.writable_roots.contains(root) {
                self.writable_roots.push(root.clone());
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn test_state() -> Self {
        Self::new(None)
    }

    pub(crate) fn uses_workspace_write(&self) -> bool {
        self.access_mode == ConfigWizardAccessMode::WorkspaceWrite
    }

    pub(crate) fn apply_text_step(
        &mut self,
        step: ConfigWizardTextStep,
        value: String,
    ) -> Result<()> {
        match step {
            ConfigWizardTextStep::WritableRoots => {
                self.writable_roots = parse_writable_roots(&value)?;
            }
        }
        Ok(())
    }

    pub(crate) fn set_workspace_write_options(
        &mut self,
        selected: &[ConfigWizardWorkspaceWriteOption],
    ) {
        self.network_access = selected.contains(&ConfigWizardWorkspaceWriteOption::NetworkAccess);
        self.exclude_tmpdir_env_var =
            selected.contains(&ConfigWizardWorkspaceWriteOption::ExcludeTmpdirEnvVar);
        self.exclude_slash_tmp =
            selected.contains(&ConfigWizardWorkspaceWriteOption::ExcludeSlashTmp);
    }

    pub(crate) fn selected_workspace_write_options(&self) -> Vec<ConfigWizardWorkspaceWriteOption> {
        [
            (
                self.network_access,
                ConfigWizardWorkspaceWriteOption::NetworkAccess,
            ),
            (
                self.exclude_tmpdir_env_var,
                ConfigWizardWorkspaceWriteOption::ExcludeTmpdirEnvVar,
            ),
            (
                self.exclude_slash_tmp,
                ConfigWizardWorkspaceWriteOption::ExcludeSlashTmp,
            ),
        ]
        .into_iter()
        .filter(|(enabled, _)| *enabled)
        .map(|(_, option)| option)
        .collect()
    }

    pub(crate) fn build_apply_request(&self) -> Result<ConfigWizardApplyRequest> {
        let mut edits = vec![
            CoreConfigEdit::ClearPath {
                segments: vec!["default_permissions".to_string()],
            },
            CoreConfigEdit::ClearPath {
                segments: vec!["permissions".to_string()],
            },
            CoreConfigEdit::ClearPath {
                segments: vec!["sandbox_workspace_write".to_string()],
            },
        ];

        for (key, value) in self.config_entries() {
            edits.push(set_path_toml_edit(vec![key.to_string()], value)?);
        }

        Ok(ConfigWizardApplyRequest {
            edits,
            summary: "Saved `/setup-sandbox` settings to config.toml.".to_string(),
        })
    }

    pub(crate) fn preview_toml(&self) -> String {
        let mut root = toml::map::Map::new();
        for (key, value) in self.config_entries() {
            root.insert(key.to_string(), value);
        }
        toml::to_string_pretty(&TomlValue::Table(root))
            .unwrap_or_else(|_| "# Failed to render preview".to_string())
    }

    pub(crate) fn access_mode_subtitle(&self) -> String {
        let mut subtitle =
            "This flow writes sandbox settings to config.toml. Current session settings stay selected by default."
                .to_string();
        if let Some(summary) = self.recommendation_summary() {
            subtitle.push_str("\n\n");
            subtitle.push_str(&summary);
        }
        subtitle
    }

    pub(crate) fn workspace_write_subtitle(&self) -> String {
        let mut subtitle =
            "These map directly to `[sandbox_workspace_write]`. Current session settings are preselected, and the next step lets you review directories you work in."
                .to_string();
        if let Some(recommendation) = self.recommendation.as_ref()
            && recommendation.workspace_write_sessions > 0
        {
            subtitle.push_str(&format!(
                "\n\nNetwork was {} in {}/{} recent workspace-write sessions.",
                if recommendation.network_access {
                    "enabled"
                } else {
                    "disabled"
                },
                recommendation.workspace_write_network_sessions,
                recommendation.workspace_write_sessions
            ));
        }
        subtitle
    }

    pub(crate) fn prompt_title(step: ConfigWizardTextStep) -> &'static str {
        match step {
            ConfigWizardTextStep::WritableRoots => "Directories You Work In",
        }
    }

    pub(crate) fn prompt_placeholder(step: ConfigWizardTextStep) -> &'static str {
        match step {
            ConfigWizardTextStep::WritableRoots => {
                "One absolute path per line, for example:\n/Users/me/code/openai\n/Users/me/code/infra"
            }
        }
    }

    pub(crate) fn prompt_initial_value(&self, step: ConfigWizardTextStep) -> Option<String> {
        match step {
            ConfigWizardTextStep::WritableRoots => {
                if self.writable_roots.is_empty() {
                    None
                } else {
                    Some(
                        self.writable_roots
                            .iter()
                            .map(|path| path.display().to_string())
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                }
            }
        }
    }

    pub(crate) fn prompt_context(&self, step: ConfigWizardTextStep) -> Option<String> {
        match step {
            ConfigWizardTextStep::WritableRoots => {
                let mut context =
                    "Add other absolute directories you regularly want Codex to edit. The current workspace is already writable, and any directories from your current sandbox settings stay prefilled. Press Enter to continue, or Esc to go back."
                        .to_string();
                if let Some(summary) = self.recommendation_summary()
                    && !self.writable_roots.is_empty()
                {
                    context.push_str("\n\n");
                    context.push_str(&summary);
                    context.push_str(
                        " Recent-session suggestions are included below so you can keep, remove, or add directories.",
                    );
                }
                Some(context)
            }
        }
    }

    pub(crate) fn recommendation_summary(&self) -> Option<String> {
        self.recommendation
            .as_ref()
            .map(ConfigWizardRecommendation::summary)
    }

    fn sandbox_workspace_write_toml(&self) -> Option<toml::map::Map<String, TomlValue>> {
        if !self.uses_workspace_write() {
            return None;
        }

        let mut table = toml::map::Map::new();
        table.insert(
            "writable_roots".to_string(),
            TomlValue::Array(
                self.writable_roots
                    .iter()
                    .map(|path| TomlValue::String(path.display().to_string()))
                    .collect(),
            ),
        );
        table.insert(
            "network_access".to_string(),
            TomlValue::Boolean(self.network_access),
        );
        table.insert(
            "exclude_tmpdir_env_var".to_string(),
            TomlValue::Boolean(self.exclude_tmpdir_env_var),
        );
        table.insert(
            "exclude_slash_tmp".to_string(),
            TomlValue::Boolean(self.exclude_slash_tmp),
        );
        Some(table)
    }

    fn config_entries(&self) -> Vec<(&'static str, TomlValue)> {
        let mut entries = vec![
            (
                "approval_policy",
                TomlValue::String(self.access_mode.approval_policy().to_string()),
            ),
            (
                "sandbox_mode",
                TomlValue::String(self.access_mode.sandbox_mode().to_string()),
            ),
        ];

        if let Some(sandbox_workspace_write) = self.sandbox_workspace_write_toml() {
            entries.push((
                "sandbox_workspace_write",
                TomlValue::Table(sandbox_workspace_write),
            ));
        }

        entries
    }
}

fn parse_writable_roots(value: &str) -> Result<Vec<AbsolutePathBuf>> {
    let mut roots = Vec::new();
    for line in value.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let path = PathBuf::from(trimmed);
        if !path.is_absolute() {
            return Err(anyhow::anyhow!(
                "directory `{trimmed}` must be an absolute path"
            ));
        }

        let absolute = AbsolutePathBuf::try_from(path).map_err(|err| {
            anyhow::anyhow!("directory `{trimmed}` is not a valid absolute path: {err}")
        })?;
        if !roots.contains(&absolute) {
            roots.push(absolute);
        }
    }
    Ok(roots)
}

fn visible_writable_roots(
    writable_roots: &[AbsolutePathBuf],
    codex_home: &Path,
) -> Vec<AbsolutePathBuf> {
    writable_roots
        .iter()
        .filter(|path| !is_hidden_codex_directory(path.as_path(), codex_home))
        .cloned()
        .collect()
}

fn is_hidden_codex_directory(path: &Path, codex_home: &Path) -> bool {
    path.starts_with(codex_home.join("worktrees")) || path.starts_with(codex_home.join("memories"))
}

#[cfg(test)]
#[path = "config_wizard_state_tests.rs"]
mod tests;
