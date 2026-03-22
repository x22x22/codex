mod cli;

pub use cli::Cli;

use anyhow::Context;
use codex_executor_protocol::CallToolResponse;
use codex_executor_protocol::ExecutorToOrchestratorMessage;
use codex_executor_protocol::ExecutorToolSpec;
use codex_executor_protocol::ListToolsResponse;
use codex_executor_protocol::OrchestratorToExecutorMessage;
use codex_executor_protocol::ShutdownResponse;
use codex_executor_protocol::ToolCallOutcome;
use tokio::sync::mpsc;

pub async fn run_main(cli: Cli) -> anyhow::Result<()> {
    let (outbound, inbound) = establish_connection(&cli).await?;
    Executor::new(outbound, inbound).run().await
}

async fn establish_connection(
    _cli: &Cli,
) -> anyhow::Result<(
    mpsc::Sender<ExecutorToOrchestratorMessage>,
    mpsc::Receiver<OrchestratorToExecutorMessage>,
)> {
    unimplemented!("executor transport establishment is not implemented yet");
}

pub struct Executor {
    outbound: mpsc::Sender<ExecutorToOrchestratorMessage>,
    inbound: mpsc::Receiver<OrchestratorToExecutorMessage>,
    tools: Vec<ExecutorToolSpec>,
}

impl Executor {
    pub fn new(
        outbound: mpsc::Sender<ExecutorToOrchestratorMessage>,
        inbound: mpsc::Receiver<OrchestratorToExecutorMessage>,
    ) -> Self {
        Self::with_tools(outbound, inbound, Vec::new())
    }

    pub fn with_tools(
        outbound: mpsc::Sender<ExecutorToOrchestratorMessage>,
        inbound: mpsc::Receiver<OrchestratorToExecutorMessage>,
        tools: Vec<ExecutorToolSpec>,
    ) -> Self {
        Self {
            outbound,
            inbound,
            tools,
        }
    }

    pub async fn run(mut self) -> anyhow::Result<()> {
        while let Some(message) = self.inbound.recv().await {
            if self.handle_message(message).await? {
                break;
            }
        }

        Ok(())
    }

    async fn handle_message(
        &mut self,
        message: OrchestratorToExecutorMessage,
    ) -> anyhow::Result<bool> {
        match message {
            OrchestratorToExecutorMessage::ListTools(request) => {
                self.send(ExecutorToOrchestratorMessage::ListToolsResponse(
                    ListToolsResponse {
                        request_id: request.request_id,
                        tools: self.tools.clone(),
                    },
                ))
                .await?;
                Ok(false)
            }
            OrchestratorToExecutorMessage::CallTool(request) => {
                let outcome = if self.tool_exists(request.tool_name.as_str()) {
                    ToolCallOutcome::Error {
                        message: format!(
                            "tool `{}` is registered but execution is not implemented yet",
                            request.tool_name
                        ),
                    }
                } else {
                    ToolCallOutcome::Error {
                        message: format!("unknown executor tool `{}`", request.tool_name),
                    }
                };

                self.send(ExecutorToOrchestratorMessage::CallToolResponse(
                    CallToolResponse {
                        request_id: request.request_id,
                        tool_name: request.tool_name,
                        outcome,
                    },
                ))
                .await?;
                Ok(false)
            }
            OrchestratorToExecutorMessage::Shutdown(request) => {
                self.send(ExecutorToOrchestratorMessage::ShutdownResponse(
                    ShutdownResponse {
                        request_id: request.request_id,
                    },
                ))
                .await?;
                Ok(true)
            }
        }
    }

    async fn send(&self, message: ExecutorToOrchestratorMessage) -> anyhow::Result<()> {
        self.outbound
            .send(message)
            .await
            .context("executor outbound channel closed")
    }

    fn tool_exists(&self, tool_name: &str) -> bool {
        self.tools.iter().any(|tool| tool.name == tool_name)
    }
}

