use super::*;
use crate::config::test_config;
use pretty_assertions::assert_eq;

#[test]
fn reasoning_summaries_override_true_enables_support() {
    let model = model_info_from_slug("unknown-model");
    let mut config = test_config();
    config.model_supports_reasoning_summaries = Some(true);

    let updated = with_config_overrides(model.clone(), &config);
    let mut expected = model;
    expected.supports_reasoning_summaries = true;

    assert_eq!(updated, expected);
}

#[test]
fn reasoning_summaries_override_false_does_not_disable_support() {
    let mut model = model_info_from_slug("unknown-model");
    model.supports_reasoning_summaries = true;
    let mut config = test_config();
    config.model_supports_reasoning_summaries = Some(false);

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn reasoning_summaries_override_false_is_noop_when_model_is_false() {
    let model = model_info_from_slug("unknown-model");
    let mut config = test_config();
    config.model_supports_reasoning_summaries = Some(false);

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn experimental_supported_tools_are_merged_from_config() {
    let mut model = model_info_from_slug("unknown-model");
    model.experimental_supported_tools = vec!["grep_files".to_string()];
    let mut config = test_config();
    config.experimental_supported_tools = vec!["read_file".to_string(), "grep_files".to_string()];

    let updated = with_config_overrides(model, &config);

    assert_eq!(
        updated.experimental_supported_tools,
        vec!["grep_files".to_string(), "read_file".to_string()]
    );
}
