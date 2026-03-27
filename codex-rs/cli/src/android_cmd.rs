use anyhow::Context;
use anyhow::anyhow;
use clap::Args;
use clap::Parser;
use codex_arg0::Arg0DispatchPaths;
use codex_tui::AppExitInfo;
use codex_tui::Cli as TuiCli;
use codex_utils_cli::CliConfigOverrides;
use futures::SinkExt;
use futures::StreamExt;
use serde_json::Value;
use serde_json::json;
use std::process::Command as StdCommand;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::time::sleep;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use uuid::Uuid;

const AGENT_BRIDGE_REMOTE_PORT: u16 = 48_765;
const AGENT_BRIDGE_BOOTSTRAP_COMPONENT: &str =
    "com.openai.codex.agent/.DesktopBridgeBootstrapActivity";
const AGENT_BRIDGE_AUTH_EXTRA: &str = "com.openai.codex.agent.extra.DESKTOP_BRIDGE_AUTH_TOKEN";
const CONTROL_PATH: &str = "/control";
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(125);
const CONNECT_RETRY_COUNT: usize = 40;

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug, Parser)]
pub struct AndroidCli {
    #[command(subcommand)]
    subcommand: AndroidSubcommand,
}

#[derive(Debug, clap::Subcommand)]
enum AndroidSubcommand {
    /// Manage Android Agent/Genie sessions over adb.
    Sessions(AndroidSessionsCli),
}

#[derive(Debug, Parser)]
struct AndroidSessionsCli {
    #[command(subcommand)]
    subcommand: AndroidSessionsSubcommand,
}

#[derive(Debug, clap::Subcommand)]
enum AndroidSessionsSubcommand {
    /// List Android Agent and Genie sessions.
    List(AndroidListSessionsArgs),
    /// Read detailed diagnostics for one Android session.
    Read(AndroidSessionIdArgs),
    /// Create a new Android session draft without starting it.
    Create(AndroidCreateSessionArgs),
    /// Start a previously created Android session draft.
    Start(AndroidStartSessionArgs),
    /// Answer a waiting Android session question.
    Answer(AndroidAnswerSessionArgs),
    /// Cancel an Android session.
    Cancel(AndroidSessionIdArgs),
    /// Attach the target surface for an Android session.
    AttachTarget(AndroidSessionIdArgs),
    /// Attach the Codex TUI to a live Android session runtime.
    Attach(AndroidAttachSessionArgs),
}

#[derive(Debug, Clone, Args)]
struct AndroidDeviceArgs {
    /// adb device serial. Defaults to the adb default device.
    #[arg(long = "serial", value_name = "SERIAL")]
    serial: Option<String>,
}

#[derive(Debug, Args)]
struct AndroidListSessionsArgs {
    #[clap(flatten)]
    device: AndroidDeviceArgs,
}

#[derive(Debug, Clone, Args)]
struct AndroidSessionIdArgs {
    #[clap(flatten)]
    device: AndroidDeviceArgs,

    /// Android framework session id.
    #[arg(value_name = "SESSION_ID")]
    session_id: String,
}

#[derive(Debug, Args)]
struct AndroidCreateSessionArgs {
    #[clap(flatten)]
    device: AndroidDeviceArgs,

    /// Optional target package to create as an app-scoped HOME draft.
    #[arg(long = "target-package", value_name = "PACKAGE")]
    target_package: Option<String>,

    /// Optional model override stored on the draft session.
    #[arg(long = "model", value_name = "MODEL")]
    model: Option<String>,

    /// Optional reasoning effort override stored on the draft session.
    #[arg(long = "reasoning-effort", value_name = "EFFORT")]
    reasoning_effort: Option<String>,
}

#[derive(Debug, Args)]
struct AndroidStartSessionArgs {
    #[clap(flatten)]
    device: AndroidDeviceArgs,

    /// Android framework session id.
    #[arg(value_name = "SESSION_ID")]
    session_id: String,

    /// Prompt or objective used to start the draft session.
    #[arg(long = "prompt", value_name = "TEXT")]
    prompt: String,
}

#[derive(Debug, Args)]
struct AndroidAnswerSessionArgs {
    #[clap(flatten)]
    device: AndroidDeviceArgs,

    /// Android framework session id.
    #[arg(value_name = "SESSION_ID")]
    session_id: String,

    /// Free-form answer text sent back to the session.
    #[arg(long = "answer", value_name = "TEXT")]
    answer: String,
}

#[derive(Debug, Args)]
struct AndroidAttachSessionArgs {
    #[clap(flatten)]
    device: AndroidDeviceArgs,

    /// Android framework session id.
    #[arg(value_name = "SESSION_ID")]
    session_id: String,

    #[clap(flatten)]
    interactive: TuiCli,
}