#[cfg(test)]
mod tests {
    use super::Executor;
    use codex_executor_protocol::CallToolRequest;
    use codex_executor_protocol::ExecutorToOrchestratorMessage;
    use codex_executor_protocol::ExecutorToolSpec;
    use codex_executor_protocol::ListToolsRequest;
    use codex_executor_protocol::ListToolsResponse;
    use codex_executor_protocol::OrchestratorToExecutorMessage;
    use codex_executor_protocol::ShutdownRequest;
    use codex_executor_protocol::ShutdownResponse;
    use codex_executor_protocol::ToolCallOutcome;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn executor_lists_registered_tools() {
        let (outbound_tx, mut outbound_rx) = mpsc::channel(4);
        let (inbound_tx, inbound_rx) = mpsc::channel(4);
        let executor = Executor::with_tools(
            outbound_tx,
            inbound_rx,
            vec![ExecutorToolSpec::new(
                "exec_command",
                "Run a command",
                json!({"type": "object"}),
            )],
        );

        let task = tokio::spawn(async move { executor.run().await });
        inbound_tx
            .send(OrchestratorToExecutorMessage::ListTools(ListToolsRequest {
                request_id: "req-1".to_string(),
            }))
            .await
            .expect("send request");

        let response = outbound_rx.recv().await.expect("response");
        assert_eq!(
            response,
            ExecutorToOrchestratorMessage::ListToolsResponse(ListToolsResponse {
                request_id: "req-1".to_string(),
                tools: vec![ExecutorToolSpec::new(
                    "exec_command",
                    "Run a command",
                    json!({"type": "object"}),
                )],
            })
        );

        drop(inbound_tx);
        task.await.expect("task join").expect("task result");
    }

    #[tokio::test]
    async fn executor_returns_error_for_call_tool_until_handlers_exist() {
        let (outbound_tx, mut outbound_rx) = mpsc::channel(4);
        let (inbound_tx, inbound_rx) = mpsc::channel(4);
        let executor = Executor::with_tools(
            outbound_tx,
            inbound_rx,
            vec![ExecutorToolSpec::new(
                "exec_command",
                "Run a command",
                json!({"type": "object"}),
            )],
        );

        let task = tokio::spawn(async move { executor.run().await });
        inbound_tx
            .send(OrchestratorToExecutorMessage::CallTool(CallToolRequest {
                request_id: "req-2".to_string(),
                tool_name: "exec_command".to_string(),
                arguments: json!({"cmd": "pwd"}),
            }))
            .await
            .expect("send request");

        let response = outbound_rx.recv().await.expect("response");
        let ExecutorToOrchestratorMessage::CallToolResponse(response) = response else {
            panic!("expected call tool response");
        };
        assert_eq!(response.request_id, "req-2");
        assert_eq!(response.tool_name, "exec_command");
        assert_eq!(
            response.outcome,
            ToolCallOutcome::Error {
                message: "tool `exec_command` is registered but execution is not implemented yet"
                    .to_string(),
            }
        );

        drop(inbound_tx);
        task.await.expect("task join").expect("task result");
    }

    #[tokio::test]
    async fn executor_acknowledges_shutdown_and_exits() {
        let (outbound_tx, mut outbound_rx) = mpsc::channel(4);
        let (inbound_tx, inbound_rx) = mpsc::channel(4);
        let executor = Executor::new(outbound_tx, inbound_rx);

        let task = tokio::spawn(async move { executor.run().await });
        inbound_tx
            .send(OrchestratorToExecutorMessage::Shutdown(ShutdownRequest {
                request_id: "req-3".to_string(),
            }))
            .await
            .expect("send request");

        let response = outbound_rx.recv().await.expect("response");
        assert_eq!(
            response,
            ExecutorToOrchestratorMessage::ShutdownResponse(ShutdownResponse {
                request_id: "req-3".to_string(),
            })
        );

        task.await.expect("task join").expect("task result");
    }
}
