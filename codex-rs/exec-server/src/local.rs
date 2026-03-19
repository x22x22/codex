use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use tokio::process::Child;
use tokio::process::Command;
use tokio::time::Instant;
use tokio::time::sleep;

use crate::client::ExecServerClient;
use crate::client::ExecServerError;
use crate::client_api::ExecServerClientConnectOptions;
use crate::client_api::RemoteExecServerConnectArgs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecServerLaunchCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}

pub struct SpawnedExecServer {
    client: ExecServerClient,
    child: StdMutex<Option<Child>>,
}

const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(25);

impl SpawnedExecServer {
    pub fn client(&self) -> &ExecServerClient {
        &self.client
    }
}

impl Drop for SpawnedExecServer {
    fn drop(&mut self) {
        if let Ok(mut child_guard) = self.child.lock()
            && let Some(child) = child_guard.as_mut()
        {
            let _ = child.start_kill();
        }
    }
}

pub async fn spawn_local_exec_server(
    command: ExecServerLaunchCommand,
    options: ExecServerClientConnectOptions,
) -> Result<SpawnedExecServer, ExecServerError> {
    let websocket_url = reserve_websocket_url().map_err(ExecServerError::Spawn)?;

    let mut child = Command::new(&command.program);
    child.args(&command.args);
    child.args(["--listen", &websocket_url]);
    child.stdin(Stdio::null());
    child.stdout(Stdio::null());
    child.stderr(Stdio::inherit());
    child.kill_on_drop(true);

    let mut child = child.spawn().map_err(ExecServerError::Spawn)?;
    let connect_args = RemoteExecServerConnectArgs {
        websocket_url,
        client_name: options.client_name.clone(),
        connect_timeout: options.initialize_timeout,
        initialize_timeout: options.initialize_timeout,
    };

    let client = match connect_when_ready(connect_args).await {
        Ok(client) => client,
        Err(err) => {
            let _ = child.start_kill();
            return Err(err);
        }
    };

    Ok(SpawnedExecServer {
        client,
        child: StdMutex::new(Some(child)),
    })
}

fn reserve_websocket_url() -> std::io::Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    drop(listener);
    Ok(format!("ws://{addr}"))
}

async fn connect_when_ready(
    args: RemoteExecServerConnectArgs,
) -> Result<ExecServerClient, ExecServerError> {
    let deadline = Instant::now() + args.connect_timeout;
    loop {
        match ExecServerClient::connect_websocket(args.clone()).await {
            Ok(client) => return Ok(client),
            Err(ExecServerError::WebSocketConnect { source, .. })
                if Instant::now() < deadline
                    && matches!(
                        source,
                        tokio_tungstenite::tungstenite::Error::Io(ref io_err)
                            if io_err.kind() == std::io::ErrorKind::ConnectionRefused
                    ) =>
            {
                sleep(CONNECT_RETRY_INTERVAL).await;
            }
            Err(err) => return Err(err),
        }
    }
}
