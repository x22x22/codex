use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use super::App;
use codex_app_server_client::InProcessAppServerClient;
use codex_app_server_client::InProcessServerEvent;
use codex_app_server_protocol::AppsListParams;
use codex_app_server_protocol::AppsListResponse;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::CollaborationModeListParams;
use codex_app_server_protocol::CollaborationModeListResponse;
use codex_app_server_protocol::CommandExecutionRequestApprovalResponse;
use codex_app_server_protocol::FeedbackUploadParams;
use codex_app_server_protocol::FeedbackUploadResponse;
use codex_app_server_protocol::FileChangeApprovalDecision;
use codex_app_server_protocol::FileChangeRequestApprovalResponse;
use codex_app_server_protocol::GetAccountParams;
use codex_app_server_protocol::GetAccountRateLimitsResponse;
use codex_app_server_protocol::GetAccountResponse;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::McpServerElicitationAction;
use codex_app_server_protocol::McpServerElicitationRequestResponse;
use codex_app_server_protocol::McpServerRefreshResponse;
use codex_app_server_protocol::ModelListParams;
use codex_app_server_protocol::ModelListResponse;
use codex_app_server_protocol::PermissionsRequestApprovalResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ReviewStartParams;
use codex_app_server_protocol::ReviewStartResponse;
use codex_app_server_protocol::ReviewTarget as ApiReviewTarget;
use codex_app_server_protocol::SandboxMode;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ServerRequest;
use codex_app_server_protocol::SkillsListParams;
use codex_app_server_protocol::SkillsListResponse;
use codex_app_server_protocol::ThreadBackgroundTerminalsCleanParams;
use codex_app_server_protocol::ThreadBackgroundTerminalsCleanResponse;
use codex_app_server_protocol::ThreadCompactStartParams;
use codex_app_server_protocol::ThreadCompactStartResponse;
use codex_app_server_protocol::ThreadForkParams;
use codex_app_server_protocol::ThreadForkResponse;
use codex_app_server_protocol::ThreadRealtimeAppendAudioParams;
use codex_app_server_protocol::ThreadRealtimeAppendAudioResponse;
use codex_app_server_protocol::ThreadRealtimeAppendTextParams;
use codex_app_server_protocol::ThreadRealtimeAppendTextResponse;
use codex_app_server_protocol::ThreadRealtimeStartParams;
use codex_app_server_protocol::ThreadRealtimeStartResponse;
use codex_app_server_protocol::ThreadRealtimeStopParams;
use codex_app_server_protocol::ThreadRealtimeStopResponse;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadRollbackParams;
use codex_app_server_protocol::ThreadRollbackResponse;
use codex_app_server_protocol::ThreadSetNameParams;
use codex_app_server_protocol::ThreadSetNameResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::ThreadUnsubscribeParams;
use codex_app_server_protocol::ThreadUnsubscribeResponse;
use codex_app_server_protocol::ToolRequestUserInputResponse as ApiToolRequestUserInputResponse;
use codex_app_server_protocol::TurnInterruptParams;
use codex_app_server_protocol::TurnInterruptResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::TurnSteerParams;
use codex_app_server_protocol::TurnSteerResponse;
use codex_core::config::Config;
use codex_protocol::ThreadId;
use codex_protocol::approvals::ElicitationAction;
use codex_protocol::config_types::CollaborationModeMask;
use codex_protocol::openai_models::ModelAvailabilityNux;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ModelUpgrade;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::ErrorEvent;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ReviewTarget;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionConfiguredEvent;
use codex_protocol::protocol::WarningEvent;
use codex_protocol::request_permissions::PermissionGrantScope;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::fs;
use tokio::io::AsyncReadExt;

use crate::thread_update::ThreadUpdate;

impl App {
    pub(super) async fn list_models_via_app_server(
        app_server_client: &InProcessAppServerClient,
    ) -> Result<Vec<ModelPreset>, String> {
        let response: ModelListResponse = send_request_with_response(
            app_server_client,
            ClientRequest::ModelList {
                request_id: RequestId::Integer(0),
                params: ModelListParams::default(),
            },
            "model/list",
        )
        .await?;
        Ok(response
            .data
            .into_iter()
            .map(model_preset_from_api)
            .collect())
    }

    pub(super) async fn list_collaboration_modes_via_app_server(
        app_server_client: &InProcessAppServerClient,
    ) -> Result<Vec<CollaborationModeMask>, String> {
        let response: CollaborationModeListResponse = send_request_with_response(
            app_server_client,
            ClientRequest::CollaborationModeList {
                request_id: RequestId::Integer(0),
                params: CollaborationModeListParams::default(),
            },
            "collaborationMode/list",
        )
        .await?;
        Ok(response
            .data
            .into_iter()
            .map(collaboration_mode_mask_from_api)
            .collect())
    }

    pub(super) async fn read_account_via_app_server(
        app_server_client: &InProcessAppServerClient,
    ) -> Result<Option<codex_app_server_protocol::Account>, String> {
        let response: GetAccountResponse = send_request_with_response(
            app_server_client,
            ClientRequest::GetAccount {
                request_id: RequestId::Integer(0),
                params: GetAccountParams {
                    refresh_token: false,
                },
            },
            "account/read",
        )
        .await?;
        Ok(response.account)
    }

    pub(super) async fn read_account_rate_limits_via_app_server(
        app_server_client: &InProcessAppServerClient,
    ) -> Result<GetAccountRateLimitsResponse, String> {
        send_request_with_response(
            app_server_client,
            ClientRequest::GetAccountRateLimits {
                request_id: RequestId::Integer(0),
                params: None,
            },
            "account/rateLimits/read",
        )
        .await
    }

    pub(super) async fn list_apps_via_app_server(
        app_server_client: &InProcessAppServerClient,
        thread_id: Option<String>,
        force_refetch: bool,
    ) -> Result<Vec<codex_app_server_protocol::AppInfo>, String> {
        let response: AppsListResponse = send_request_with_response(
            app_server_client,
            ClientRequest::AppsList {
                request_id: RequestId::Integer(0),
                params: AppsListParams {
                    cursor: None,
                    limit: None,
                    thread_id,
                    force_refetch,
                },
            },
            "app/list",
        )
        .await?;
        Ok(response.data)
    }

    pub(super) async fn upload_feedback_via_app_server(
        app_server_client: &InProcessAppServerClient,
        request_id: RequestId,
        classification: String,
        reason: Option<String>,
        thread_id: Option<String>,
        include_logs: bool,
        extra_log_files: Option<Vec<PathBuf>>,
    ) -> Result<String, String> {
        let response: FeedbackUploadResponse = send_request_with_response(
            app_server_client,
            ClientRequest::FeedbackUpload {
                request_id,
                params: FeedbackUploadParams {
                    classification,
                    reason,
                    thread_id,
                    include_logs,
                    extra_log_files,
                },
            },
            "feedback/upload",
        )
        .await?;
        Ok(response.thread_id)
    }

