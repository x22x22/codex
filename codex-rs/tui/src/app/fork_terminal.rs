#![cfg(target_os = "macos")]

use codex_protocol::ThreadId;
use color_eyre::eyre::Result;

use super::App;
use crate::tui;

fn append_config_override(args: &mut Vec<String>, key: &str, value: String) {
    args.push("-c".to_string());
    args.push(format!("{key}={value}"));
}

pub(super) async fn spawn_fork_in_terminal(
    app: &App,
    tui: &mut tui::Tui,
    thread_id: ThreadId,
) -> Result<()> {
    let program = std::env::current_exe()?.to_string_lossy().into_owned();
    let mut args = vec!["fork".to_string(), thread_id.to_string()];

    for (key, value) in &app.cli_kv_overrides {
        append_config_override(&mut args, key, value.to_string());
    }
    if let Some(profile) = app.active_profile.as_ref() {
        args.push("-p".to_string());
        args.push(profile.clone());
    }

    let cwd = app.config.cwd.display().to_string();
    args.push("-C".to_string());
    args.push(cwd.clone());
    args.push("-m".to_string());
    args.push(app.chat_widget.current_model().to_string());

    if let Some(effort) = app.config.model_reasoning_effort {
        append_config_override(&mut args, "model_reasoning_effort", effort.to_string());
    }
    if let Some(policy) = app.runtime_approval_policy_override.as_ref()
        && let Ok(value) = toml::Value::try_from(*policy)
    {
        append_config_override(&mut args, "approval_policy", value.to_string());
    }
    if let Some(policy) = app.runtime_sandbox_policy_override.as_ref() {
        let sandbox_mode = match policy {
            codex_protocol::protocol::SandboxPolicy::ReadOnly { .. } => "read-only",
            codex_protocol::protocol::SandboxPolicy::WorkspaceWrite { .. } => "workspace-write",
            codex_protocol::protocol::SandboxPolicy::DangerFullAccess
            | codex_protocol::protocol::SandboxPolicy::ExternalSandbox { .. } => {
                "danger-full-access"
            }
        };
        append_config_override(&mut args, "sandbox_mode", sandbox_mode.to_string());
    }

    let command =
        shlex::try_join(std::iter::once(program.as_str()).chain(args.iter().map(String::as_str)))
            .map_err(|err| color_eyre::eyre::eyre!(err.to_string()))?;
    let shell_command = format!(
        "cd {} && exec {command}",
        shlex::try_join(std::iter::once(cwd.as_str()))
            .map_err(|err| color_eyre::eyre::eyre!(err.to_string()))?,
    );
    let command = shlex::try_join(["/bin/sh", "-lc", shell_command.as_str()])
        .map_err(|err| color_eyre::eyre::eyre!(err.to_string()))?;

    tui.with_restored(tui::RestoreMode::KeepRaw, || async move {
        for app_name in ["iTerm2", "iTerm"] {
            let status = tokio::process::Command::new("osascript")
                .arg("-e")
                .arg("on run argv")
                .arg("-e")
                .arg(format!(
                    "tell application \"{app_name}\" to create window with default profile command (item 1 of argv)"
                ))
                .arg("-e")
                .arg("end run")
                .arg(&command)
                .status()
                .await?;
            if status.success() {
                return Ok(());
            }
        }
        Err(color_eyre::eyre::eyre!("failed to open iTerm"))
    })
    .await
}
