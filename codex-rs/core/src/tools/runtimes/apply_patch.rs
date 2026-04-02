//! Apply Patch runtime: executes verified patches under the orchestrator.
//!
//! Assumes `apply_patch` verification/approval happened upstream. Reuses that
//! decision to avoid re-prompting, builds the self-invocation command for
//! `codex --codex-run-as-apply-patch`, and runs under the current
//! `SandboxAttempt` with a minimal environment.
use crate::error::CodexErr;
use crate::error::SandboxErr;
use crate::exec::ExecCapturePolicy;
use crate::exec::ExecToolCallOutput;
use crate::exec::StreamOutput;
use crate::exec::is_likely_sandbox_denied;
use crate::guardian::GuardianApprovalRequest;
use crate::guardian::review_approval_request;
use crate::guardian::routes_approval_to_guardian;
use crate::sandboxing::ExecOptions;
use crate::sandboxing::ExecRequest;
use crate::sandboxing::execute_env;
use crate::tools::sandboxing::Approvable;
use crate::tools::sandboxing::ApprovalCtx;
use crate::tools::sandboxing::ExecApprovalRequirement;
use crate::tools::sandboxing::SandboxAttempt;
use crate::tools::sandboxing::Sandboxable;
use crate::tools::sandboxing::ToolCtx;
use crate::tools::sandboxing::ToolError;
use crate::tools::sandboxing::ToolRuntime;
use crate::tools::sandboxing::with_cached_approval;
use codex_apply_patch::ApplyPatchAction;
use codex_apply_patch::CODEX_CORE_APPLY_PATCH_ARG1;
use codex_exec_server::ExecOutputStream as ExecutorOutputStream;
use codex_exec_server::ExecParams as ExecutorExecParams;
use codex_exec_server::ExecutorAttachment;
use codex_exec_server::ProcessId;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::FileChange;
use codex_protocol::protocol::ReviewDecision;
use codex_sandboxing::SandboxCommand;
use codex_sandboxing::SandboxablePreference;
use codex_utils_absolute_path::AbsolutePathBuf;
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

const EXEC_TIMEOUT_EXIT_CODE: i32 = 124;

#[derive(Debug)]
pub struct ApplyPatchRequest {
    pub action: ApplyPatchAction,
    pub file_paths: Vec<AbsolutePathBuf>,
    pub changes: std::collections::HashMap<PathBuf, FileChange>,
    pub exec_approval_requirement: ExecApprovalRequirement,
    pub additional_permissions: Option<PermissionProfile>,
    pub permissions_preapproved: bool,
    pub timeout_ms: Option<u64>,
}

pub struct ApplyPatchRuntime {
    executor_attachment: Arc<ExecutorAttachment>,
}

impl ApplyPatchRuntime {
    pub fn new(executor_attachment: Arc<ExecutorAttachment>) -> Self {
        Self {
            executor_attachment,
        }
    }

    fn build_guardian_review_request(
        req: &ApplyPatchRequest,
        call_id: &str,
    ) -> GuardianApprovalRequest {
        GuardianApprovalRequest::ApplyPatch {
            id: call_id.to_string(),
            cwd: req.action.cwd.clone(),
            files: req.file_paths.clone(),
            change_count: req.changes.len(),
            patch: req.action.patch.clone(),
        }
    }

    #[cfg(target_os = "windows")]
    fn build_sandbox_command(
        req: &ApplyPatchRequest,
        codex_home: &std::path::Path,
    ) -> Result<SandboxCommand, ToolError> {
        Ok(Self::build_sandbox_command_with_program(
            req,
            codex_windows_sandbox::resolve_current_exe_for_launch(codex_home, "codex.exe"),
        ))
    }

    #[cfg(not(target_os = "windows"))]
    fn build_sandbox_command(
        req: &ApplyPatchRequest,
        codex_self_exe: Option<&PathBuf>,
    ) -> Result<SandboxCommand, ToolError> {
        let exe = Self::resolve_apply_patch_program(codex_self_exe)?;
        Ok(Self::build_sandbox_command_with_program(req, exe))
    }

    #[cfg(not(target_os = "windows"))]
    fn resolve_apply_patch_program(codex_self_exe: Option<&PathBuf>) -> Result<PathBuf, ToolError> {
        if let Some(path) = codex_self_exe {
            return Ok(path.clone());
        }

        std::env::current_exe()
            .map_err(|e| ToolError::Rejected(format!("failed to determine codex exe: {e}")))
    }

