use crate::tools::spec::ToolsConfig;

const CORE_BASE_INSTRUCTIONS: &str = include_str!("../templates/base_instructions/core.md");
const APPLY_PATCH_INSTRUCTIONS: &str =
    include_str!("../templates/base_instructions/capabilities/apply_patch.md");
const UNIFIED_EXEC_INSTRUCTIONS: &str =
    include_str!("../templates/base_instructions/capabilities/unified_exec.md");
const UPDATE_PLAN_INSTRUCTIONS: &str =
    include_str!("../templates/base_instructions/capabilities/update_plan.md");

pub(crate) fn compose_base_instructions(tools_config: &ToolsConfig) -> String {
    let mut sections = vec![CORE_BASE_INSTRUCTIONS.trim().to_string()];

    if tools_config.has_builtin_tool("exec_command") {
        sections.push(UNIFIED_EXEC_INSTRUCTIONS.trim().to_string());
    }
    if tools_config.has_builtin_tool("apply_patch") {
        sections.push(APPLY_PATCH_INSTRUCTIONS.trim().to_string());
    }
    if tools_config.has_builtin_tool("update_plan") {
        sections.push(UPDATE_PLAN_INSTRUCTIONS.trim().to_string());
    }

    sections.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::Feature;
    use crate::features::Features;
    use crate::models_manager::model_info::model_info_from_slug;
    use crate::tools::spec::ToolsConfigParams;
    use codex_protocol::protocol::SessionSource;

    fn tools_config() -> ToolsConfig {
        let mut features = Features::new();
        let _ = features.enable(Feature::ShellTool);
        let _ = features.enable(Feature::UnifiedExec);
        let _ = features.enable(Feature::ApplyPatchFreeform);
        let model_info = model_info_from_slug("gpt-5.2-codex");
        ToolsConfig::new(&ToolsConfigParams {
            model_info: &model_info,
            features: &features,
            web_search_mode: None,
            session_source: SessionSource::Cli,
        })
    }

    #[test]
    fn omits_unenabled_capability_sections() {
        let tools_config = tools_config().with_builtin_tools(Some(vec!["exec_command".to_string()]));

        let instructions = compose_base_instructions(&tools_config);

        assert!(instructions.contains("# Unified Exec"));
        assert!(!instructions.contains("# Apply Patch"));
        assert!(!instructions.contains("# Update Plan"));
    }

    #[test]
    fn includes_capability_sections_for_enabled_tools() {
        let tools_config = tools_config().with_builtin_tools(Some(vec![
            "exec_command".to_string(),
            "apply_patch".to_string(),
            "update_plan".to_string(),
        ]));

        let instructions = compose_base_instructions(&tools_config);

        assert!(instructions.contains("# Unified Exec"));
        assert!(instructions.contains("# Apply Patch"));
        assert!(instructions.contains("# Update Plan"));
    }
}
