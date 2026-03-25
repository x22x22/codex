pub(crate) mod config_rules;
mod env_var_dependencies;
pub mod injection;
pub(crate) mod invocation_utils;
pub mod loader;
pub mod manager;
pub mod remote;
pub mod system;

pub(crate) mod model {
    pub(crate) use codex_skills::SkillDependencies;
    pub(crate) use codex_skills::SkillError;
    pub(crate) use codex_skills::SkillInterface;
    pub(crate) use codex_skills::SkillLoadOutcome;
    pub(crate) use codex_skills::SkillManagedNetworkOverride;
    pub(crate) use codex_skills::SkillMetadata;
    pub(crate) use codex_skills::SkillPolicy;
    pub(crate) use codex_skills::SkillToolDependency;
    pub(crate) use codex_skills::filter_skill_load_outcome_for_product;
}

mod render {
    pub(crate) use codex_skills::render_skills_section;
}

pub(crate) use env_var_dependencies::collect_env_var_dependencies;
pub(crate) use env_var_dependencies::resolve_skill_dependencies_for_turn;
pub(crate) use injection::SkillInjections;
pub(crate) use injection::build_skill_injections;
pub(crate) use injection::collect_explicit_skill_mentions;
pub(crate) use invocation_utils::build_implicit_skill_path_indexes;
pub(crate) use invocation_utils::maybe_emit_implicit_skill_invocation;
pub(crate) use manager::SkillsManager;
pub(crate) use model::SkillError;
pub(crate) use model::SkillLoadOutcome;
pub(crate) use model::SkillMetadata;
pub(crate) use model::SkillPolicy;
pub(crate) use model::filter_skill_load_outcome_for_product;
pub(crate) use render::render_skills_section;
