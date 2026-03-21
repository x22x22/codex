use std::collections::HashMap;
use std::path::PathBuf;

use crate::approvals::ExecPolicyAmendment;
use crate::models::PermissionProfile;
use crate::models::SandboxPermissions;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

/// Approval plan chosen by the orchestrator for a unified-exec command.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum UnifiedExecApprovalRequirement {
    Skip {
        bypass_sandbox: bool,
        proposed_exec_policy_amendment: Option<ExecPolicyAmendment>,
    },
    NeedsApproval {
        reason: Option<String>,
        proposed_exec_policy_amendment: Option<ExecPolicyAmendment>,
    },
    Forbidden {
        reason: String,
    },
}

/// Fully resolved unified-exec startup request suitable for an executor wire.
///
/// The executor, not the orchestrator, mints the long-lived session handle for
/// any interactive process that survives the initial poll.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedExecExecCommandRequest {
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub tty: bool,
    pub yield_time_ms: u64,
    pub max_output_tokens: usize,
    pub sandbox_permissions: SandboxPermissions,
    pub additional_permissions: Option<PermissionProfile>,
    pub additional_permissions_preapproved: bool,
    pub justification: Option<String>,
    pub exec_approval_requirement: UnifiedExecApprovalRequirement,
}

/// Fully resolved unified-exec follow-up request suitable for an executor wire.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedExecWriteStdinRequest {
    pub process_id: i32,
    pub input: String,
    pub yield_time_ms: u64,
    pub max_output_tokens: usize,
}

#[cfg(test)]
mod tests {
    use super::UnifiedExecApprovalRequirement;
    use super::UnifiedExecExecCommandRequest;
    use super::UnifiedExecWriteStdinRequest;
    use crate::models::SandboxPermissions;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn unified_exec_exec_command_request_uses_camel_case() {
        let request = UnifiedExecExecCommandRequest {
            command: vec!["bash".to_string(), "-lc".to_string(), "pwd".to_string()],
            cwd: PathBuf::from("/tmp/example"),
            env: HashMap::from([("TERM".to_string(), "dumb".to_string())]),
            tty: true,
            yield_time_ms: 2_500,
            max_output_tokens: 4_000,
            sandbox_permissions: SandboxPermissions::UseDefault,
            additional_permissions: None,
            additional_permissions_preapproved: false,
            justification: Some("Need to inspect the repo".to_string()),
            exec_approval_requirement: UnifiedExecApprovalRequirement::NeedsApproval {
                reason: Some("sandbox escalation requested".to_string()),
                proposed_exec_policy_amendment: None,
            },
        };

        let value = serde_json::to_value(&request).expect("serialize request");
        assert_eq!(
            value,
            json!({
                "command": ["bash", "-lc", "pwd"],
                "cwd": "/tmp/example",
                "env": {
                    "TERM": "dumb",
                },
                "tty": true,
                "yieldTimeMs": 2_500,
                "maxOutputTokens": 4_000,
                "sandboxPermissions": "use_default",
                "additionalPermissions": null,
                "additionalPermissionsPreapproved": false,
                "justification": "Need to inspect the repo",
                "execApprovalRequirement": {
                    "needsApproval": {
                        "reason": "sandbox escalation requested",
                        "proposedExecPolicyAmendment": null,
                    }
                }
            })
        );
    }

    #[test]
    fn unified_exec_write_stdin_request_round_trips() {
        let value = json!({
            "processId": 7,
            "input": "echo hello\n",
            "yieldTimeMs": 5_000,
            "maxOutputTokens": 1_500,
        });

        let request = serde_json::from_value::<UnifiedExecWriteStdinRequest>(value.clone())
            .expect("deserialize request");
        assert_eq!(
            request,
            UnifiedExecWriteStdinRequest {
                process_id: 7,
                input: "echo hello\n".to_string(),
                yield_time_ms: 5_000,
                max_output_tokens: 1_500,
            }
        );

        let encoded = serde_json::to_value(&request).expect("serialize request");
        assert_eq!(encoded, value);
    }
}
