use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use crate::config::types::SecurityConfig;
use codex_protocol::ThreadId;
use codex_protocol::protocol::ApplyPatchApprovalRequestEvent;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ExecApprovalRequestEvent;
use codex_protocol::protocol::ExecCommandEndEvent;
use codex_protocol::protocol::ExecCommandStatus;
use codex_protocol::protocol::SecurityEvent;
use codex_protocol::protocol::SecurityEventKind;

#[cfg(test)]
use super::audit_logger::AuditLog;
use super::audit_logger::AuditLogger;
use super::redactor::SecurityRedactor;
#[cfg(test)]
use super::stats::SecurityStats;

pub(crate) struct SecurityMonitor {
    emit_core_events: bool,
    in_capture: AtomicBool,
    logger: AuditLogger,
    redactor: SecurityRedactor,
}

impl SecurityMonitor {
    pub(crate) fn new(thread_id: ThreadId, config: SecurityConfig) -> Self {
        let file_path = if config.auditlog.enabled {
            let base_dir = config
                .auditlog
                .dir
                .clone()
                .unwrap_or_else(|| PathBuf::from("auditlog"));
            Some(base_dir.join(format!("{thread_id}.jsonl")))
        } else {
            None
        };

        Self {
            emit_core_events: config.emit_core_events,
            in_capture: AtomicBool::new(false),
            logger: AuditLogger::new(config.session_buffer_limit, file_path),
            redactor: SecurityRedactor,
        }
    }