    pub(super) fn next_app_server_request_id(&mut self) -> RequestId {
        let request_id = self.next_app_server_request_id;
        self.next_app_server_request_id = self.next_app_server_request_id.saturating_add(1);
        RequestId::Integer(request_id)
    }

    pub(super) async fn thread_start_via_app_server(
        &mut self,
        app_server_client: &InProcessAppServerClient,
        config: &Config,
    ) -> Result<SessionConfiguredEvent, String> {
        let response: ThreadStartResponse = send_request_with_response(
            app_server_client,
            ClientRequest::ThreadStart {
                request_id: self.next_app_server_request_id(),
                params: thread_start_params_from_config(config),
            },
            "thread/start",
        )
        .await?;
        let (history_log_id, history_entry_count) = history_metadata(config).await;
        session_configured_from_thread_start_response(
            &response,
            history_log_id,
            history_entry_count,
        )
    }

    pub(super) async fn thread_resume_via_app_server(
        &mut self,
        app_server_client: &InProcessAppServerClient,
        config: &Config,
        path: PathBuf,
    ) -> Result<SessionConfiguredEvent, String> {
        let response: ThreadResumeResponse = send_request_with_response(
            app_server_client,
            ClientRequest::ThreadResume {
                request_id: self.next_app_server_request_id(),
                params: thread_resume_params_from_config(config, path),
            },
            "thread/resume",
        )
        .await?;
        let (history_log_id, history_entry_count) = history_metadata(config).await;
        session_configured_from_thread_resume_response(
            &response,
            history_log_id,
            history_entry_count,
        )
    }

    pub(super) async fn thread_fork_via_app_server(
        &mut self,
        app_server_client: &InProcessAppServerClient,
        config: &Config,
        path: PathBuf,
    ) -> Result<SessionConfiguredEvent, String> {
        let response: ThreadForkResponse = send_request_with_response(
            app_server_client,
            ClientRequest::ThreadFork {
                request_id: self.next_app_server_request_id(),
                params: thread_fork_params_from_config(config, path),
            },
            "thread/fork",
        )
        .await?;
        let (history_log_id, history_entry_count) = history_metadata(config).await;
        session_configured_from_thread_fork_response(&response, history_log_id, history_entry_count)
    }

    pub(super) async fn unsubscribe_thread_via_app_server(
        &mut self,
        app_server_client: &InProcessAppServerClient,
        thread_id: ThreadId,
    ) -> Result<(), String> {
        let _: ThreadUnsubscribeResponse = send_request_with_response(
            app_server_client,
            ClientRequest::ThreadUnsubscribe {
                request_id: self.next_app_server_request_id(),
                params: ThreadUnsubscribeParams {
                    thread_id: thread_id.to_string(),
                },
            },
            "thread/unsubscribe",
        )
        .await?;
        self.active_turn_ids.remove(&thread_id);
        Ok(())
    }

    pub(super) async fn submit_app_server_op(
        &mut self,
        app_server_client: &InProcessAppServerClient,
        thread_id: ThreadId,
        op: Op,
    ) -> bool {
        if let Err(err) = self
            .submit_app_server_op_inner(app_server_client, thread_id, op)
            .await
        {
            self.chat_widget.add_error_message(err);
            return false;
        }
        true
    }

