use std::future::Future;
use std::sync::Arc;

use super::SessionTask;
use super::SessionTaskContext;
use crate::codex::TurnContext;
use crate::error::Result as CodexResult;
use crate::state::TaskKind;
use async_trait::async_trait;
use codex_protocol::user_input::UserInput;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Default)]
pub(crate) struct CompactTask;

async fn await_compaction_or_cancellation<F>(
    cancellation_token: CancellationToken,
    compact_future: F,
) where
    F: Future<Output = CodexResult<()>>,
{
    tokio::select! {
        _ = cancellation_token.cancelled() => {}
        _ = compact_future => {}
    }
}

#[async_trait]
impl SessionTask for CompactTask {
    fn kind(&self) -> TaskKind {
        TaskKind::Compact
    }

    fn span_name(&self) -> &'static str {
        "session_task.compact"
    }

    async fn run(
        self: Arc<Self>,
        session: Arc<SessionTaskContext>,
        ctx: Arc<TurnContext>,
        input: Vec<UserInput>,
        cancellation_token: CancellationToken,
    ) -> Option<String> {
        let session = session.clone_session();
        if crate::compact::should_use_remote_compact_task(&ctx.provider) {
            let _ = session.services.session_telemetry.counter(
                "codex.task.compact",
                /*inc*/ 1,
                &[("type", "remote")],
            );
            await_compaction_or_cancellation(
                cancellation_token,
                crate::compact_remote::run_remote_compact_task(session.clone(), ctx),
            )
            .await;
        } else {
            let _ = session.services.session_telemetry.counter(
                "codex.task.compact",
                /*inc*/ 1,
                &[("type", "local")],
            );
            await_compaction_or_cancellation(
                cancellation_token,
                crate::compact::run_compact_task(session.clone(), ctx, input),
            )
            .await;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::Notify;

    use super::await_compaction_or_cancellation;
    use crate::error::Result as CodexResult;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn await_compaction_or_cancellation_returns_when_future_finishes() {
        let finished = Arc::new(Notify::new());
        let finished_clone = Arc::clone(&finished);

        let task = tokio::spawn(async move {
            await_compaction_or_cancellation(CancellationToken::new(), async move {
                finished_clone.notify_waiters();
                Ok(())
            })
            .await;
        });

        tokio::time::timeout(Duration::from_secs(1), finished.notified())
            .await
            .expect("compaction future should be awaited");
        task.await.expect("task should complete");
    }

    #[tokio::test]
    async fn await_compaction_or_cancellation_returns_when_cancelled() {
        let cancellation_token = CancellationToken::new();
        let child_token = cancellation_token.child_token();

        let task = tokio::spawn(async move {
            await_compaction_or_cancellation(child_token, pending::<CodexResult<()>>()).await;
        });

        cancellation_token.cancel();

        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("cancellation should unblock compaction waiting")
            .expect("task should complete cleanly");
    }
}
