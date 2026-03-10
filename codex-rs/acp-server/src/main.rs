use codex_acp_server::run_main;
use codex_arg0::Arg0DispatchPaths;
use codex_arg0::arg0_dispatch_or_else;
use codex_utils_cli::CliConfigOverrides;

fn main() -> anyhow::Result<()> {
    arg0_dispatch_or_else(|arg0_paths: Arg0DispatchPaths| async move {
        run_main(arg0_paths, CliConfigOverrides::default()).await?;
        Ok(())
    })
}