    async fn submit_app_server_op_inner(
        &mut self,
        app_server_client: &InProcessAppServerClient,
        thread_id: ThreadId,
        op: Op,
    ) -> Result<(), String> {
        match op {
            Op::Interrupt => {
                let Some(turn_id) = self.active_turn_ids.get(&thread_id).cloned() else {
                    return Err("No active turn to interrupt.".to_string());
                };
                let _: TurnInterruptResponse = send_request_with_response(
                    app_server_client,
                    ClientRequest::TurnInterrupt {
                        request_id: self.next_app_server_request_id(),
                        params: TurnInterruptParams {
                            thread_id: thread_id.to_string(),
                            turn_id,
                        },
                    },
                    "turn/interrupt",
                )
                .await?;
            }
            Op::CleanBackgroundTerminals => {
                let _: ThreadBackgroundTerminalsCleanResponse = send_request_with_response(
                    app_server_client,
                    ClientRequest::ThreadBackgroundTerminalsClean {
                        request_id: self.next_app_server_request_id(),
                        params: ThreadBackgroundTerminalsCleanParams {
                            thread_id: thread_id.to_string(),
                        },
                    },
                    "thread/backgroundTerminals/clean",
                )
                .await?;
            }
            Op::RealtimeConversationStart(params) => {
                let _: ThreadRealtimeStartResponse = send_request_with_response(
                    app_server_client,
                    ClientRequest::ThreadRealtimeStart {
                        request_id: self.next_app_server_request_id(),
                        params: ThreadRealtimeStartParams {
                            thread_id: thread_id.to_string(),
                            prompt: params.prompt,
                            session_id: params.session_id,
                        },
                    },
                    "thread/realtime/start",
                )
                .await?;
            }
            Op::RealtimeConversationAudio(params) => {
                let _: ThreadRealtimeAppendAudioResponse = send_request_with_response(
                    app_server_client,
                    ClientRequest::ThreadRealtimeAppendAudio {
                        request_id: self.next_app_server_request_id(),
                        params: ThreadRealtimeAppendAudioParams {
                            thread_id: thread_id.to_string(),
                            audio: params.frame.into(),
                        },
                    },
                    "thread/realtime/appendAudio",
                )
                .await?;
            }
            Op::RealtimeConversationText(params) => {
                let _: ThreadRealtimeAppendTextResponse = send_request_with_response(
                    app_server_client,
                    ClientRequest::ThreadRealtimeAppendText {
                        request_id: self.next_app_server_request_id(),
                        params: ThreadRealtimeAppendTextParams {
                            thread_id: thread_id.to_string(),
                            text: params.text,
                        },
                    },
                    "thread/realtime/appendText",
                )
                .await?;
            }
            Op::RealtimeConversationClose => {
                let _: ThreadRealtimeStopResponse = send_request_with_response(
                    app_server_client,
                    ClientRequest::ThreadRealtimeStop {
                        request_id: self.next_app_server_request_id(),
                        params: ThreadRealtimeStopParams {
                            thread_id: thread_id.to_string(),
                        },
                    },
                    "thread/realtime/stop",
                )
                .await?;
            }
            Op::UserTurn {
                items,
                cwd,
                approval_policy,
                sandbox_policy,
                model,
                effort,
                summary,
                service_tier,
                final_output_json_schema,
                collaboration_mode,
                personality,
            } => {
                if let Some(expected_turn_id) = self.active_turn_ids.get(&thread_id).cloned() {
                    let _: TurnSteerResponse = send_request_with_response(
                        app_server_client,
                        ClientRequest::TurnSteer {
                            request_id: self.next_app_server_request_id(),
                            params: TurnSteerParams {
                                thread_id: thread_id.to_string(),
                                input: items.into_iter().map(Into::into).collect(),
                                expected_turn_id,
                            },
                        },
                        "turn/steer",
                    )
                    .await?;
                } else {
                    let response: TurnStartResponse = send_request_with_response(
                        app_server_client,
                        ClientRequest::TurnStart {
                            request_id: self.next_app_server_request_id(),
                            params: TurnStartParams {
                                thread_id: thread_id.to_string(),
                                input: items.into_iter().map(Into::into).collect(),
                                cwd: Some(cwd),
                                approval_policy: Some(approval_policy.into()),
                                approvals_reviewer: Some(self.config.approvals_reviewer.into()),
                                sandbox_policy: Some(sandbox_policy.into()),
                                model: Some(model),
                                service_tier,
                                effort,
                                summary,
                                personality,
                                output_schema: final_output_json_schema,
                                collaboration_mode,
                            },
                        },
                        "turn/start",
                    )
                    .await?;
                    self.active_turn_ids.insert(thread_id, response.turn.id);
                }
            }
            Op::UserInput {
                items,
                final_output_json_schema,
            } => {
                if let Some(expected_turn_id) = self.active_turn_ids.get(&thread_id).cloned() {
                    let _: TurnSteerResponse = send_request_with_response(
                        app_server_client,
                        ClientRequest::TurnSteer {
                            request_id: self.next_app_server_request_id(),
                            params: TurnSteerParams {
                                thread_id: thread_id.to_string(),
                                input: items.into_iter().map(Into::into).collect(),
                                expected_turn_id,
                            },
                        },
                        "turn/steer",
                    )
                    .await?;
                } else {
                    let response: TurnStartResponse = send_request_with_response(
                        app_server_client,
                        ClientRequest::TurnStart {
                            request_id: self.next_app_server_request_id(),
                            params: TurnStartParams {
                                thread_id: thread_id.to_string(),
                                input: items.into_iter().map(Into::into).collect(),
                                cwd: Some(self.config.cwd.clone()),
                                approval_policy: Some(
                                    self.config.permissions.approval_policy.value().into(),
                                ),
                                approvals_reviewer: Some(self.config.approvals_reviewer.into()),
                                sandbox_policy: Some(
                                    self.config.permissions.sandbox_policy.get().clone().into(),
                                ),
                                model: self.config.model.clone(),
                                service_tier: self.config.service_tier.map(Some),
                                effort: self.config.model_reasoning_effort,
                                summary: None,
                                personality: self.config.personality,
                                output_schema: final_output_json_schema,
                                collaboration_mode: None,
                            },
                        },
                        "turn/start",
                    )
                    .await?;
                    self.active_turn_ids.insert(thread_id, response.turn.id);
                }
            }
            Op::ExecApproval { id, decision, .. } => {
                let Some(request_id) = self.pending_exec_approval_request_ids.remove(&id) else {
                    return Err(format!(
                        "Missing app-server approval request for exec approval {id}."
                    ));
                };
                let response = CommandExecutionRequestApprovalResponse {
                    decision: decision.into(),
                };
                resolve_server_request(
                    app_server_client,
                    request_id,
                    response,
                    "item/commandExecution/requestApproval",
                )
                .await?;
            }
            Op::PatchApproval { id, decision } => {
                let Some(request_id) = self.pending_patch_approval_request_ids.remove(&id) else {
                    return Err(format!(
                        "Missing app-server approval request for patch approval {id}."
                    ));
                };
                let response = FileChangeRequestApprovalResponse {
                    decision: review_decision_to_file_change_decision(decision),
                };
                resolve_server_request(
                    app_server_client,
                    request_id,
                    response,
                    "item/fileChange/requestApproval",
                )
                .await?;
            }
            Op::ResolveElicitation {
                server_name,
                request_id,
                decision,
                content,
                meta,
            } => {
                let key = (
                    server_name.clone(),
                    mcp_request_id_to_app_server_request_id(&request_id),
                );
                let Some(server_request_id) = self.pending_elicitation_request_ids.remove(&key)
                else {
                    return Err(format!(
                        "Missing app-server request for MCP elicitation {server_name}/{request_id}."
                    ));
                };
                let response = McpServerElicitationRequestResponse {
                    action: elicitation_action_to_api(decision),
                    content,
                    meta,
                };
                resolve_server_request(
                    app_server_client,
                    server_request_id,
                    response,
                    "mcpServer/elicitation/request",
                )
                .await?;
            }
            Op::UserInputAnswer { id, response } => {
                let Some(request_id) = self.pending_user_input_request_ids.remove(&id) else {
                    return Err(format!(
                        "Missing app-server request for user input turn {id}."
                    ));
                };
                let response: ApiToolRequestUserInputResponse =
                    serde_json::from_value(serde_json::to_value(response).map_err(|err| {
                        format!("failed to encode request_user_input response: {err}")
                    })?)
                    .map_err(|err| {
                        format!("failed to convert request_user_input response: {err}")
                    })?;
                resolve_server_request(
                    app_server_client,
                    request_id,
                    response,
                    "item/tool/requestUserInput",
                )
                .await?;
            }
            Op::RequestPermissionsResponse { id, response } => {
                let Some(request_id) = self.pending_permissions_request_ids.remove(&id) else {
                    return Err(format!(
                        "Missing app-server request for permissions approval {id}."
                    ));
                };
                let response = PermissionsRequestApprovalResponse {
                    permissions: granted_permission_profile_from_request(response.permissions),
                    scope: permission_grant_scope_to_api(response.scope),
                };
                resolve_server_request(
                    app_server_client,
                    request_id,
                    response,
                    "item/permissions/requestApproval",
                )
                .await?;
            }
            Op::DynamicToolResponse { id, response } => {
                let Some(request_id) = self.pending_dynamic_tool_request_ids.remove(&id) else {
                    return Err(format!(
                        "Missing app-server request for dynamic tool call {id}."
                    ));
                };
                resolve_server_request(app_server_client, request_id, response, "item/tool/call")
                    .await?;
            }
            Op::AddToHistory { text } => {
                let _ = (thread_id, text);
                // TODO(app-server): expose message-history append/lookup APIs and migrate
                // `AddToHistory`/`GetHistoryEntryRequest` together.
            }
            Op::ListSkills { cwds, force_reload } => {
                let response: SkillsListResponse = send_request_with_response(
                    app_server_client,
                    ClientRequest::SkillsList {
                        request_id: self.next_app_server_request_id(),
                        params: SkillsListParams {
                            cwds,
                            force_reload,
                            per_cwd_extra_user_roots: None,
                        },
                    },
                    "skills/list",
                )
                .await?;
                self.handle_skills_list_response_now(response.data);
            }
            Op::RefreshMcpServers { config } => {
                let _: McpServerRefreshResponse = send_request_with_response(
                    app_server_client,
                    ClientRequest::McpServerRefresh {
                        request_id: self.next_app_server_request_id(),
                        params: None,
                    },
                    "config/mcpServer/reload",
                )
                .await?;
                let _ = config;
            }
            Op::ReloadUserConfig | Op::OverrideTurnContext { .. } => {
                // TODO(app-server): add a thread-scoped override/context refresh API so the TUI
                // does not need to treat these as local-only state updates.
            }
            Op::Compact => {
                let _: ThreadCompactStartResponse = send_request_with_response(
                    app_server_client,
                    ClientRequest::ThreadCompactStart {
                        request_id: self.next_app_server_request_id(),
                        params: ThreadCompactStartParams {
                            thread_id: thread_id.to_string(),
                        },
                    },
                    "thread/compact/start",
                )
                .await?;
            }
            Op::SetThreadName { name } => {
                let _: ThreadSetNameResponse = send_request_with_response(
                    app_server_client,
                    ClientRequest::ThreadSetName {
                        request_id: self.next_app_server_request_id(),
                        params: ThreadSetNameParams {
                            thread_id: thread_id.to_string(),
                            name,
                        },
                    },
                    "thread/name/set",
                )
                .await?;
            }
            Op::ThreadRollback { num_turns } => {
                let _: ThreadRollbackResponse = send_request_with_response(
                    app_server_client,
                    ClientRequest::ThreadRollback {
                        request_id: self.next_app_server_request_id(),
                        params: ThreadRollbackParams {
                            thread_id: thread_id.to_string(),
                            num_turns,
                        },
                    },
                    "thread/rollback",
                )
                .await?;
                self.route_thread_update(ThreadUpdate::ThreadRolledBack { num_turns })
                    .await;
            }
            Op::Review { review_request } => {
                let response: ReviewStartResponse = send_request_with_response(
                    app_server_client,
                    ClientRequest::ReviewStart {
                        request_id: self.next_app_server_request_id(),
                        params: ReviewStartParams {
                            thread_id: thread_id.to_string(),
                            target: review_target_to_api(review_request.target),
                            delivery: None,
                        },
                    },
                    "review/start",
                )
                .await?;
                self.active_turn_ids.insert(thread_id, response.turn.id);
            }
            Op::Shutdown => {
                self.unsubscribe_thread_via_app_server(app_server_client, thread_id)
                    .await?;
            }
            Op::ListCustomPrompts
            | Op::Undo
            | Op::DropMemories
            | Op::UpdateMemories
            | Op::RunUserShellCommand { .. }
            | Op::GetHistoryEntryRequest { .. }
            | Op::ListMcpTools => {
                // TODO(app-server): migrate these legacy-only TUI features once app-server grows
                // equivalent APIs. Until then, keep routing the still-emitted TUI ops through the
                // shared in-process thread runtime so existing behavior does not regress.
                app_server_client
                    .submit_legacy_thread_op(thread_id, op)
                    .await
                    .map_err(|err| format!("failed to submit legacy TUI op: {err}"))?;
            }
            Op::ListRemoteSkills { .. } | Op::DownloadRemoteSkill { .. } | Op::ListModels => {
                return Err(format!(
                    "This TUI feature is not yet routed through app-server: {}",
                    legacy_op_name(&op)
                ));
            }
            _ => {
                return Err(format!(
                    "This TUI operation is not yet supported over app-server: {}",
                    legacy_op_name(&op)
                ));
            }
        }

        Ok(())
    }

