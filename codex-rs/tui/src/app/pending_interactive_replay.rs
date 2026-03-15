use crate::thread_update::ThreadUpdate;
use codex_protocol::protocol::Op;
use std::collections::HashMap;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ElicitationRequestId {
    String(String),
    Integer(i64),
}

impl From<codex_app_server_protocol::RequestId> for ElicitationRequestId {
    fn from(value: codex_app_server_protocol::RequestId) -> Self {
        match value {
            codex_app_server_protocol::RequestId::String(value) => Self::String(value),
            codex_app_server_protocol::RequestId::Integer(value) => Self::Integer(value),
        }
    }
}

impl From<codex_protocol::mcp::RequestId> for ElicitationRequestId {
    fn from(value: codex_protocol::mcp::RequestId) -> Self {
        match value {
            codex_protocol::mcp::RequestId::String(value) => Self::String(value),
            codex_protocol::mcp::RequestId::Integer(value) => Self::Integer(value),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ElicitationRequestKey {
    server_name: String,
    request_id: ElicitationRequestId,
}

impl ElicitationRequestKey {
    fn new(server_name: String, request_id: ElicitationRequestId) -> Self {
        Self {
            server_name,
            request_id,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct PendingInteractiveReplayState {
    exec_approval_ids: HashSet<String>,
    exec_approval_ids_by_turn_id: HashMap<String, Vec<String>>,
    patch_approval_ids: HashSet<String>,
    patch_approval_ids_by_turn_id: HashMap<String, Vec<String>>,
    elicitation_requests: HashSet<ElicitationRequestKey>,
    request_permissions_ids: HashSet<String>,
    request_permissions_ids_by_turn_id: HashMap<String, Vec<String>>,
    request_user_input_ids: HashSet<String>,
    request_user_input_ids_by_turn_id: HashMap<String, Vec<String>>,
}

impl PendingInteractiveReplayState {
    pub(super) fn update_can_change_pending_thread_approvals(update: &ThreadUpdate) -> bool {
        matches!(
            update,
            ThreadUpdate::CommandExecutionRequestApproval { .. }
                | ThreadUpdate::FileChangeRequestApproval { .. }
                | ThreadUpdate::McpServerElicitationRequest { .. }
                | ThreadUpdate::PermissionsRequestApproval { .. }
                | ThreadUpdate::ToolRequestUserInput { .. }
                | ThreadUpdate::TurnCompleted(_)
                | ThreadUpdate::ThreadClosed(_)
                | ThreadUpdate::ThreadRolledBack { .. }
        )
    }

    pub(super) fn op_can_change_state(op: &Op) -> bool {
        matches!(
            op,
            Op::ExecApproval { .. }
                | Op::PatchApproval { .. }
                | Op::ResolveElicitation { .. }
                | Op::RequestPermissionsResponse { .. }
                | Op::UserInputAnswer { .. }
                | Op::Shutdown
        )
    }

    pub(super) fn note_outbound_op(&mut self, op: &Op) {
        match op {
            Op::ExecApproval { id, turn_id, .. } => {
                self.exec_approval_ids.remove(id);
                if let Some(turn_id) = turn_id {
                    Self::remove_call_id_from_turn_map_entry(
                        &mut self.exec_approval_ids_by_turn_id,
                        turn_id,
                        id,
                    );
                }
            }
            Op::PatchApproval { id, .. } => {
                self.patch_approval_ids.remove(id);
                Self::remove_call_id_from_turn_map(&mut self.patch_approval_ids_by_turn_id, id);
            }
            Op::ResolveElicitation {
                server_name,
                request_id,
                ..
            } => {
                self.elicitation_requests
                    .remove(&ElicitationRequestKey::new(
                        server_name.clone(),
                        request_id.clone().into(),
                    ));
            }
            Op::RequestPermissionsResponse { id, .. } => {
                self.request_permissions_ids.remove(id);
                Self::remove_call_id_from_turn_map(
                    &mut self.request_permissions_ids_by_turn_id,
                    id,
                );
            }
            Op::UserInputAnswer { id, .. } => {
                let mut remove_turn_entry = false;
                if let Some(item_ids) = self.request_user_input_ids_by_turn_id.get_mut(id) {
                    if !item_ids.is_empty() {
                        let item_id = item_ids.remove(0);
                        self.request_user_input_ids.remove(&item_id);
                    }
                    if item_ids.is_empty() {
                        remove_turn_entry = true;
                    }
                }
                if remove_turn_entry {
                    self.request_user_input_ids_by_turn_id.remove(id);
                }
            }
            Op::Shutdown => self.clear(),
            _ => {}
        }
    }

    pub(super) fn note_update(&mut self, update: &ThreadUpdate) {
        match update {
            ThreadUpdate::CommandExecutionRequestApproval { params, .. } => {
                let approval_id = params
                    .approval_id
                    .clone()
                    .unwrap_or_else(|| params.item_id.clone());
                self.exec_approval_ids.insert(approval_id.clone());
                self.exec_approval_ids_by_turn_id
                    .entry(params.turn_id.clone())
                    .or_default()
                    .push(approval_id);
            }
            ThreadUpdate::FileChangeRequestApproval { params, .. } => {
                self.patch_approval_ids.insert(params.item_id.clone());
                self.patch_approval_ids_by_turn_id
                    .entry(params.turn_id.clone())
                    .or_default()
                    .push(params.item_id.clone());
            }
            ThreadUpdate::McpServerElicitationRequest { request_id, params } => {
                self.elicitation_requests.insert(ElicitationRequestKey::new(
                    params.server_name.clone(),
                    request_id.clone().into(),
                ));
            }
            ThreadUpdate::ToolRequestUserInput { params, .. } => {
                self.request_user_input_ids.insert(params.item_id.clone());
                self.request_user_input_ids_by_turn_id
                    .entry(params.turn_id.clone())
                    .or_default()
                    .push(params.item_id.clone());
            }
            ThreadUpdate::PermissionsRequestApproval { params, .. } => {
                self.request_permissions_ids.insert(params.item_id.clone());
                self.request_permissions_ids_by_turn_id
                    .entry(params.turn_id.clone())
                    .or_default()
                    .push(params.item_id.clone());
            }
            ThreadUpdate::TurnCompleted(notification) => {
                self.clear_exec_approval_turn(&notification.turn.id);
                self.clear_patch_approval_turn(&notification.turn.id);
                self.clear_request_permissions_turn(&notification.turn.id);
                self.clear_request_user_input_turn(&notification.turn.id);
            }
            ThreadUpdate::ThreadClosed(_) | ThreadUpdate::ThreadRolledBack { .. } => self.clear(),
            _ => {}
        }
    }

    pub(super) fn note_evicted_update(&mut self, update: &ThreadUpdate) {
        match update {
            ThreadUpdate::CommandExecutionRequestApproval { params, .. } => {
                let approval_id = params
                    .approval_id
                    .clone()
                    .unwrap_or_else(|| params.item_id.clone());
                self.exec_approval_ids.remove(&approval_id);
                Self::remove_call_id_from_turn_map_entry(
                    &mut self.exec_approval_ids_by_turn_id,
                    &params.turn_id,
                    &approval_id,
                );
            }
            ThreadUpdate::FileChangeRequestApproval { params, .. } => {
                self.patch_approval_ids.remove(&params.item_id);
                Self::remove_call_id_from_turn_map_entry(
                    &mut self.patch_approval_ids_by_turn_id,
                    &params.turn_id,
                    &params.item_id,
                );
            }
            ThreadUpdate::McpServerElicitationRequest { request_id, params } => {
                self.elicitation_requests
                    .remove(&ElicitationRequestKey::new(
                        params.server_name.clone(),
                        request_id.clone().into(),
                    ));
            }
            ThreadUpdate::ToolRequestUserInput { params, .. } => {
                self.request_user_input_ids.remove(&params.item_id);
                Self::remove_call_id_from_turn_map_entry(
                    &mut self.request_user_input_ids_by_turn_id,
                    &params.turn_id,
                    &params.item_id,
                );
            }
            ThreadUpdate::PermissionsRequestApproval { params, .. } => {
                self.request_permissions_ids.remove(&params.item_id);
                Self::remove_call_id_from_turn_map_entry(
                    &mut self.request_permissions_ids_by_turn_id,
                    &params.turn_id,
                    &params.item_id,
                );
            }
            _ => {}
        }
    }

    pub(super) fn should_replay_snapshot_update(&self, update: &ThreadUpdate) -> bool {
        match update {
            ThreadUpdate::CommandExecutionRequestApproval { params, .. } => self
                .exec_approval_ids
                .contains(params.approval_id.as_ref().unwrap_or(&params.item_id)),
            ThreadUpdate::FileChangeRequestApproval { params, .. } => {
                self.patch_approval_ids.contains(&params.item_id)
            }
            ThreadUpdate::McpServerElicitationRequest { request_id, params } => self
                .elicitation_requests
                .contains(&ElicitationRequestKey::new(
                    params.server_name.clone(),
                    request_id.clone().into(),
                )),
            ThreadUpdate::ToolRequestUserInput { params, .. } => {
                self.request_user_input_ids.contains(&params.item_id)
            }
            ThreadUpdate::PermissionsRequestApproval { params, .. } => {
                self.request_permissions_ids.contains(&params.item_id)
            }
            _ => true,
        }
    }

    pub(super) fn has_pending_thread_approvals(&self) -> bool {
        !self.exec_approval_ids.is_empty()
            || !self.patch_approval_ids.is_empty()
            || !self.elicitation_requests.is_empty()
            || !self.request_permissions_ids.is_empty()
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    fn clear_request_user_input_turn(&mut self, turn_id: &str) {
        if let Some(item_ids) = self.request_user_input_ids_by_turn_id.remove(turn_id) {
            for item_id in item_ids {
                self.request_user_input_ids.remove(&item_id);
            }
        }
    }

    fn clear_request_permissions_turn(&mut self, turn_id: &str) {
        if let Some(item_ids) = self.request_permissions_ids_by_turn_id.remove(turn_id) {
            for item_id in item_ids {
                self.request_permissions_ids.remove(&item_id);
            }
        }
    }

    fn clear_exec_approval_turn(&mut self, turn_id: &str) {
        if let Some(item_ids) = self.exec_approval_ids_by_turn_id.remove(turn_id) {
            for item_id in item_ids {
                self.exec_approval_ids.remove(&item_id);
            }
        }
    }

    fn clear_patch_approval_turn(&mut self, turn_id: &str) {
        if let Some(item_ids) = self.patch_approval_ids_by_turn_id.remove(turn_id) {
            for item_id in item_ids {
                self.patch_approval_ids.remove(&item_id);
            }
        }
    }

    fn remove_call_id_from_turn_map(
        call_ids_by_turn_id: &mut HashMap<String, Vec<String>>,
        call_id: &str,
    ) {
        call_ids_by_turn_id.retain(|_, call_ids| {
            call_ids.retain(|queued_call_id| queued_call_id != call_id);
            !call_ids.is_empty()
        });
    }

    fn remove_call_id_from_turn_map_entry(
        call_ids_by_turn_id: &mut HashMap<String, Vec<String>>,
        turn_id: &str,
        call_id: &str,
    ) {
        let mut remove_turn_entry = false;
        if let Some(call_ids) = call_ids_by_turn_id.get_mut(turn_id) {
            call_ids.retain(|queued_call_id| queued_call_id != call_id);
            if call_ids.is_empty() {
                remove_turn_entry = true;
            }
        }
        if remove_turn_entry {
            call_ids_by_turn_id.remove(turn_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PendingInteractiveReplayState;
    use crate::thread_update::ThreadUpdate;
    use codex_app_server_protocol::McpServerElicitationRequest;
    use codex_app_server_protocol::McpServerElicitationRequestParams;
    use codex_app_server_protocol::RequestId;
    use pretty_assertions::assert_eq;

    #[test]
    fn elicitation_request_keys_preserve_request_id_type() {
        let mut state = PendingInteractiveReplayState::default();
        let params = McpServerElicitationRequestParams {
            thread_id: "thread-1".to_string(),
            turn_id: Some("turn-1".to_string()),
            server_name: "server".to_string(),
            request: McpServerElicitationRequest::Url {
                meta: None,
                message: "Open this link".to_string(),
                url: "https://example.com".to_string(),
                elicitation_id: "elicitation-1".to_string(),
            },
        };

        state.note_update(&ThreadUpdate::McpServerElicitationRequest {
            request_id: RequestId::Integer(1),
            params: params.clone(),
        });

        assert_eq!(
            state.should_replay_snapshot_update(&ThreadUpdate::McpServerElicitationRequest {
                request_id: RequestId::Integer(1),
                params: params.clone(),
            }),
            true
        );
        assert_eq!(
            state.should_replay_snapshot_update(&ThreadUpdate::McpServerElicitationRequest {
                request_id: RequestId::String("1".to_string()),
                params,
            }),
            false
        );
    }
}
