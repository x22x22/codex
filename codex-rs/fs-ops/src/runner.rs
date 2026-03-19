use crate::FsCommand;
use crate::FsError;
use crate::parse_command_from_args;
use anyhow::Context;
use anyhow::Result;
use std::ffi::OsString;
use std::io::Read;
use std::io::Write;

pub fn run_from_args(
    args: impl Iterator<Item = OsString>,
    stdin: &mut impl Read,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> Result<()> {
    let command = match parse_command_from_args(args) {
        Ok(command) => command,
        Err(error) => {
            writeln!(stderr, "{error}").context("failed to write fs helper usage error")?;
            anyhow::bail!("{error}");
        }
    };

    if let Err(error) = execute(command, stdin, stdout) {
        write_error(stderr, &error)?;
        anyhow::bail!("{error}");
    }

    Ok(())
}

pub fn execute(
    command: FsCommand,
    stdin: &mut impl Read,
    stdout: &mut impl Write,
) -> Result<(), FsError> {
    match command {
        FsCommand::ReadFile { path } => {
            let mut file = std::fs::File::open(path).map_err(FsError::from)?;
            std::io::copy(&mut file, stdout)
                .map(|_| ())
                .map_err(FsError::from)
        }
        FsCommand::WriteFile { path } => {
            let mut file = std::fs::File::create(path).map_err(FsError::from)?;
            std::io::copy(stdin, &mut file)
                .map(|_| ())
                .map_err(FsError::from)
        }
    }
}

pub fn write_error(stderr: &mut impl Write, error: &FsError) -> Result<()> {
    serde_json::to_writer(&mut *stderr, error).context("failed to serialize fs error")?;
    writeln!(stderr).context("failed to terminate fs error with newline")?;
    Ok(())
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
