use anyhow::Result;
use clap::Parser;
use codex_fs_ops::CODEX_CORE_FS_OPS_ARG1;
use std::path::PathBuf;

/// Generate the JSON Schema for `config.toml` and write it to `config.schema.json`.
#[derive(Parser)]
#[command(name = "codex-write-config-schema")]
struct Args {
    #[arg(short, long, value_name = "PATH")]
    out: Option<PathBuf>,
}

fn main() -> Result<()> {
    let mut args = std::env::args_os();
    let _program_name = args.next();
    if matches!(
        args.next().as_deref(),
        Some(flag) if flag == std::ffi::OsStr::new(CODEX_CORE_FS_OPS_ARG1)
    ) {
        return codex_fs_ops::run_from_args(args);
    }

    let args = Args::parse();
    let out_path = args
        .out
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.schema.json"));
    codex_core::config::schema::write_config_schema(&out_path)?;
    Ok(())
}