pub async fn run(
    android_cli: AndroidCli,
    root_config_overrides: CliConfigOverrides,
    root_interactive: TuiCli,
    arg0_paths: Arg0DispatchPaths,
) -> anyhow::Result<Option<AppExitInfo>> {
    match android_cli.subcommand {
        AndroidSubcommand::Sessions(AndroidSessionsCli { subcommand }) => match subcommand {
            AndroidSessionsSubcommand::List(args) => {
                let bridge = AndroidBridgeClient::connect(args.device.serial).await?;
                print_json(bridge.rpc("androidSession/list", json!({})).await?)?;
                Ok(None)
            }
            AndroidSessionsSubcommand::Read(args) => {
                let bridge = AndroidBridgeClient::connect(args.device.serial).await?;
                print_json(
                    bridge
                        .rpc(
                            "androidSession/read",
                            json!({ "sessionId": args.session_id }),
                        )
                        .await?,
                )?;
                Ok(None)
            }
            AndroidSessionsSubcommand::Create(args) => {
                let bridge = AndroidBridgeClient::connect(args.device.serial).await?;
                print_json(
                    bridge
                        .rpc(
                            "androidSession/create",
                            json!({
                                "targetPackage": args.target_package,
                                "model": args.model,
                                "reasoningEffort": args.reasoning_effort,
                            }),
                        )
                        .await?,
                )?;
                Ok(None)
            }
            AndroidSessionsSubcommand::Start(args) => {
                let bridge = AndroidBridgeClient::connect(args.device.serial).await?;
                print_json(
                    bridge
                        .rpc(
                            "androidSession/start",
                            json!({
                                "sessionId": args.session_id,
                                "prompt": args.prompt,
                            }),
                        )
                        .await?,
                )?;
                Ok(None)
            }
            AndroidSessionsSubcommand::Answer(args) => {
                let bridge = AndroidBridgeClient::connect(args.device.serial).await?;
                print_json(
                    bridge
                        .rpc(
                            "androidSession/answer",
                            json!({
                                "sessionId": args.session_id,
                                "answer": args.answer,
                            }),
                        )
                        .await?,
                )?;
                Ok(None)
            }
            AndroidSessionsSubcommand::Cancel(args) => {
                let bridge = AndroidBridgeClient::connect(args.device.serial).await?;
                print_json(
                    bridge
                        .rpc(
                            "androidSession/cancel",
                            json!({ "sessionId": args.session_id }),
                        )
                        .await?,
                )?;
                Ok(None)
            }
            AndroidSessionsSubcommand::AttachTarget(args) => {
                let bridge = AndroidBridgeClient::connect(args.device.serial).await?;
                print_json(
                    bridge
                        .rpc(
                            "androidSession/attachTarget",
                            json!({ "sessionId": args.session_id }),
                        )
                        .await?,
                )?;
                Ok(None)
            }
            AndroidSessionsSubcommand::Attach(args) => {
                let bridge = AndroidBridgeClient::connect(args.device.serial).await?;
                let attach = bridge
                    .rpc(
                        "androidSession/attach",
                        json!({ "sessionId": args.session_id }),
                    )
                    .await?;
                let thread_id = required_string(&attach, "threadId")?;
                let websocket_path = required_string(&attach, "websocketPath")?;
                let remote = format!("ws://127.0.0.1:{}{websocket_path}", bridge.local_port());

                let mut interactive = root_interactive;
                interactive.resume_picker = false;
                interactive.resume_last = false;
                interactive.resume_session_id = Some(thread_id);
                interactive.resume_show_all = false;
                interactive.resume_include_non_interactive = false;
                interactive.fork_picker = false;
                interactive.fork_last = false;
                interactive.fork_session_id = None;
                interactive.fork_show_all = false;
                super::merge_interactive_cli_flags(&mut interactive, args.interactive);
                super::prepend_config_flags(
                    &mut interactive.config_overrides,
                    root_config_overrides,
                );
                interactive
                    .config_overrides
                    .raw_overrides
                    .push("features.tui_app_server=true".to_string());

                let exit_info = super::run_interactive_tui_with_remote_auth_token(
                    interactive,
                    Some(remote),
                    Some(bridge.auth_token().to_string()),
                    arg0_paths,
                )
                .await
                .map_err(anyhow::Error::from)?;
                Ok(Some(exit_info))
            }
        },
    }
}

struct AndroidBridgeClient {
    auth_token: String,
    forward_guard: AdbForwardGuard,
}

impl AndroidBridgeClient {
    async fn connect(serial: Option<String>) -> anyhow::Result<Self> {
        let local_port = allocate_forward_port(serial.as_deref()).await?;
        let forward_guard = AdbForwardGuard { local_port, serial };
        let auth_token = Uuid::new_v4().simple().to_string();
        bootstrap_bridge(forward_guard.serial.as_deref(), &auth_token).await?;

        let bridge = Self {
            auth_token,
            forward_guard,
        };
        drop(bridge.connect_websocket_with_retry(CONTROL_PATH).await?);
        Ok(bridge)
    }

    fn auth_token(&self) -> &str {
        &self.auth_token
    }

    fn local_port(&self) -> u16 {
        self.forward_guard.local_port
    }