    fn build_sandbox_command_with_program(req: &ApplyPatchRequest, exe: PathBuf) -> SandboxCommand {
        SandboxCommand {
            program: exe.into_os_string(),
            args: vec![
                CODEX_CORE_APPLY_PATCH_ARG1.to_string(),
                req.action.patch.clone(),
            ],
            cwd: req.action.cwd.clone(),
            // Run apply_patch with a minimal environment for determinism and to avoid leaks.
            env: HashMap::new(),
            additional_permissions: req.additional_permissions.clone(),
        }
    }

    fn stdout_stream(ctx: &ToolCtx) -> Option<crate::exec::StdoutStream> {
        Some(crate::exec::StdoutStream {
            sub_id: ctx.turn.sub_id.clone(),
            call_id: ctx.call_id.clone(),
            tx_event: ctx.session.get_tx_event(),
        })
    }

    async fn execute_request(
        &self,
        env: ExecRequest,
        ctx: &ToolCtx,
    ) -> Result<ExecToolCallOutput, CodexErr> {
        let start = Instant::now();
        let out = if self.executor_attachment.exec_server_url().is_some() {
            self.execute_request_remote(env, ctx).await?
        } else {
            execute_env(env, Self::stdout_stream(ctx)).await?
        };
        let duration = start.elapsed();

        let mut out = out;
        out.duration = duration;
        Ok(out)
    }

    async fn execute_request_remote(
        &self,
        env: ExecRequest,
        ctx: &ToolCtx,
    ) -> Result<ExecToolCallOutput, CodexErr> {
        let started = self
            .executor_attachment
            .get_exec_backend()
            .start(ExecutorExecParams {
                process_id: ProcessId::new(format!("apply-patch-{}", ctx.call_id)),
                argv: env.command.clone(),
                cwd: env.cwd.clone(),
                env: env.env.clone(),
                tty: false,
                arg0: env.arg0.clone(),
            })
            .await
            .map_err(|err| CodexErr::Io(io::Error::other(err)))?;

        let process = started.process;
        let mut wake_rx = process.subscribe_wake();
        let mut after_seq = None;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut aggregated_output = Vec::new();
        let mut exit_code = None;
        let mut timed_out = false;
        let expiration = env.expiration.clone();
        let capture_policy = env.capture_policy;
        let expiration_wait = async move {
            if matches!(capture_policy, ExecCapturePolicy::ShellTool) {
                expiration.wait().await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::pin!(expiration_wait);

        loop {
            let response = process
                .read(after_seq, /*max_bytes*/ None, /*wait_ms*/ Some(0))
                .await
                .map_err(|err| CodexErr::Io(io::Error::other(err)))?;

            for chunk in response.chunks {
                let bytes = chunk.chunk.into_inner();
                match chunk.stream {
                    ExecutorOutputStream::Stdout | ExecutorOutputStream::Pty => {
                        stdout.extend_from_slice(&bytes);
                    }
                    ExecutorOutputStream::Stderr => {
                        stderr.extend_from_slice(&bytes);
                    }
                }
                aggregated_output.extend_from_slice(&bytes);
            }

            if let Some(message) = response.failure {
                return Err(CodexErr::Io(io::Error::other(message)));
            }

            if response.exited {
                exit_code = response.exit_code;
            }

            if response.closed {
                break;
            }

            after_seq = response.next_seq.checked_sub(1);
            tokio::select! {
                wake_result = wake_rx.changed() => {
                    if wake_result.is_err() {
                        return Err(CodexErr::Io(io::Error::other(
                            "exec-server wake channel closed",
                        )));
                    }
                }
                _ = &mut expiration_wait, if !timed_out => {
                    process
                        .terminate()
                        .await
                        .map_err(|err| CodexErr::Io(io::Error::other(err)))?;
                    timed_out = true;
                    exit_code = Some(EXEC_TIMEOUT_EXIT_CODE);
                    break;
                }
            }
        }

        let output = ExecToolCallOutput {
            exit_code: exit_code.unwrap_or(-1),
            stdout: StreamOutput::new(String::from_utf8_lossy(&stdout).to_string()),
            stderr: StreamOutput::new(String::from_utf8_lossy(&stderr).to_string()),
            aggregated_output: StreamOutput::new(
                String::from_utf8_lossy(&aggregated_output).to_string(),
            ),
            duration: Duration::ZERO,
            timed_out,
        };

        if timed_out {
            return Err(CodexErr::Sandbox(SandboxErr::Timeout {
                output: Box::new(output),
            }));
        }

        if is_likely_sandbox_denied(env.sandbox, &output) {
            return Err(CodexErr::Sandbox(SandboxErr::Denied {
                output: Box::new(output),
                network_policy_decision: None,
            }));
        }

        Ok(output)
    }
}

impl Sandboxable for ApplyPatchRuntime {
    fn sandbox_preference(&self) -> SandboxablePreference {
        SandboxablePreference::Auto
    }
    fn escalate_on_failure(&self) -> bool {
        true
    }
}

impl Approvable<ApplyPatchRequest> for ApplyPatchRuntime {
    type ApprovalKey = AbsolutePathBuf;

    fn approval_keys(&self, req: &ApplyPatchRequest) -> Vec<Self::ApprovalKey> {
        req.file_paths.clone()
    }

    fn start_approval_async<'a>(
        &'a mut self,
        req: &'a ApplyPatchRequest,
        ctx: ApprovalCtx<'a>,
    ) -> BoxFuture<'a, ReviewDecision> {
        let session = ctx.session;
        let turn = ctx.turn;
        let call_id = ctx.call_id.to_string();
        let retry_reason = ctx.retry_reason.clone();
        let approval_keys = self.approval_keys(req);
        let changes = req.changes.clone();
        Box::pin(async move {
            if req.permissions_preapproved && retry_reason.is_none() {
                return ReviewDecision::Approved;
            }
            if routes_approval_to_guardian(turn) {
                let action = ApplyPatchRuntime::build_guardian_review_request(req, ctx.call_id);
                return review_approval_request(session, turn, action, retry_reason).await;
            }
            if let Some(reason) = retry_reason {
                let rx_approve = session
                    .request_patch_approval(
                        turn,
                        call_id,
                        changes.clone(),
                        Some(reason),
                        /*grant_root*/ None,
                    )
                    .await;
                return rx_approve.await.unwrap_or_default();
            }

            with_cached_approval(
                &session.services,
                "apply_patch",
                approval_keys,
                || async move {
                    let rx_approve = session
                        .request_patch_approval(
                            turn, call_id, changes, /*reason*/ None, /*grant_root*/ None,
                        )
                        .await;
                    rx_approve.await.unwrap_or_default()
                },
            )
            .await
        })
    }