    fn note_server_request(&mut self, request: &ServerRequest) {
        match request {
            ServerRequest::CommandExecutionRequestApproval { request_id, params } => {
                let approval_id = params
                    .approval_id
                    .clone()
                    .unwrap_or_else(|| params.item_id.clone());
                self.pending_exec_approval_request_ids
                    .insert(approval_id, request_id.clone());
            }
            ServerRequest::FileChangeRequestApproval { request_id, params } => {
                self.pending_patch_approval_request_ids
                    .insert(params.item_id.clone(), request_id.clone());
            }
            ServerRequest::McpServerElicitationRequest { request_id, params } => {
                self.pending_elicitation_request_ids.insert(
                    (params.server_name.clone(), request_id.clone()),
                    request_id.clone(),
                );
            }
            ServerRequest::PermissionsRequestApproval { request_id, params } => {
                self.pending_permissions_request_ids
                    .insert(params.item_id.clone(), request_id.clone());
            }
            ServerRequest::ToolRequestUserInput { request_id, params } => {
                self.pending_user_input_request_ids
                    .insert(params.turn_id.clone(), request_id.clone());
            }
            ServerRequest::DynamicToolCall { request_id, params } => {
                self.pending_dynamic_tool_request_ids
                    .insert(params.call_id.clone(), request_id.clone());
            }
            ServerRequest::ApplyPatchApproval { .. }
            | ServerRequest::ExecCommandApproval { .. } => {
                // These legacy server requests are not expected on the turn/start path that the
                // TUI now uses. Keep them explicit so regressions are obvious during cleanup.
            }
            ServerRequest::ChatgptAuthTokensRefresh { .. } => {}
        }
    }

    async fn route_thread_update(&mut self, update: ThreadUpdate) {
        let Some(thread_id_text) = update.thread_id() else {
            self.handle_thread_update_now(update);
            return;
        };
        let Ok(thread_id) = ThreadId::from_string(&thread_id_text) else {
            tracing::warn!("failed to parse thread id for thread update");
            return;
        };
        if self.primary_thread_id.is_none() || self.primary_thread_id == Some(thread_id) {
            if let Err(err) = self.enqueue_primary_update(update).await {
                tracing::warn!("{err}");
            }
        } else if let Err(err) = self.handle_routed_thread_update(thread_id, update).await {
            tracing::warn!("{err}");
        }
    }

