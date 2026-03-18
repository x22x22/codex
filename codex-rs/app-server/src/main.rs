use clap::Parser;
use codex_app_server::AppServerTransport;
use codex_app_server::run_main_with_runtime;
use codex_arg0::Arg0DispatchPaths;
use codex_arg0::arg0_dispatch_or_else;
use codex_core::config_loader::LoaderOverrides;
use codex_utils_cli::CliConfigOverrides;
use std::path::PathBuf;

// Debug-only test hook: lets integration tests point the server at a temporary
// managed config file without writing to /etc.
const MANAGED_CONFIG_PATH_ENV_VAR: &str = "CODEX_APP_SERVER_MANAGED_CONFIG_PATH";

#[derive(Debug, Parser)]
struct AppServerArgs {
    /// Transport endpoint URL. Supported values: `stdio://` (default),
    /// `ws://IP:PORT`.
    #[arg(
        long = "listen",
        value_name = "URL",
        default_value = AppServerTransport::DEFAULT_LISTEN_URL
    )]
    listen: AppServerTransport,

    /// Also connect outbound to the ChatGPT remote control server derived from
    /// the configured `chatgpt_base_url`.
    #[arg(long = "with-remote-control", default_value_t = false)]
    with_remote_control: bool,
}

fn main() -> anyhow::Result<()> {
    arg0_dispatch_or_else(|arg0_paths: Arg0DispatchPaths| async move {
        let args = AppServerArgs::parse();
        let managed_config_path = managed_config_path_from_debug_env();
        let loader_overrides = LoaderOverrides {
            managed_config_path,
            ..Default::default()
        };
        run_main_with_runtime(
            arg0_paths,
            CliConfigOverrides::default(),
            loader_overrides,
            false,
            Some(args.listen),
            args.with_remote_control,
        )
        .await?;
        Ok(())
    })
}

fn managed_config_path_from_debug_env() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    {
        if let Ok(value) = std::env::var(MANAGED_CONFIG_PATH_ENV_VAR) {
            return if value.is_empty() {
                None
            } else {
                Some(PathBuf::from(value))
            };
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::AppServerArgs;
    use clap::Parser;
    use pretty_assertions::assert_eq;

    #[test]
    fn app_server_args_default_to_stdio_without_remote_control() {
        let args = AppServerArgs::parse_from(["codex-app-server"]);
        assert_eq!(args.listen, super::AppServerTransport::Stdio);
        assert!(!args.with_remote_control);
    }

    #[test]
    fn app_server_args_parse_with_remote_control_flag() {
        let args = AppServerArgs::parse_from([
            "codex-app-server",
            "--listen",
            "ws://127.0.0.1:8080",
            "--with-remote-control",
        ]);
        assert_eq!(
            args.listen,
            super::AppServerTransport::WebSocket {
                bind_address: "127.0.0.1:8080".parse().expect("valid socket address"),
            }
        );
        assert!(args.with_remote_control);
    }
}
