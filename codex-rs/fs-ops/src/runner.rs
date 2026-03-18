use crate::FsCommand;
use crate::FsPayload;
use crate::FsResponse;
use crate::parse_command_from_args;
use anyhow::Context;
use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use std::ffi::OsString;
use std::io::Write;

pub fn execute(command: FsCommand) -> FsResponse {
    match command {
        FsCommand::ReadBytes { path } => match std::fs::read(&path) {
            Ok(bytes) => FsResponse::Success {
                payload: FsPayload::Bytes {
                    base64: BASE64_STANDARD.encode(bytes),
                },
            },
            Err(error) => FsResponse::Error {
                error: error.into(),
            },
        },
        FsCommand::ReadText { path } => match std::fs::read_to_string(&path) {
            Ok(text) => FsResponse::Success {
                payload: FsPayload::Text { text },
            },
            Err(error) => FsResponse::Error {
                error: error.into(),
            },
        },
    }
}

pub fn write_response(stdout: &mut impl Write, response: &FsResponse) -> Result<()> {
    serde_json::to_writer(&mut *stdout, response).context("failed to serialize fs response")?;
    writeln!(stdout).context("failed to terminate fs response with newline")?;
    Ok(())
}

pub fn run_from_args(args: impl Iterator<Item = OsString>) -> Result<()> {
    let command = parse_command_from_args(args).map_err(anyhow::Error::msg)?;
    let response = execute(command);
    let mut stdout = std::io::stdout().lock();
    write_response(&mut stdout, &response)
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