    async fn handle_server_notification(
        &mut self,
        app_server_client: &InProcessAppServerClient,
        notification: ServerNotification,
    ) {
        match notification {
            ServerNotification::TurnStarted(payload) => {
                let active_turn_id = payload.turn.id.clone();
                if let Ok(thread_id) = ThreadId::from_string(&payload.thread_id) {
                    self.active_turn_ids.insert(thread_id, active_turn_id);
                }
                self.route_thread_update(ThreadUpdate::TurnStarted(payload))
                    .await;
            }
            ServerNotification::TurnCompleted(payload) => {
                if let Ok(thread_id) = ThreadId::from_string(&payload.thread_id)
                    && self.active_turn_ids.get(&thread_id) == Some(&payload.turn.id)
                {
                    self.active_turn_ids.remove(&thread_id);
                }
                self.route_thread_update(ThreadUpdate::TurnCompleted(payload))
                    .await;
            }
            ServerNotification::ThreadStarted(payload) => {
                let agent_nickname = payload.thread.agent_nickname.clone();
                let agent_role = payload.thread.agent_role.clone();
                if let Ok(thread_id) = ThreadId::from_string(&payload.thread.id) {
                    self.upsert_agent_picker_thread(thread_id, agent_nickname, agent_role, false);
                }
                self.route_thread_update(ThreadUpdate::ThreadStarted(payload))
                    .await;
            }
            ServerNotification::ThreadStatusChanged(payload) => {
                self.route_thread_update(ThreadUpdate::ThreadStatusChanged(payload))
                    .await;
            }
            ServerNotification::ThreadClosed(payload) => {
                if let Ok(thread_id) = ThreadId::from_string(&payload.thread_id) {
                    self.mark_agent_picker_thread_closed(thread_id);
                    self.active_turn_ids.remove(&thread_id);
                }
                self.route_thread_update(ThreadUpdate::ThreadClosed(payload))
                    .await;
            }
            ServerNotification::ThreadNameUpdated(payload) => {
                if let Ok(thread_id) = ThreadId::from_string(&payload.thread_id)
                    && let Some(channel) = self.thread_event_channels.get(&thread_id)
                {
                    let mut store = channel.store.lock().await;
                    if let Some(session) = store.session_configured.as_mut() {
                        session.thread_name = payload.thread_name.clone();
                    }
                }
                self.route_thread_update(ThreadUpdate::ThreadNameUpdated(payload))
                    .await;
            }
            ServerNotification::ThreadTokenUsageUpdated(payload) => {
                self.route_thread_update(ThreadUpdate::ThreadTokenUsageUpdated(payload))
                    .await;
            }
            ServerNotification::TurnDiffUpdated(payload) => {
                self.route_thread_update(ThreadUpdate::TurnDiffUpdated(payload))
                    .await;
            }
            ServerNotification::TurnPlanUpdated(payload) => {
                self.route_thread_update(ThreadUpdate::TurnPlanUpdated(payload))
                    .await;
            }
            ServerNotification::ItemStarted(payload) => {
                self.route_thread_update(ThreadUpdate::ItemStarted(payload))
                    .await;
            }
            ServerNotification::ItemGuardianApprovalReviewStarted(payload) => {
                self.route_thread_update(ThreadUpdate::ItemGuardianApprovalReviewStarted(payload))
                    .await;
            }
            ServerNotification::ItemGuardianApprovalReviewCompleted(payload) => {
                self.route_thread_update(ThreadUpdate::ItemGuardianApprovalReviewCompleted(
                    payload,
                ))
                .await;
            }
            ServerNotification::ItemCompleted(payload) => {
                self.route_thread_update(ThreadUpdate::ItemCompleted(payload))
                    .await;
            }
            ServerNotification::AgentMessageDelta(payload) => {
                self.route_thread_update(ThreadUpdate::AgentMessageDelta(payload))
                    .await;
            }
            ServerNotification::PlanDelta(payload) => {
                self.route_thread_update(ThreadUpdate::PlanDelta(payload))
                    .await;
            }
            ServerNotification::ReasoningSummaryTextDelta(payload) => {
                self.route_thread_update(ThreadUpdate::ReasoningSummaryTextDelta(payload))
                    .await;
            }
            ServerNotification::ReasoningSummaryPartAdded(payload) => {
                self.route_thread_update(ThreadUpdate::ReasoningSummaryPartAdded(payload))
                    .await;
            }
            ServerNotification::ReasoningTextDelta(payload) => {
                self.route_thread_update(ThreadUpdate::ReasoningTextDelta(payload))
                    .await;
            }
            ServerNotification::TerminalInteraction(payload) => {
                self.route_thread_update(ThreadUpdate::TerminalInteraction(payload))
                    .await;
            }
            ServerNotification::CommandExecutionOutputDelta(payload) => {
                self.route_thread_update(ThreadUpdate::CommandExecutionOutputDelta(payload))
                    .await;
            }
            ServerNotification::FileChangeOutputDelta(payload) => {
                self.route_thread_update(ThreadUpdate::FileChangeOutputDelta(payload))
                    .await;
            }
            ServerNotification::McpToolCallProgress(payload) => {
                self.route_thread_update(ThreadUpdate::McpToolCallProgress(payload))
                    .await;
            }
            ServerNotification::HookStarted(payload) => {
                self.route_thread_update(ThreadUpdate::HookStarted(payload))
                    .await;
            }
            ServerNotification::HookCompleted(payload) => {
                self.route_thread_update(ThreadUpdate::HookCompleted(payload))
                    .await;
            }
            ServerNotification::Error(payload) => {
                self.route_thread_update(ThreadUpdate::Error(payload)).await;
            }
            ServerNotification::ModelRerouted(payload) => {
                self.route_thread_update(ThreadUpdate::ModelRerouted(payload))
                    .await;
            }
            ServerNotification::DeprecationNotice(payload) => {
                self.route_thread_update(ThreadUpdate::DeprecationNotice(payload))
                    .await;
            }
            ServerNotification::ThreadRealtimeStarted(payload) => {
                self.route_thread_update(ThreadUpdate::ThreadRealtimeStarted(payload))
                    .await;
            }
            ServerNotification::ThreadRealtimeItemAdded(payload) => {
                self.route_thread_update(ThreadUpdate::ThreadRealtimeItemAdded(payload))
                    .await;
            }
            ServerNotification::ThreadRealtimeOutputAudioDelta(payload) => {
                self.route_thread_update(ThreadUpdate::ThreadRealtimeOutputAudioDelta(payload))
                    .await;
            }
            ServerNotification::ThreadRealtimeError(payload) => {
                self.route_thread_update(ThreadUpdate::ThreadRealtimeError(payload))
                    .await;
            }
            ServerNotification::ThreadRealtimeClosed(payload) => {
                self.route_thread_update(ThreadUpdate::ThreadRealtimeClosed(payload))
                    .await;
            }
            ServerNotification::AccountUpdated(_) => {
                match Self::read_account_via_app_server(app_server_client).await {
                    Ok(account) => {
                        self.feedback_audience =
                            crate::account_state::feedback_audience_from_account(account.as_ref());
                        self.chat_widget.set_account(account);
                        self.refresh_status_line();
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "failed to refresh account via app-server");
                    }
                }
            }
            ServerNotification::AccountRateLimitsUpdated(payload) => {
                self.chat_widget
                    .on_rate_limit_snapshot(Some(rate_limit_snapshot_from_api(
                        payload.rate_limits,
                    )));
            }
            ServerNotification::AppListUpdated(payload) => {
                self.chat_widget.on_connectors_loaded(
                    Ok(crate::app_event::ConnectorsSnapshot {
                        connectors: payload.data,
                    }),
                    true,
                );
            }
            ServerNotification::SkillsChanged(_) => {
                if let Some(thread_id) = self.primary_thread_id {
                    let _ = self
                        .submit_app_server_op_inner(
                            app_server_client,
                            thread_id,
                            Op::ListSkills {
                                cwds: Vec::new(),
                                force_reload: true,
                            },
                        )
                        .await;
                }
            }
            _ => {}
        }
    }

    pub(super) async fn handle_app_server_event(
        &mut self,
        app_server_client: &InProcessAppServerClient,
        event: InProcessServerEvent,
    ) {
        match event {
            InProcessServerEvent::Lagged { skipped } => {
                tracing::warn!(
                    skipped,
                    "app-server event consumer lagged; dropping ignored events"
                );
                self.chat_widget
                    .add_error_message(lagged_event_warning_message(skipped));
            }
            InProcessServerEvent::ServerNotification(notification) => {
                self.handle_server_notification(app_server_client, notification)
                    .await;
            }
            InProcessServerEvent::LegacyNotification(notification) => {
                self.handle_legacy_notification(notification);
            }
            InProcessServerEvent::ServerRequest(request) => {
                self.note_server_request(&request);
                match request.clone() {
                    ServerRequest::CommandExecutionRequestApproval { request_id, params } => {
                        self.route_thread_update(ThreadUpdate::CommandExecutionRequestApproval {
                            _request_id: request_id,
                            params,
                        })
                        .await;
                    }
                    ServerRequest::FileChangeRequestApproval { request_id, params } => {
                        self.route_thread_update(ThreadUpdate::FileChangeRequestApproval {
                            _request_id: request_id,
                            params,
                        })
                        .await;
                    }
                    ServerRequest::McpServerElicitationRequest { request_id, params } => {
                        self.route_thread_update(ThreadUpdate::McpServerElicitationRequest {
                            request_id,
                            params,
                        })
                        .await;
                    }
                    ServerRequest::PermissionsRequestApproval { request_id, params } => {
                        self.route_thread_update(ThreadUpdate::PermissionsRequestApproval {
                            _request_id: request_id,
                            params,
                        })
                        .await;
                    }
                    ServerRequest::ToolRequestUserInput { request_id, params } => {
                        self.route_thread_update(ThreadUpdate::ToolRequestUserInput {
                            _request_id: request_id,
                            params,
                        })
                        .await;
                    }
                    ServerRequest::DynamicToolCall { request_id, params } => {
                        self.route_thread_update(ThreadUpdate::DynamicToolCall {
                            _request_id: request_id,
                            params,
                        })
                        .await;
                    }
                    _ => {}
                }
                if let ServerRequest::ChatgptAuthTokensRefresh {
                    request_id,
                    params: _,
                } = request
                    && let Err(err) = self
                        .reject_app_server_request(
                            app_server_client,
                            request_id,
                            "TUI does not yet handle auth refresh server requests".to_string(),
                        )
                        .await
                {
                    tracing::warn!("{err}");
                }
            }
        }
    }

    fn handle_legacy_notification(&mut self, notification: JSONRPCNotification) {
        let Some(params) = notification.params else {
            return;
        };
        let Ok(event) = serde_json::from_value::<Event>(params) else {
            tracing::debug!(
                method = notification.method,
                "failed to decode legacy notification"
            );
            return;
        };

        match event.msg {
            EventMsg::GetHistoryEntryResponse(event) => {
                self.chat_widget
                    .handle_get_history_entry_response_now(event);
            }
            EventMsg::ListCustomPromptsResponse(event) => {
                self.chat_widget
                    .handle_list_custom_prompts_response_now(event);
            }
            EventMsg::McpListToolsResponse(event) => {
                self.chat_widget.handle_list_mcp_tools_response_now(event);
            }
            EventMsg::Warning(WarningEvent { message }) => {
                self.chat_widget.handle_warning_now(message);
            }
            EventMsg::Error(ErrorEvent { message, .. }) => {
                self.chat_widget.add_error_message(message);
            }
            _ => {}
        }
    }

    async fn reject_app_server_request(
        &self,
        app_server_client: &InProcessAppServerClient,
        request_id: RequestId,
        reason: String,
    ) -> Result<(), String> {
        app_server_client
            .reject_server_request(
                request_id,
                JSONRPCErrorError {
                    code: -32000,
                    message: reason,
                    data: None,
                },
            )
            .await
            .map_err(|err| format!("failed to reject app-server request: {err}"))
    }
}

