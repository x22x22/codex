use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    about = "Generate TypeScript bindings, JSON Schemas, and Python bindings for the Codex app-server protocol"
)]
struct Args {
    /// Output directory where generated files will be written
    #[arg(short = 'o', long = "out", value_name = "DIR")]
    out_dir: PathBuf,

    /// Optional Prettier executable path to format generated TypeScript files
    #[arg(short = 'p', long = "prettier", value_name = "PRETTIER_BIN")]
    prettier: Option<PathBuf>,

    /// Optional Ruff executable path to format generated Python files
    #[arg(long = "ruff", value_name = "RUFF_BIN")]
    ruff: Option<PathBuf>,

    /// Include experimental API methods and fields in generated output.
    #[arg(long = "experimental")]
    experimental: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let ts_out_dir = args.out_dir.join("typescript");
    let json_out_dir = args.out_dir.join("json");
    let python_out_dir = args.out_dir.join("python");
    codex_app_server_protocol::generate_ts_with_options(
        &ts_out_dir,
        args.prettier.as_deref(),
        codex_app_server_protocol::GenerateTsOptions {
            experimental_api: args.experimental,
            ..codex_app_server_protocol::GenerateTsOptions::default()
        },
    )?;
    codex_app_server_protocol::generate_json_with_experimental(&json_out_dir, args.experimental)?;
    codex_app_server_protocol::generate_python_with_options(
        &python_out_dir,
        args.ruff.as_deref(),
        codex_app_server_protocol::GeneratePythonOptions {
            experimental_api: args.experimental,
            ..codex_app_server_protocol::GeneratePythonOptions::default()
        },
    )
}
