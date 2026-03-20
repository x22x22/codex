use crate::command::FsCommand;
use crate::command::parse_command_from_args;
use std::ffi::OsString;
use std::io::Read;
use std::io::Write;

pub fn run_from_args(
    args: impl Iterator<Item = OsString>,
    stdin: &mut impl Read,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> std::io::Result<()> {
    let command = parse_command_from_args(args)
        .inspect_err(|error| {
            writeln!(stderr, "{error}").ok();
        })
        .map_err(std::io::Error::other)?;

    execute(command, stdin, stdout)
}

fn execute(
    command: FsCommand,
    _stdin: &mut impl Read,
    stdout: &mut impl Write,
) -> std::io::Result<()> {
    match command {
        FsCommand::ReadFile { path } => {
            let mut file = std::fs::File::open(path)?;
            std::io::copy(&mut file, stdout).map(|_| ())
        }
    }
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