async fn send_request_with_response<T>(
    client: &InProcessAppServerClient,
    request: ClientRequest,
    method: &str,
) -> Result<T, String>
where
    T: DeserializeOwned,
{
    client.request_typed(request).await.map_err(|err| {
        if method.is_empty() {
            err.to_string()
        } else {
            format!("{method}: {err}")
        }
    })
}

fn thread_start_params_from_config(config: &Config) -> ThreadStartParams {
    ThreadStartParams {
        model: config.model.clone(),
        model_provider: Some(config.model_provider_id.clone()),
        service_tier: config.service_tier.map(Some),
        cwd: Some(config.cwd.to_string_lossy().to_string()),
        approval_policy: Some(config.permissions.approval_policy.value().into()),
        approvals_reviewer: Some(config.approvals_reviewer.into()),
        sandbox: sandbox_mode_from_policy(config.permissions.sandbox_policy.get()),
        config: config_request_overrides_from_config(config),
        base_instructions: None,
        developer_instructions: None,
        personality: config.personality,
        ephemeral: Some(config.ephemeral),
        dynamic_tools: None,
        mock_experimental_field: None,
        experimental_raw_events: false,
        persist_extended_history: true,
        service_name: None,
    }
}

fn thread_resume_params_from_config(config: &Config, path: PathBuf) -> ThreadResumeParams {
    ThreadResumeParams {
        thread_id: "resume".to_string(),
        history: None,
        path: Some(path),
        model: config.model.clone(),
        model_provider: Some(config.model_provider_id.clone()),
        service_tier: config.service_tier.map(Some),
        cwd: Some(config.cwd.to_string_lossy().to_string()),
        approval_policy: Some(config.permissions.approval_policy.value().into()),
        approvals_reviewer: Some(config.approvals_reviewer.into()),
        sandbox: sandbox_mode_from_policy(config.permissions.sandbox_policy.get()),
        config: config_request_overrides_from_config(config),
        base_instructions: None,
        developer_instructions: None,
        personality: config.personality,
        persist_extended_history: true,
    }
}

fn thread_fork_params_from_config(config: &Config, path: PathBuf) -> ThreadForkParams {
    ThreadForkParams {
        thread_id: "fork".to_string(),
        path: Some(path),
        model: config.model.clone(),
        model_provider: Some(config.model_provider_id.clone()),
        service_tier: config.service_tier.map(Some),
        cwd: Some(config.cwd.to_string_lossy().to_string()),
        approval_policy: Some(config.permissions.approval_policy.value().into()),
        approvals_reviewer: Some(config.approvals_reviewer.into()),
        sandbox: sandbox_mode_from_policy(config.permissions.sandbox_policy.get()),
        config: config_request_overrides_from_config(config),
        base_instructions: None,
        developer_instructions: None,
        ephemeral: false,
        persist_extended_history: true,
    }
}

