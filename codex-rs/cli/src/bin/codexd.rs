use clap::Parser;
use codex_app_server::run_main_with_runtime;
use codex_arg0::Arg0DispatchPaths;
use codex_arg0::arg0_dispatch_or_else;
use codex_core::config_loader::LoaderOverrides;
use codex_utils_cli::CliConfigOverrides;
use std::io::ErrorKind;

#[derive(Debug, Parser)]
#[clap(
    author,
    version,
    bin_name = "codexd",
    override_usage = "codexd [OPTIONS]"
)]
struct CodexdCli {
    #[clap(flatten)]
    config_overrides: CliConfigOverrides,
}

fn main() -> anyhow::Result<()> {
    arg0_dispatch_or_else(|arg0_paths: Arg0DispatchPaths| async move {
        let cli = CodexdCli::parse();
        cli.config_overrides.parse_overrides().map_err(|err| {
            std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("error parsing -c overrides: {err}"),
            )
        })?;

        run_main_with_runtime(
            arg0_paths,
            cli.config_overrides,
            LoaderOverrides::default(),
            true,
            None,
            true,
        )
        .await?;
        Ok(())
    })
}
