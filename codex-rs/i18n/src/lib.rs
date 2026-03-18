use std::collections::HashMap;
use std::sync::LazyLock;

use fluent_bundle::FluentArgs;
use fluent_bundle::FluentResource;
use fluent_bundle::concurrent::FluentBundle;
use include_dir::Dir;
use include_dir::include_dir;
use thiserror::Error;
use unic_langid::LanguageIdentifier;

const LOCALES_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/src/locales");
pub const DEFAULT_LOCALE: &str = "en-US";

type Bundle = FluentBundle<FluentResource>;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LocaleError {
    #[error("invalid locale `{0}`")]
    InvalidLocale(String),
}

static SUPPORTED_LOCALES: LazyLock<Vec<String>> = LazyLock::new(load_supported_locales);

static BUNDLES: LazyLock<HashMap<String, Bundle>> = LazyLock::new(load_bundles);

pub fn resolve_locale(config_locale: Option<&str>) -> Result<String, LocaleError> {
    resolve_locale_with_system(config_locale, sys_locale::get_locale().as_deref())
}

pub fn resolve_locale_with_system(
    config_locale: Option<&str>,
    system_locale: Option<&str>,
) -> Result<String, LocaleError> {
    if let Some(config_locale) = config_locale {
        let requested = parse_locale(config_locale)
            .ok_or_else(|| LocaleError::InvalidLocale(config_locale.trim().to_string()))?;
        return Ok(negotiate_locale(Some(&requested)));
    }

    let requested = system_locale.and_then(parse_locale);
    Ok(negotiate_locale(requested.as_ref()))
}

pub fn format_message(locale: &str, message_id: &str, args: &[(&str, &str)]) -> Option<String> {
    let bundle = BUNDLES
        .get(locale)
        .or_else(|| BUNDLES.get(DEFAULT_LOCALE))?;
    render_message(bundle, message_id, args).or_else(|| {
        BUNDLES
            .get(DEFAULT_LOCALE)
            .and_then(|fallback| render_message(fallback, message_id, args))
    })
}

fn parse_locale(value: &str) -> Option<LanguageIdentifier> {
    normalize_locale_candidate(value)?.parse().ok()
}

fn normalize_locale_candidate(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let without_modifier = trimmed.split('@').next()?;
    let without_encoding = without_modifier.split('.').next()?;
    let normalized = without_encoding.replace('_', "-");
    (!normalized.is_empty()).then_some(normalized)
}

fn negotiate_locale(requested: Option<&LanguageIdentifier>) -> String {
    let Some(requested) = requested else {
        return DEFAULT_LOCALE.to_string();
    };

    let exact = requested.to_string();
    if SUPPORTED_LOCALES.iter().any(|locale| locale == &exact) {
        return exact;
    }

    let requested_language = requested.language.as_str();
    SUPPORTED_LOCALES
        .iter()
        .find(|locale| locale.split('-').next() == Some(requested_language))
        .cloned()
        .unwrap_or_else(|| DEFAULT_LOCALE.to_string())
}

fn load_supported_locales() -> Vec<String> {
    let mut locales = LOCALES_DIR
        .dirs()
        .filter_map(|dir| dir.path().file_name())
        .filter_map(|name| name.to_str())
        .map(ToString::to_string)
        .collect::<Vec<String>>();
    locales.sort();
    locales
}

fn load_bundles() -> HashMap<String, Bundle> {
    let mut bundles = HashMap::new();

    for dir in LOCALES_DIR.dirs() {
        let Some(locale_name) = dir.path().file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(language_identifier) = parse_locale(locale_name) else {
            continue;
        };
        let mut bundle = FluentBundle::new_concurrent(vec![language_identifier]);
        bundle.set_use_isolating(false);

        for file in dir.files() {
            if file.path().extension().and_then(|value| value.to_str()) != Some("ftl") {
                continue;
            }

            let Some(contents) = file.contents_utf8() else {
                panic!("locale files must be valid UTF-8");
            };
            let Ok(resource) = FluentResource::try_new(contents.to_string()) else {
                panic!("locale files must be valid Fluent resources");
            };
            if bundle.add_resource(resource).is_err() {
                panic!("locale resources must not define duplicate messages");
            }
        }

        bundles.insert(locale_name.to_string(), bundle);
    }

    bundles
}

fn render_message(bundle: &Bundle, message_id: &str, args: &[(&str, &str)]) -> Option<String> {
    let message = bundle.get_message(message_id)?;
    let pattern = message.value()?;
    let mut errors = Vec::new();
    let fluent_args = (!args.is_empty()).then(|| {
        let mut fluent_args = FluentArgs::new();
        for (name, value) in args {
            fluent_args.set(*name, *value);
        }
        fluent_args
    });

    Some(
        bundle
            .format_pattern(pattern, fluent_args.as_ref(), &mut errors)
            .into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn explicit_locale_overrides_system_locale() {
        assert_eq!(
            resolve_locale_with_system(Some("zh-CN"), Some("en-US")).expect("resolve locale"),
            "zh-CN"
        );
    }

    #[test]
    fn unsupported_explicit_locale_falls_back_to_default() {
        assert_eq!(
            resolve_locale_with_system(Some("fr-FR"), Some("zh-CN")).expect("resolve locale"),
            DEFAULT_LOCALE
        );
    }

    #[test]
    fn supported_system_locale_is_used_when_config_is_missing() {
        assert_eq!(
            resolve_locale_with_system(None, Some("zh")).expect("resolve locale"),
            "zh-CN"
        );
    }

    #[test]
    fn unsupported_system_locale_falls_back_to_default() {
        assert_eq!(
            resolve_locale_with_system(None, Some("fr-FR")).expect("resolve locale"),
            DEFAULT_LOCALE
        );
    }

    #[test]
    fn invalid_explicit_locale_is_rejected() {
        assert_eq!(
            resolve_locale_with_system(Some("not a locale"), None),
            Err(LocaleError::InvalidLocale("not a locale".to_string()))
        );
    }

    #[test]
    fn formats_english_message() {
        assert_eq!(
            format_message(
                "en-US",
                "approval-question-google-docs-create-document",
                &[("connector_name", "Google Docs")]
            ),
            Some("Allow Google Docs to create a document?".to_string())
        );
    }

    #[test]
    fn formats_chinese_message() {
        assert_eq!(
            format_message(
                "zh-CN",
                "approval-question-google-docs-create-document",
                &[("connector_name", "Google Docs")]
            ),
            Some("允许 Google Docs 创建文档吗？".to_string())
        );
    }
}
