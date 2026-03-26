use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use codex_core::AuthManager;
use codex_core::auth::AuthCredentialsStoreMode;
use codex_core::auth::login_with_api_key;
use codex_core::auth::read_openai_api_key_from_env;
use codex_login::CreatedApiKey;
use codex_login::OPENAI_API_KEY_ENV_VAR;
use codex_login::PendingCreateApiKey;
use codex_login::start_create_api_key as start_create_api_key_flow;
use codex_protocol::config_types::ForcedLoginMethod;
use ratatui::style::Stylize;
use ratatui::text::Line;

use super::ChatWidget;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::clipboard_text;
use crate::history_cell;
use crate::history_cell::PlainHistoryCell;

impl ChatWidget {
    pub(crate) fn start_create_api_key(&mut self) {
        match start_create_api_key_command(
            self.app_event_tx.clone(),
            self.auth_manager.clone(),
            self.config.codex_home.clone(),
            self.config.forced_login_method,
        ) {
            Ok(start_message) => {
                self.add_to_history(start_message);
                self.request_redraw();
            }
            Err(err) => {
                self.add_error_message(err);
            }
        }
    }
}

fn start_create_api_key_command(
    app_event_tx: AppEventSender,
    auth_manager: Arc<AuthManager>,
    codex_home: PathBuf,
    forced_login_method: Option<ForcedLoginMethod>,
) -> Result<PlainHistoryCell, String> {
    if read_openai_api_key_from_env().is_some() {
        return Ok(existing_shell_api_key_message());
    }

    let session = start_create_api_key_flow()
        .map_err(|err| format!("Failed to start API key creation: {err}"))?;
    let browser_opened = session.open_browser();
    let start_message = continue_in_browser_message(
        session.auth_url(),
        session.callback_port(),
        browser_opened,
    );

    let app_event_tx_for_task = app_event_tx;
    tokio::spawn(async move {
        let cell = complete_command(session, codex_home, forced_login_method, auth_manager).await;
        app_event_tx_for_task.send(AppEvent::InsertHistoryCell(Box::new(cell)));
    });

    Ok(start_message)
}

fn existing_shell_api_key_message() -> PlainHistoryCell {
    history_cell::new_info_event(
        format!(
            "{OPENAI_API_KEY_ENV_VAR} is already set in this Codex session; skipping API key creation."
        ),
        Some(format!(
            "This Codex session already inherited {OPENAI_API_KEY_ENV_VAR} from its shell environment. Unset it and run /create-api-key again if you want Codex to create a different key."
        )),
    )
}

fn continue_in_browser_message(
    auth_url: &str,
    callback_port: u16,
    browser_opened: bool,
) -> PlainHistoryCell {
    let mut lines = vec![
        vec![
            "• ".dim(),
            "Finish API key creation via your browser.".into(),
        ]
        .into(),
        "".into(),
    ];

    if browser_opened {
        lines.push(
            "  Codex tried to open this link for you."
                .dark_gray()
                .into(),
        );
    } else {
        lines.push(
            "  Codex couldn't auto-open your browser, but the API key creation flow is still waiting."
                .dark_gray()
                .into(),
        );
    }
    lines.push("".into());
    lines.push("  Open the following link to authenticate:".into());
    lines.push("".into());
    lines.push(Line::from(vec![
        "  ".into(),
        auth_url.to_string().cyan().underlined(),
    ]));
    lines.push("".into());
    lines.push(
        format!("  Codex will display the new {OPENAI_API_KEY_ENV_VAR} here and copy it to your clipboard.")
        .dark_gray()
        .into(),
    );
    lines.push("".into());
    lines.push(
        format!(
            "  On a remote or headless machine, forward localhost:{callback_port} back to this Codex host before opening the link."
        )
        .dark_gray()
        .into(),
    );

    PlainHistoryCell::new(lines)
}

async fn complete_command(
    session: PendingCreateApiKey,
    codex_home: PathBuf,
    forced_login_method: Option<ForcedLoginMethod>,
    auth_manager: Arc<AuthManager>,
) -> PlainHistoryCell {
    let provisioned = match session.finish().await {
        Ok(provisioned) => provisioned,
        Err(err) => {
            return history_cell::new_error_event(format!("API key creation failed: {err}"));
        }
    };
    let copy_result = clipboard_text::copy_text_to_clipboard(&provisioned.project_api_key);

    success_cell(
        &provisioned,
        copy_result,
        live_apply_api_key(
            forced_login_method,
            &codex_home,
            &provisioned.project_api_key,
            auth_manager,
        ),
    )
}

fn live_apply_api_key(
    forced_login_method: Option<ForcedLoginMethod>,
    codex_home: &Path,
    api_key: &str,
    auth_manager: Arc<AuthManager>,
) -> LiveApplyOutcome {
    if matches!(forced_login_method, Some(ForcedLoginMethod::Chatgpt)) {
        return LiveApplyOutcome::Skipped(format!(
            "Created {OPENAI_API_KEY_ENV_VAR}, but left this session unchanged because ChatGPT login is required here."
        ));
    }

    match login_with_api_key(codex_home, api_key, AuthCredentialsStoreMode::Ephemeral) {
        Ok(()) => {
            auth_manager.reload();
            LiveApplyOutcome::Applied
        }
        Err(err) => LiveApplyOutcome::Failed(err.to_string()),
    }
}

