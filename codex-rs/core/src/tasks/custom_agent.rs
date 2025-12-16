use std::sync::Arc;

use async_trait::async_trait;
use codex_protocol::user_input::UserInput;
use tokio_util::sync::CancellationToken;

use crate::codex::TurnContext;
use crate::codex_delegate::run_codex_conversation_one_shot;
use crate::protocol::SandboxPolicy;
use crate::state::TaskKind;

use super::SessionTask;
use super::SessionTaskContext;

#[derive(Clone)]
pub(crate) struct CustomAgentTask {
    agent_name: String,
    instructions: String,
    model: Option<String>,
    sandbox_policy: SandboxPolicy,
}

impl CustomAgentTask {
    pub(crate) fn new(
        agent_name: String,
        instructions: String,
        model: Option<String>,
        sandbox_policy: SandboxPolicy,
    ) -> Self {
        Self {
            agent_name,
            instructions,
            model,
            sandbox_policy,
        }
    }
}

#[async_trait]
impl SessionTask for CustomAgentTask {
    fn kind(&self) -> TaskKind {
        TaskKind::Custom
    }

    async fn run(
        self: Arc<Self>,
        session: Arc<SessionTaskContext>,
        ctx: Arc<TurnContext>,
        input: Vec<UserInput>,
        cancellation_token: CancellationToken,
    ) -> Option<String> {
        // Start sub-agent conversation and get the receiver for events.
        let output = match start_custom_agent_conversation(
            self.agent_name.clone(),
            self.instructions.clone(),
            self.model.clone(),
            self.sandbox_policy.clone(),
            session.clone(),
            ctx.clone(),
            input,
            cancellation_token.clone(),
        )
        .await
        {
            Some(receiver) => {
                process_custom_agent_events(session.clone(), ctx.clone(), receiver).await
            }
            None => None,
        };
        // Custom agents don't have special completion handling like review
        output
    }
}

async fn start_custom_agent_conversation(
    _agent_name: String,
    instructions: String,
    model: Option<String>,
    sandbox_policy: SandboxPolicy,
    session: Arc<SessionTaskContext>,
    ctx: Arc<TurnContext>,
    input: Vec<UserInput>,
    cancellation_token: CancellationToken,
) -> Option<async_channel::Receiver<codex_protocol::protocol::Event>> {
    let config = ctx.client.config();
    let mut sub_agent_config = config.as_ref().clone();

    // Apply custom agent configuration
    sub_agent_config.sandbox_policy = sandbox_policy;
    sub_agent_config.base_instructions = Some(instructions);

    // Apply model override if specified
    if let Some(model_name) = model {
        sub_agent_config.model = Some(model_name);
    }

    // Use the agent name as the subagent identifier
    (run_codex_conversation_one_shot(
        sub_agent_config,
        session.auth_manager(),
        session.models_manager(),
        input,
        session.clone_session(),
        ctx.clone(),
        cancellation_token,
        None,
    )
    .await)
        .ok()
        .map(|io| io.rx_event)
}

async fn process_custom_agent_events(
    session: Arc<SessionTaskContext>,
    ctx: Arc<TurnContext>,
    receiver: async_channel::Receiver<codex_protocol::protocol::Event>,
) -> Option<String> {
    use codex_protocol::protocol::EventMsg;

    // Forward all events from the custom agent to the parent session
    while let Ok(event) = receiver.recv().await {
        match event.clone().msg {
            EventMsg::TaskComplete(_) => {
                // Forward the completion event
                session
                    .clone_session()
                    .send_event(ctx.as_ref(), event.msg)
                    .await;
                return None;
            }
            EventMsg::TurnAborted(_) => {
                // Forward the abort event
                session
                    .clone_session()
                    .send_event(ctx.as_ref(), event.msg)
                    .await;
                return None;
            }
            other => {
                // Forward all other events
                session
                    .clone_session()
                    .send_event(ctx.as_ref(), other)
                    .await;
            }
        }
    }
    None
}