fn config_request_overrides_from_config(config: &Config) -> Option<HashMap<String, Value>> {
    config
        .active_profile
        .as_ref()
        .map(|profile| HashMap::from([("profile".to_string(), Value::String(profile.clone()))]))
}

fn sandbox_mode_from_policy(sandbox_policy: &SandboxPolicy) -> Option<SandboxMode> {
    match sandbox_policy {
        SandboxPolicy::DangerFullAccess => Some(SandboxMode::DangerFullAccess),
        SandboxPolicy::ReadOnly { .. } => Some(SandboxMode::ReadOnly),
        SandboxPolicy::WorkspaceWrite { .. } => Some(SandboxMode::WorkspaceWrite),
        SandboxPolicy::ExternalSandbox { .. } => None,
    }
}

fn session_configured_from_thread_start_response(
    response: &ThreadStartResponse,
    history_log_id: u64,
    history_entry_count: usize,
) -> Result<SessionConfiguredEvent, String> {
    session_configured_from_thread_response(
        &response.thread.id,
        response.thread.name.clone(),
        response.thread.path.clone(),
        response.model.clone(),
        response.model_provider.clone(),
        response.service_tier,
        response.approval_policy.to_core(),
        response.approvals_reviewer.to_core(),
        response.sandbox.to_core(),
        response.cwd.clone(),
        response.reasoning_effort,
        history_log_id,
        history_entry_count,
    )
}

fn session_configured_from_thread_resume_response(
    response: &ThreadResumeResponse,
    history_log_id: u64,
    history_entry_count: usize,
) -> Result<SessionConfiguredEvent, String> {
    session_configured_from_thread_response(
        &response.thread.id,
        response.thread.name.clone(),
        response.thread.path.clone(),
        response.model.clone(),
        response.model_provider.clone(),
        response.service_tier,
        response.approval_policy.to_core(),
        response.approvals_reviewer.to_core(),
        response.sandbox.to_core(),
        response.cwd.clone(),
        response.reasoning_effort,
        history_log_id,
        history_entry_count,
    )
}

fn session_configured_from_thread_fork_response(
    response: &ThreadForkResponse,
    history_log_id: u64,
    history_entry_count: usize,
) -> Result<SessionConfiguredEvent, String> {
    session_configured_from_thread_response(
        &response.thread.id,
        response.thread.name.clone(),
        response.thread.path.clone(),
        response.model.clone(),
        response.model_provider.clone(),
        response.service_tier,
        response.approval_policy.to_core(),
        response.approvals_reviewer.to_core(),
        response.sandbox.to_core(),
        response.cwd.clone(),
        response.reasoning_effort,
        history_log_id,
        history_entry_count,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "session mapping keeps explicit fields"
)]
fn session_configured_from_thread_response(
    thread_id: &str,
    thread_name: Option<String>,
    rollout_path: Option<PathBuf>,
    model: String,
    model_provider_id: String,
    service_tier: Option<codex_protocol::config_types::ServiceTier>,
    approval_policy: AskForApproval,
    approvals_reviewer: codex_core::config::types::ApprovalsReviewer,
    sandbox_policy: SandboxPolicy,
    cwd: PathBuf,
    reasoning_effort: Option<codex_protocol::openai_models::ReasoningEffort>,
    history_log_id: u64,
    history_entry_count: usize,
) -> Result<SessionConfiguredEvent, String> {
    let session_id = ThreadId::from_string(thread_id)
        .map_err(|err| format!("thread id `{thread_id}` is invalid: {err}"))?;

    Ok(SessionConfiguredEvent {
        session_id,
        forked_from_id: None,
        thread_name,
        model,
        model_provider_id,
        service_tier,
        approval_policy,
        approvals_reviewer,
        sandbox_policy,
        cwd,
        reasoning_effort,
        history_log_id,
        history_entry_count,
        initial_messages: None,
        network_proxy: None,
        rollout_path,
    })
}

async fn history_metadata(config: &Config) -> (u64, usize) {
    let path = history_filepath(config);
    history_metadata_for_file(&path).await
}

fn history_filepath(config: &Config) -> PathBuf {
    config.codex_home.join("history.jsonl")
}

async fn history_metadata_for_file(path: &Path) -> (u64, usize) {
    let log_id = match fs::metadata(path).await {
        Ok(metadata) => history_log_id(&metadata).unwrap_or(0),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return (0, 0),
        Err(_) => return (0, 0),
    };

    let mut file = match fs::File::open(path).await {
        Ok(file) => file,
        Err(_) => return (log_id, 0),
    };

    let mut buf = [0u8; 8192];
    let mut count = 0usize;
    loop {
        match file.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                count += buf[..n].iter().filter(|&&b| b == b'\n').count();
            }
            Err(_) => return (log_id, 0),
        }
    }

    (log_id, count)
}

#[cfg(unix)]
fn history_log_id(metadata: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.ino())
}

#[cfg(windows)]
fn history_log_id(metadata: &std::fs::Metadata) -> Option<u64> {
    use std::os::windows::fs::MetadataExt;
    Some(metadata.creation_time())
}

#[cfg(not(any(unix, windows)))]
fn history_log_id(_metadata: &std::fs::Metadata) -> Option<u64> {
    None
}

fn review_target_to_api(target: ReviewTarget) -> ApiReviewTarget {
    match target {
        ReviewTarget::UncommittedChanges => ApiReviewTarget::UncommittedChanges,
        ReviewTarget::BaseBranch { branch } => ApiReviewTarget::BaseBranch { branch },
        ReviewTarget::Commit { sha, title } => ApiReviewTarget::Commit { sha, title },
        ReviewTarget::Custom { instructions } => ApiReviewTarget::Custom { instructions },
    }
}

fn review_decision_to_file_change_decision(
    decision: codex_protocol::protocol::ReviewDecision,
) -> FileChangeApprovalDecision {
    match decision {
        codex_protocol::protocol::ReviewDecision::Approved => FileChangeApprovalDecision::Accept,
        codex_protocol::protocol::ReviewDecision::ApprovedForSession => {
            FileChangeApprovalDecision::AcceptForSession
        }
        codex_protocol::protocol::ReviewDecision::Abort => FileChangeApprovalDecision::Cancel,
        codex_protocol::protocol::ReviewDecision::Denied => FileChangeApprovalDecision::Decline,
        codex_protocol::protocol::ReviewDecision::ApprovedExecpolicyAmendment { .. }
        | codex_protocol::protocol::ReviewDecision::NetworkPolicyAmendment { .. } => {
            FileChangeApprovalDecision::Accept
        }
    }
}

