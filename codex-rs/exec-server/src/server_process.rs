use std::path::PathBuf;
use std::process::Stdio;

use tokio::process::Child;
use tokio::process::ChildStdin;
use tokio::process::ChildStdout;
use tokio::process::Command;

use crate::client::ExecServerError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecServerLaunchCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}

pub(crate) struct SpawnedStdioExecServer {
    pub(crate) child: Child,
    pub(crate) stdin: ChildStdin,
    pub(crate) stdout: ChildStdout,
}

pub(crate) fn spawn_stdio_exec_server(
    command: ExecServerLaunchCommand,
) -> Result<SpawnedStdioExecServer, ExecServerError> {
    let mut child = Command::new(&command.program);
    child.args(&command.args);
    child.stdin(Stdio::piped());
    child.stdout(Stdio::piped());
    child.stderr(Stdio::inherit());
    child.kill_on_drop(true);

    let mut child = child.spawn().map_err(ExecServerError::Spawn)?;
    let stdin = child.stdin.take().ok_or_else(|| {
        ExecServerError::Protocol("exec-server stdin was not captured".to_string())
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ExecServerError::Protocol("exec-server stdout was not captured".to_string())
    })?;

    Ok(SpawnedStdioExecServer {
        child,
        stdin,
        stdout,
    })
}
