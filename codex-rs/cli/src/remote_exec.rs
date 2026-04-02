use std::io::ErrorKind;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::net::TcpStream;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;

pub(crate) const DEFAULT_EXEC_SERVER_PROGRAM: &str = "codex-exec-server";

const LOOPBACK_HOST: &str = "127.0.0.1";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(100);
const PROBE_IO_TIMEOUT: Duration = Duration::from_millis(300);
const SHUTDOWN_WAIT_TIMEOUT: Duration = Duration::from_secs(2);
const STDERR_TAIL_MAX_BYTES: usize = 16 * 1024;

const WEBSOCKET_PROBE_REQUEST: &[u8] = b"\
GET / HTTP/1.1\r\n\
Host: 127.0.0.1\r\n\
Connection: Upgrade\r\n\
Upgrade: websocket\r\n\
Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
Sec-WebSocket-Version: 13\r\n\
\r\n";

pub(crate) struct RemoteExecServerSshTunnel {
    websocket_url: String,
    child: Child,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    stderr_reader: Option<JoinHandle<()>>,
}

impl RemoteExecServerSshTunnel {
    pub(crate) async fn launch(
        ssh_target: String,
        exec_server_program: Option<String>,
    ) -> anyhow::Result<Self> {
        tokio::task::spawn_blocking(move || Self::launch_blocking(ssh_target, exec_server_program))
            .await?
    }

    pub(crate) fn websocket_url(&self) -> &str {
        &self.websocket_url
    }

    fn launch_blocking(
        ssh_target: String,
        exec_server_program: Option<String>,
    ) -> anyhow::Result<Self> {
        let ssh_target = ssh_target.trim().to_string();
        if ssh_target.is_empty() {
            anyhow::bail!("`--exec-server-ssh` requires a non-empty SSH target");
        }

        let exec_server_program = exec_server_program
            .unwrap_or_else(|| DEFAULT_EXEC_SERVER_PROGRAM.to_string())
            .trim()
            .to_string();
        if exec_server_program.is_empty() {
            anyhow::bail!("`--exec-server-program` requires a non-empty command");
        }

        let listener = TcpListener::bind((LOOPBACK_HOST, 0))?;
        let local_port = listener.local_addr()?.port();
        drop(listener);

        let websocket_url = format!("ws://{LOOPBACK_HOST}:{local_port}");
        let forward_spec = format!("{LOOPBACK_HOST}:{local_port}:{LOOPBACK_HOST}:{local_port}");

        let mut command = build_ssh_command(
            &ssh_target,
            &exec_server_program,
            &forward_spec,
            &websocket_url,
        );
        let mut child = command.spawn().map_err(|err| {
            anyhow::anyhow!("failed to start `ssh {ssh_target}` for remote exec-server: {err}")
        })?;

        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        let stderr_reader = child
            .stderr
            .take()
            .map(|stderr| spawn_stderr_reader(stderr, Arc::clone(&stderr_tail)));

        let mut tunnel = Self {
            websocket_url,
            child,
            stderr_tail,
            stderr_reader,
        };

        if let Err(err) = tunnel.wait_until_ready(local_port) {
            tunnel.terminate();
            return Err(err);
        }

        Ok(tunnel)
    }

    fn wait_until_ready(&mut self, local_port: u16) -> anyhow::Result<()> {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait()? {
                let stderr_tail = self.stderr_tail();
                if status.success() {
                    anyhow::bail!(
                        "remote exec-server command exited before startup completed{stderr_tail}"
                    );
                }
                anyhow::bail!(
                    "remote exec-server ssh command failed with status {status}{stderr_tail}"
                );
            }

            if probe_websocket(local_port).is_ok() {
                return Ok(());
            }

            thread::sleep(STARTUP_POLL_INTERVAL);
        }

        anyhow::bail!(
            "timed out waiting for remote exec-server to accept websocket connections at `{}`{}",
            self.websocket_url,
            self.stderr_tail()
        );
    }

    fn stderr_tail(&self) -> String {
        let Ok(stderr_tail) = self.stderr_tail.lock() else {
            return String::new();
        };
        if stderr_tail.is_empty() {
            return String::new();
        }
        format!("; stderr: {}", String::from_utf8_lossy(&stderr_tail).trim())
    }

    fn terminate(&mut self) {
        let _ = self.child.kill();
        let deadline = Instant::now() + SHUTDOWN_WAIT_TIMEOUT;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => thread::sleep(STARTUP_POLL_INTERVAL),
                Err(_) => break,
            }
        }
        let _ = self.child.wait();
        if let Some(stderr_reader) = self.stderr_reader.take() {
            let _ = stderr_reader.join();
        }
    }
}

impl Drop for RemoteExecServerSshTunnel {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn build_ssh_command(
    ssh_target: &str,
    exec_server_program: &str,
    forward_spec: &str,
    websocket_url: &str,
) -> Command {
    let mut command = Command::new("ssh");
    command
        .arg("-T")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ExitOnForwardFailure=yes")
        .arg("-L")
        .arg(forward_spec)
        .arg(ssh_target)
        .arg(exec_server_program)
        .arg("--listen")
        .arg(websocket_url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command
}

fn probe_websocket(local_port: u16) -> std::io::Result<()> {
    let mut stream = TcpStream::connect((LOOPBACK_HOST, local_port))?;
    stream.set_read_timeout(Some(PROBE_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(PROBE_IO_TIMEOUT))?;
    stream.write_all(WEBSOCKET_PROBE_REQUEST)?;

    let mut response = [0_u8; 128];
    let bytes_read = stream.read(&mut response)?;
    if bytes_read == 0 {
        return Err(std::io::Error::new(
            ErrorKind::UnexpectedEof,
            "websocket probe received an empty response",
        ));
    }
    if response[..bytes_read].starts_with(b"HTTP/1.1 101") {
        return Ok(());
    }

    Err(std::io::Error::new(
        ErrorKind::InvalidData,
        format!(
            "websocket probe returned `{}`",
            String::from_utf8_lossy(&response[..bytes_read]).trim_end()
        ),
    ))
}

fn spawn_stderr_reader(
    mut stderr: impl Read + Send + 'static,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0_u8; 1024];
        loop {
            let bytes_read = match stderr.read(&mut buffer) {
                Ok(0) => break,
                Ok(bytes_read) => bytes_read,
                Err(_) => break,
            };
            let Ok(mut stderr_tail) = stderr_tail.lock() else {
                break;
            };
            stderr_tail.extend_from_slice(&buffer[..bytes_read]);
            if stderr_tail.len() > STDERR_TAIL_MAX_BYTES {
                let trim_to = stderr_tail.len() - STDERR_TAIL_MAX_BYTES;
                stderr_tail.drain(..trim_to);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::build_ssh_command;
    use pretty_assertions::assert_eq;

    #[test]
    fn build_ssh_command_forwards_loopback_port_and_exec_server_program() {
        let command = build_ssh_command(
            "dev",
            "codex-exec-server",
            "127.0.0.1:9876:127.0.0.1:9876",
            "ws://127.0.0.1:9876",
        );

        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            args,
            vec![
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "ExitOnForwardFailure=yes",
                "-L",
                "127.0.0.1:9876:127.0.0.1:9876",
                "dev",
                "codex-exec-server",
                "--listen",
                "ws://127.0.0.1:9876",
            ]
        );
    }
}
