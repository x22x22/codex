pub(crate) mod config_rules;
mod env_var_dependencies;
pub mod injection;
pub(crate) mod invocation_utils;
pub mod loader;
pub mod manager;
pub mod model;
pub mod remote;
pub mod render;
pub mod system;

pub(crate) use invocation_utils::build_implicit_skill_path_indexes;
pub use manager::SkillsManager;
pub use model::SkillError;
pub use model::SkillLoadOutcome;
pub use model::SkillMetadata;
pub use model::SkillPolicy;
pub use model::filter_skill_load_outcome_for_product;
pub use render::render_skills_section;