fn success_cell(
    provisioned: &CreatedApiKey,
    copy_result: Result<(), String>,
    live_apply_outcome: LiveApplyOutcome,
) -> PlainHistoryCell {
    let organization = provisioned
        .organization_title
        .clone()
        .unwrap_or_else(|| provisioned.organization_id.clone());
    let project = provisioned
        .default_project_title
        .clone()
        .unwrap_or_else(|| provisioned.default_project_id.clone());
    let masked_api_key = mask_api_key(&provisioned.project_api_key);
    let copy_status = match copy_result {
        Ok(()) => "Copied the full key to your clipboard.".to_string(),
        Err(err) => format!("Could not copy the key to your clipboard: {err}"),
    };
    let live_apply_status = match live_apply_outcome {
        LiveApplyOutcome::Applied => Some(
            "Updated this session to use the newly created API key without touching auth.json."
                .to_string(),
        ),
        LiveApplyOutcome::Skipped(reason) => Some(reason),
        LiveApplyOutcome::Failed(err) => Some(format!(
            "Created {OPENAI_API_KEY_ENV_VAR}, but could not hot-apply it in this session: {err}",
        )),
    };
    let hint = Some(match live_apply_status {
        Some(live_apply_status) => format!("{copy_status} {live_apply_status}"),
        None => copy_status,
    });

    history_cell::new_info_event(
        format!(
            "Created an API key for {organization} / {project}: {masked_api_key}"
        ),
        hint,
    )
}

fn mask_api_key(api_key: &str) -> String {
    const UNMASKED_PREFIX_LEN: usize = 8;
    const UNMASKED_SUFFIX_LEN: usize = 4;

    if api_key.len() <= UNMASKED_PREFIX_LEN + UNMASKED_SUFFIX_LEN {
        return api_key.to_string();
    }

    format!(
        "{}...{}",
        &api_key[..UNMASKED_PREFIX_LEN],
        &api_key[api_key.len() - UNMASKED_SUFFIX_LEN..]
    )
}

enum LiveApplyOutcome {
    Applied,
    Skipped(String),
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_cell::HistoryCell;
    use insta::assert_snapshot;

    #[test]
    fn success_cell_snapshot() {
        let cell = success_cell(
            &CreatedApiKey {
                organization_id: "org-default".to_string(),
                organization_title: Some("Default Org".to_string()),
                default_project_id: "proj-default".to_string(),
                default_project_title: Some("Default Project".to_string()),
                project_api_key: "sk-proj-123".to_string(),
            },
            Ok(()),
            LiveApplyOutcome::Applied,
        );

        assert_snapshot!(render_cell(&cell));
    }

    #[test]
    fn success_cell_snapshot_when_live_apply_is_skipped() {
        let cell = success_cell(
            &CreatedApiKey {
                organization_id: "org-default".to_string(),
                organization_title: None,
                default_project_id: "proj-default".to_string(),
                default_project_title: None,
                project_api_key: "sk-proj-123".to_string(),
            },
            Err("clipboard unavailable".to_string()),
            LiveApplyOutcome::Skipped(
                "Created OPENAI_API_KEY, but left this session unchanged because ChatGPT login is required here."
                    .to_string(),
            ),
        );

        assert_snapshot!(render_cell(&cell));
    }

    #[test]
    fn continue_in_browser_message_snapshot() {
        let cell = continue_in_browser_message(
            "https://auth.openai.com/oauth/authorize?client_id=abc",
            /*callback_port*/ 5000,
            /*browser_opened*/ false,
        );

        assert_snapshot!(render_cell(&cell));
    }

    #[test]
    fn existing_shell_api_key_message_mentions_openai_api_key() {
        let cell = existing_shell_api_key_message();

        assert_eq!(
            render_cell(&cell),
            "• OPENAI_API_KEY is already set in this Codex session; skipping API key creation. This Codex session already inherited OPENAI_API_KEY from its shell environment. Unset it and run /create-api-key again if you want Codex to create a different key."
        );
    }

    #[test]
    fn continue_in_browser_message_always_includes_the_auth_url() {
        let cell = continue_in_browser_message(
            "https://auth.example.com/oauth/authorize?state=abc",
            5000,
            /*browser_opened*/ false,
        );

        assert!(render_cell(&cell).contains("https://auth.example.com/oauth/authorize?state=abc"));
    }

    #[test]
    fn mask_api_key_preserves_prefix_and_suffix() {
        assert_eq!(mask_api_key("sk-proj-1234567890"), "sk-proj-...7890");
    }

    fn render_cell(cell: &PlainHistoryCell) -> String {
        cell.display_lines(120)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}