    pub(crate) fn capture(&self, event: &EventMsg) -> Option<SecurityEvent> {
        if self.in_capture.swap(true, Ordering::SeqCst) {
            return None;
        }

        let security_event = self.to_security_event(event);
        if let Some(security_event) = security_event.as_ref() {
            self.logger.record(security_event.clone());
        }

        self.in_capture.store(false, Ordering::SeqCst);
        if self.emit_core_events {
            security_event
        } else {
            None
        }
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Vec<AuditLog> {
        self.logger.snapshot()
    }

    #[cfg(test)]
    pub(crate) fn stats(&self) -> SecurityStats {
        self.logger.stats()
    }

    fn to_security_event(&self, event: &EventMsg) -> Option<SecurityEvent> {
        match event {
            EventMsg::ExecCommandBegin(event) => Some(SecurityEvent {
                kind: SecurityEventKind::Command,
                action: "exec_command_begin".to_owned(),
                turn_id: event.turn_id.clone(),
                call_id: Some(event.call_id.clone()),
                allowed: None,
                target: self.redactor.sanitize_command(&event.command),
                details: Some(format!("cwd={}", self.redactor.sanitize_path(&event.cwd))),
                duration_ms: None,
            }),
            EventMsg::ExecCommandEnd(event) => Some(self.exec_command_end_event(event)),
            EventMsg::ExecApprovalRequest(event) => Some(self.exec_approval_request_event(event)),
            EventMsg::ApplyPatchApprovalRequest(event) => {
                Some(self.apply_patch_approval_request_event(event))
            }
            EventMsg::Security(_) => None,
            _ => None,
        }
    }

    fn exec_command_end_event(&self, event: &ExecCommandEndEvent) -> SecurityEvent {
        let status = match event.status {
            ExecCommandStatus::Completed => "completed",
            ExecCommandStatus::Failed => "failed",
            ExecCommandStatus::Declined => "declined",
        };
        let duration_ms = u64::try_from(event.duration.as_millis()).unwrap_or(u64::MAX);
        SecurityEvent {
            kind: SecurityEventKind::Command,
            action: "exec_command_end".to_owned(),
            turn_id: event.turn_id.clone(),
            call_id: Some(event.call_id.clone()),
            allowed: None,
            target: self.redactor.sanitize_command(&event.command),
            details: Some(
                self.redactor
                    .sanitize_text(&format!("status={status} exit_code={}", event.exit_code)),
            ),
            duration_ms: Some(duration_ms),
        }
    }

    fn exec_approval_request_event(&self, event: &ExecApprovalRequestEvent) -> SecurityEvent {
        let mut details = event
            .reason
            .as_deref()
            .map(|reason| self.redactor.sanitize_text(reason));
        if details.is_none()
            && let Some(context) = event.network_approval_context.as_ref()
        {
            details = Some(self.redactor.sanitize_text(&context.host));
        }

        SecurityEvent {
            kind: SecurityEventKind::Permission,
            action: "exec_approval_request".to_owned(),
            turn_id: event.turn_id.clone(),
            call_id: Some(event.effective_approval_id()),
            allowed: None,
            target: self.redactor.sanitize_command(&event.command),
            details,
            duration_ms: None,
        }
    }

    fn apply_patch_approval_request_event(
        &self,
        event: &ApplyPatchApprovalRequestEvent,
    ) -> SecurityEvent {
        let mut details = event
            .reason
            .as_deref()
            .map(|reason| self.redactor.sanitize_text(reason));
        if details.is_none()
            && let Some(grant_root) = event.grant_root.as_deref()
        {
            details = Some(format!(
                "grant_root={}",
                self.redactor.sanitize_path(grant_root)
            ));
        }

        SecurityEvent {
            kind: SecurityEventKind::Permission,
            action: "apply_patch_approval_request".to_owned(),
            turn_id: event.turn_id.clone(),
            call_id: Some(event.call_id.clone()),
            allowed: None,
            target: Some(format!("{} file(s)", event.changes.len())),
            details,
            duration_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;

    use super::SecurityMonitor;
    use crate::config::types::SecurityAuditLogConfig;
    use crate::config::types::SecurityConfig;
    use codex_protocol::ThreadId;
    use codex_protocol::protocol::ApplyPatchApprovalRequestEvent;
    use codex_protocol::protocol::EventMsg;
    use codex_protocol::protocol::ExecCommandEndEvent;
    use codex_protocol::protocol::ExecCommandSource;
    use codex_protocol::protocol::ExecCommandStatus;
    use codex_protocol::protocol::SecurityEvent;
    use codex_protocol::protocol::SecurityEventKind;
    use pretty_assertions::assert_eq;

    #[test]
    fn capture_records_and_emits_core_event() {
        let monitor = SecurityMonitor::new(
            ThreadId::new(),
            SecurityConfig {
                enabled: true,
                emit_core_events: true,
                session_buffer_limit: 4,
                auditlog: SecurityAuditLogConfig::default(),
            },
        );

        let event = EventMsg::ExecCommandEnd(ExecCommandEndEvent {
            call_id: "call-1".to_owned(),
            process_id: None,
            turn_id: "turn-1".to_owned(),
            command: vec![
                "echo".to_owned(),
                "sk-abcdefghijklmnopqrstuvwxyz123456".to_owned(),
            ],
            cwd: PathBuf::from("/tmp/workspace"),
            parsed_cmd: Vec::new(),
            source: ExecCommandSource::Agent,
            interaction_input: None,
            stdout: String::new(),
            stderr: String::new(),
            aggregated_output: String::new(),
            exit_code: 0,
            duration: Duration::from_millis(25),
            formatted_output: String::new(),
            status: ExecCommandStatus::Completed,
        });

        let security_event = monitor.capture(&event).expect("security event");
        assert_eq!(SecurityEventKind::Command, security_event.kind);
        assert_eq!("exec_command_end", security_event.action);
        assert_eq!(Some(25), security_event.duration_ms);
        assert_eq!(
            Some("echo [REDACTED_SECRET]".to_owned()),
            security_event.target
        );

        let snapshot = monitor.snapshot();
        assert_eq!(1, snapshot.len());
        assert_eq!(security_event, snapshot[0].event);
        assert_eq!(1, monitor.stats().total);
    }

    #[test]
    fn security_events_are_not_re_emitted() {
        let monitor = SecurityMonitor::new(ThreadId::new(), SecurityConfig::default());
        let event = EventMsg::Security(SecurityEvent {
            kind: SecurityEventKind::Command,
            action: "existing".to_owned(),
            turn_id: "turn-1".to_owned(),
            call_id: None,
            allowed: None,
            target: None,
            details: None,
            duration_ms: None,
        });

        assert_eq!(None, monitor.capture(&event));
        assert_eq!(0, monitor.snapshot().len());
    }

    #[test]
    fn apply_patch_details_fall_back_to_grant_root() {
        let monitor = SecurityMonitor::new(
            ThreadId::new(),
            SecurityConfig {
                enabled: true,
                emit_core_events: true,
                session_buffer_limit: 4,
                auditlog: SecurityAuditLogConfig::default(),
            },
        );

        let event = EventMsg::ApplyPatchApprovalRequest(ApplyPatchApprovalRequestEvent {
            call_id: "call-1".to_owned(),
            turn_id: "turn-1".to_owned(),
            changes: HashMap::new(),
            reason: None,
            grant_root: Some(PathBuf::from("/tmp/workspace")),
        });

        let security_event = monitor.capture(&event).expect("security event");
        assert_eq!(SecurityEventKind::Permission, security_event.kind);
        assert_eq!(
            Some("grant_root=/tmp/workspace".to_owned()),
            security_event.details
        );
    }
}