fn mcp_request_id_to_app_server_request_id(
    request_id: &codex_protocol::mcp::RequestId,
) -> RequestId {
    match request_id {
        codex_protocol::mcp::RequestId::String(value) => RequestId::String(value.clone()),
        codex_protocol::mcp::RequestId::Integer(value) => RequestId::Integer(*value),
    }
}

fn permission_grant_scope_to_api(
    scope: PermissionGrantScope,
) -> codex_app_server_protocol::PermissionGrantScope {
    match scope {
        PermissionGrantScope::Turn => codex_app_server_protocol::PermissionGrantScope::Turn,
        PermissionGrantScope::Session => codex_app_server_protocol::PermissionGrantScope::Session,
    }
}

fn granted_permission_profile_from_request(
    permissions: codex_protocol::request_permissions::RequestPermissionProfile,
) -> codex_app_server_protocol::GrantedPermissionProfile {
    codex_app_server_protocol::GrantedPermissionProfile {
        network: permissions.network.map(Into::into),
        file_system: permissions.file_system.map(Into::into),
        macos: None,
    }
}

fn elicitation_action_to_api(action: ElicitationAction) -> McpServerElicitationAction {
    match action {
        ElicitationAction::Accept => McpServerElicitationAction::Accept,
        ElicitationAction::Decline => McpServerElicitationAction::Decline,
        ElicitationAction::Cancel => McpServerElicitationAction::Cancel,
    }
}

fn lagged_event_warning_message(skipped: usize) -> String {
    format!("in-process app-server event stream lagged; dropped {skipped} events")
}

fn model_preset_from_api(model: codex_app_server_protocol::Model) -> ModelPreset {
    ModelPreset {
        id: model.id,
        model: model.model.clone(),
        display_name: model.display_name,
        description: model.description,
        default_reasoning_effort: model.default_reasoning_effort,
        supported_reasoning_efforts: model
            .supported_reasoning_efforts
            .into_iter()
            .map(|option| ReasoningEffortPreset {
                effort: option.reasoning_effort,
                description: option.description,
            })
            .collect(),
        supports_personality: model.supports_personality,
        is_default: model.is_default,
        upgrade: model.upgrade_info.map(|upgrade| ModelUpgrade {
            id: upgrade.model,
            reasoning_effort_mapping: None,
            migration_config_key: model.model.clone(),
            model_link: upgrade.model_link,
            upgrade_copy: upgrade.upgrade_copy,
            migration_markdown: upgrade.migration_markdown,
        }),
        show_in_picker: !model.hidden,
        availability_nux: model.availability_nux.map(|nux| ModelAvailabilityNux {
            message: nux.message,
        }),
        supported_in_api: true,
        input_modalities: model.input_modalities,
    }
}

fn collaboration_mode_mask_from_api(
    mask: codex_app_server_protocol::CollaborationModeMask,
) -> CollaborationModeMask {
    CollaborationModeMask {
        name: mask.name,
        mode: mask.mode,
        model: mask.model,
        reasoning_effort: mask.reasoning_effort,
        developer_instructions: None,
    }
}

pub(super) fn rate_limit_snapshot_from_api(
    snapshot: codex_app_server_protocol::RateLimitSnapshot,
) -> codex_protocol::protocol::RateLimitSnapshot {
    codex_protocol::protocol::RateLimitSnapshot {
        limit_id: snapshot.limit_id,
        limit_name: snapshot.limit_name,
        primary: snapshot.primary.map(rate_limit_window_from_api),
        secondary: snapshot.secondary.map(rate_limit_window_from_api),
        credits: snapshot.credits.map(credits_snapshot_from_api),
        plan_type: snapshot.plan_type,
    }
}

fn rate_limit_window_from_api(
    window: codex_app_server_protocol::RateLimitWindow,
) -> codex_protocol::protocol::RateLimitWindow {
    codex_protocol::protocol::RateLimitWindow {
        used_percent: f64::from(window.used_percent),
        window_minutes: window.window_duration_mins,
        resets_at: window.resets_at,
    }
}

fn credits_snapshot_from_api(
    credits: codex_app_server_protocol::CreditsSnapshot,
) -> codex_protocol::protocol::CreditsSnapshot {
    codex_protocol::protocol::CreditsSnapshot {
        has_credits: credits.has_credits,
        unlimited: credits.unlimited,
        balance: credits.balance,
    }
}

async fn resolve_server_request<T>(
    client: &InProcessAppServerClient,
    request_id: RequestId,
    value: T,
    method: &str,
) -> Result<(), String>
where
    T: serde::Serialize,
{
    let value = serde_json::to_value(value)
        .map_err(|err| format!("failed to encode `{method}` server request response: {err}"))?;
    client
        .resolve_server_request(request_id, value)
        .await
        .map_err(|err| format!("failed to resolve `{method}` server request: {err}"))
}

fn legacy_op_name(op: &Op) -> &'static str {
    match op {
        Op::Interrupt => "interrupt",
        Op::CleanBackgroundTerminals => "clean_background_terminals",
        Op::RealtimeConversationStart(_) => "realtime_conversation_start",
        Op::RealtimeConversationAudio(_) => "realtime_conversation_audio",
        Op::RealtimeConversationText(_) => "realtime_conversation_text",
        Op::RealtimeConversationClose => "realtime_conversation_close",
        Op::UserInput { .. } => "user_input",
        Op::UserTurn { .. } => "user_turn",
        Op::OverrideTurnContext { .. } => "override_turn_context",
        Op::ExecApproval { .. } => "exec_approval",
        Op::PatchApproval { .. } => "patch_approval",
        Op::ResolveElicitation { .. } => "resolve_elicitation",
        Op::UserInputAnswer { .. } => "user_input_answer",
        Op::RequestPermissionsResponse { .. } => "request_permissions_response",
        Op::DynamicToolResponse { .. } => "dynamic_tool_response",
        Op::AddToHistory { .. } => "add_to_history",
        Op::GetHistoryEntryRequest { .. } => "get_history_entry_request",
        Op::ListMcpTools => "list_mcp_tools",
        Op::RefreshMcpServers { .. } => "refresh_mcp_servers",
        Op::ReloadUserConfig => "reload_user_config",
        Op::ListCustomPrompts => "list_custom_prompts",
        Op::ListSkills { .. } => "list_skills",
        Op::ListRemoteSkills { .. } => "list_remote_skills",
        Op::DownloadRemoteSkill { .. } => "download_remote_skill",
        Op::Compact => "compact",
        Op::DropMemories => "drop_memories",
        Op::UpdateMemories => "update_memories",
        Op::SetThreadName { .. } => "set_thread_name",
        Op::Undo => "undo",
        Op::ThreadRollback { .. } => "thread_rollback",
        Op::Review { .. } => "review",
        Op::Shutdown => "shutdown",
        Op::RunUserShellCommand { .. } => "run_user_shell_command",
        Op::ListModels => "list_models",
        _ => "unknown",
    }
}