    fn wants_no_sandbox_approval(&self, policy: AskForApproval) -> bool {
        match policy {
            AskForApproval::Never => false,
            AskForApproval::Granular(granular_config) => granular_config.allows_sandbox_approval(),
            AskForApproval::OnFailure => true,
            AskForApproval::OnRequest => true,
            AskForApproval::UnlessTrusted => true,
        }
    }

    // apply_patch approvals are decided upstream by assess_patch_safety.
    //
    // This override ensures the orchestrator runs the patch approval flow when required instead
    // of falling back to the global exec approval policy.
    fn exec_approval_requirement(
        &self,
        req: &ApplyPatchRequest,
    ) -> Option<ExecApprovalRequirement> {
        Some(req.exec_approval_requirement.clone())
    }
}

impl ToolRuntime<ApplyPatchRequest, ExecToolCallOutput> for ApplyPatchRuntime {
    async fn run(
        &mut self,
        req: &ApplyPatchRequest,
        attempt: &SandboxAttempt<'_>,
        ctx: &ToolCtx,
    ) -> Result<ExecToolCallOutput, ToolError> {
        #[cfg(target_os = "windows")]
        let command = Self::build_sandbox_command(req, &ctx.turn.config.codex_home)?;
        #[cfg(not(target_os = "windows"))]
        let command = Self::build_sandbox_command(req, ctx.turn.codex_self_exe.as_ref())?;
        let options = ExecOptions {
            expiration: req.timeout_ms.into(),
            capture_policy: ExecCapturePolicy::ShellTool,
        };
        let env = attempt
            .env_for(command, options, /*network*/ None)
            .map_err(|err| ToolError::Codex(err.into()))?;
        let out = self
            .execute_request(env, ctx)
            .await
            .map_err(ToolError::Codex)?;
        Ok(out)
    }
}

#[cfg(test)]
#[path = "apply_patch_tests.rs"]
mod tests;