    async fn rpc(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let mut socket = self.connect_websocket_with_retry(CONTROL_PATH).await?;
        let request_id = Uuid::new_v4().simple().to_string();
        let request = json!({
            "id": request_id,
            "method": method,
            "params": params,
        });
        socket
            .send(Message::Text(request.to_string().into()))
            .await
            .with_context(|| format!("failed to send desktop bridge request `{method}`"))?;

        while let Some(message) = socket.next().await {
            match message.context("desktop bridge websocket transport failed")? {
                Message::Text(text) => {
                    let response: Value = serde_json::from_str(text.as_ref())
                        .context("desktop bridge returned invalid JSON")?;
                    if response.get("id").and_then(Value::as_str) != Some(request_id.as_str()) {
                        continue;
                    }
                    if let Some(error) = response.get("error") {
                        let message = error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("desktop bridge request failed");
                        anyhow::bail!("{message}");
                    }
                    let result = response
                        .get("result")
                        .cloned()
                        .ok_or_else(|| anyhow!("desktop bridge response was missing `result`"))?;
                    let _ = socket.close(None).await;
                    return Ok(result);
                }
                Message::Binary(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
                Message::Close(frame) => {
                    let reason = frame
                        .and_then(|value| {
                            let reason = value.reason.to_string();
                            if reason.is_empty() {
                                None
                            } else {
                                Some(reason)
                            }
                        })
                        .unwrap_or_else(|| "connection closed".to_string());
                    anyhow::bail!("desktop bridge closed the websocket: {reason}");
                }
            }
        }

        anyhow::bail!("desktop bridge closed before replying to `{method}`")
    }

    async fn connect_websocket_with_retry(&self, path: &str) -> anyhow::Result<WsStream> {
        let websocket_url = format!("ws://127.0.0.1:{}{path}", self.local_port());
        let mut last_error = None;

        for attempt in 0..CONNECT_RETRY_COUNT {
            match self.connect_websocket_once(&websocket_url).await {
                Ok(socket) => return Ok(socket),
                Err(err) => {
                    last_error = Some(err);
                    if attempt + 1 < CONNECT_RETRY_COUNT {
                        sleep(CONNECT_RETRY_DELAY).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("failed to connect to `{websocket_url}`")))
    }

    async fn connect_websocket_once(&self, websocket_url: &str) -> anyhow::Result<WsStream> {
        let mut request = websocket_url
            .into_client_request()
            .with_context(|| format!("invalid desktop bridge URL `{websocket_url}`"))?;
        let header_value = HeaderValue::from_str(&format!("Bearer {}", self.auth_token))
            .context("desktop bridge auth token produced an invalid Authorization header")?;
        request.headers_mut().insert(AUTHORIZATION, header_value);
        let (socket, _response) = connect_async(request)
            .await
            .with_context(|| format!("failed to connect to desktop bridge at `{websocket_url}`"))?;
        Ok(socket)
    }
}

struct AdbForwardGuard {
    local_port: u16,
    serial: Option<String>,
}

impl Drop for AdbForwardGuard {
    fn drop(&mut self) {
        let mut command = StdCommand::new("adb");
        if let Some(serial) = self.serial.as_deref() {
            command.arg("-s").arg(serial);
        }
        let _ = command
            .arg("forward")
            .arg("--remove")
            .arg(format!("tcp:{}", self.local_port))
            .output();
    }
}

async fn allocate_forward_port(serial: Option<&str>) -> anyhow::Result<u16> {
    let output = adb_output(
        serial,
        &[
            "forward".to_string(),
            "tcp:0".to_string(),
            format!("tcp:{AGENT_BRIDGE_REMOTE_PORT}"),
        ],
    )
    .await?;
    output
        .trim()
        .parse::<u16>()
        .with_context(|| format!("adb returned an invalid forwarded port `{output}`"))
}

async fn bootstrap_bridge(serial: Option<&str>, auth_token: &str) -> anyhow::Result<()> {
    let _ = adb_output(
        serial,
        &[
            "shell".to_string(),
            "am".to_string(),
            "start".to_string(),
            "-W".to_string(),
            "-n".to_string(),
            AGENT_BRIDGE_BOOTSTRAP_COMPONENT.to_string(),
            "--es".to_string(),
            AGENT_BRIDGE_AUTH_EXTRA.to_string(),
            auth_token.to_string(),
        ],
    )
    .await?;
    Ok(())
}

async fn adb_output(serial: Option<&str>, args: &[String]) -> anyhow::Result<String> {
    let mut command = Command::new("adb");
    if let Some(serial) = serial {
        command.arg("-s").arg(serial);
    }
    command.args(args);
    let rendered = render_command("adb", serial, args);
    let output = command
        .output()
        .await
        .with_context(|| format!("failed to run `{rendered}`"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("exit status {}", output.status)
    };
    anyhow::bail!("`{rendered}` failed: {detail}")
}

fn render_command(command: &str, serial: Option<&str>, args: &[String]) -> String {
    let mut rendered = Vec::with_capacity(args.len() + 3);
    rendered.push(command.to_string());
    if let Some(serial) = serial {
        rendered.push("-s".to_string());
        rendered.push(serial.to_string());
    }
    rendered.extend(args.iter().cloned());
    rendered.join(" ")
}

fn print_json(value: Value) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn required_string(value: &Value, key: &str) -> anyhow::Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("desktop bridge response was missing `{key}`"))
}
