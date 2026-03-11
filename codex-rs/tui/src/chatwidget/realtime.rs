use super::*;
use codex_protocol::protocol::ConversationAudioTruncateParams;
use codex_protocol::protocol::ConversationStartParams;
use codex_protocol::protocol::ConversationTextParams;
use codex_protocol::protocol::RealtimeConversationClosedEvent;
use codex_protocol::protocol::RealtimeConversationRealtimeEvent;
use codex_protocol::protocol::RealtimeConversationStartedEvent;
use codex_protocol::protocol::RealtimeEvent;
use codex_protocol::protocol::RealtimeHandoffRequested;
use codex_protocol::protocol::RealtimeOutputAudioDelta;
use codex_protocol::protocol::RealtimeToolAction;
use codex_protocol::protocol::RealtimeToolActionRequested;
use codex_protocol::protocol::RealtimeToolCallCompleteParams;
use codex_protocol::user_input::ByteRange;
use serde_json::json;

const REALTIME_CONVERSATION_PROMPT: &str = concat!(
    "You are in a realtime voice conversation in the Codex TUI. ",
    "You can call the codex function to delegate work to Codex, this should be your default action. Don't verbalize anything before calling codex. ",
    "The codex function queues requests by default when Codex is already busy. Queuing is expected behavior and usually the right choice. ",
    "It is fine to queue follow-up work and let Codex pick it up on the next turn. ",
    "Set send_immediately=true only when you truly need to steer the current Codex turn right away. ",
    "If the user asks to stop, cancel, or abort ongoing Codex work, call cancel_current_operation before speaking. ",
    "If the user asks to stop voice mode, exit realtime mode, or turn off live voice, call turn_off_realtime_mode and do not speak afterward. ",
    "Codex can inspect the repository, read and edit files, run commands, and produce detailed ",
    "text output that the user can read in the TUI. ",
    "Use codex whenever the user asks for codebase-specific facts, debugging, file changes, ",
    "command output, or anything that benefits from tools. ",
    "Do not use codex for pure control actions when a dedicated control function fits. ",
    "Use manage_message_queue to inspect or edit queued draft work. Supported actions are list, replace_last, remove_last, and clear. ",
    "If the user asks what is queued, or asks to replace, remove, or clear queued draft work, use manage_message_queue instead of codex. ",
    "Use manage_runtime_settings to inspect or change runtime settings for future turns, including model, working_directory, reasoning_effort, fast_mode, personality, and collaboration_mode. ",
    "If the user asks to inspect or change those runtime settings, use manage_runtime_settings instead of codex. ",
    "If you call manage_runtime_settings without any setting fields, it returns the current settings and the list of possible settings and allowed values. ",
    "Use run_tui_command for built-in TUI actions. Supported commands are compact, review, plan, diff, and agent. ",
    "If the user asks to compact, review, switch to Plan mode, show the diff, or open the agent picker, use run_tui_command instead of codex. ",
    "When you speak to the user directly, be extremely concise. ",
    "Prefer short spoken answers because the user can see the detailed text output themselves. ",
    "Do not read long code snippets, long file paths, diffs, or command output aloud unless the ",
    "user explicitly asks for that."
);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum RealtimeConversationPhase {
    #[default]
    Inactive,
    Starting,
    Active,
    Stopping,
}

#[derive(Default)]
pub(super) struct RealtimeConversationUiState {
    phase: RealtimeConversationPhase,
    requested_close: bool,
    close_when_idle: bool,
    session_id: Option<String>,
    warned_unsupported_composer_submission: bool,
    meter_placeholder_id: Option<String>,
    #[cfg(not(target_os = "linux"))]
    capture_stop_flag: Option<Arc<AtomicBool>>,
    #[cfg(not(target_os = "linux"))]
    capture: Option<crate::voice::VoiceCapture>,
    #[cfg(not(target_os = "linux"))]
    audio_player: Option<crate::voice::RealtimeAudioPlayer>,
}

impl RealtimeConversationUiState {
    pub(super) fn is_live(&self) -> bool {
        matches!(
            self.phase,
            RealtimeConversationPhase::Starting
                | RealtimeConversationPhase::Active
                | RealtimeConversationPhase::Stopping
        )
    }

    pub(super) fn is_active(&self) -> bool {
        matches!(self.phase, RealtimeConversationPhase::Active)
    }
}

#[cfg(test)]
impl RealtimeConversationUiState {
    pub(super) fn set_phase_for_test(&mut self, phase: RealtimeConversationPhase) {
        self.phase = phase;
    }

    pub(super) fn set_meter_placeholder_id_for_test(&mut self, id: Option<String>) {
        self.meter_placeholder_id = id;
    }

    pub(super) fn phase_for_test(&self) -> RealtimeConversationPhase {
        self.phase
    }

    pub(super) fn requested_close_for_test(&self) -> bool {
        self.requested_close
    }

    pub(super) fn close_when_idle_for_test(&self) -> bool {
        self.close_when_idle
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct RenderedUserMessageEvent {
    pub(super) message: String,
    pub(super) remote_image_urls: Vec<String>,
    pub(super) local_images: Vec<PathBuf>,
    pub(super) text_elements: Vec<TextElement>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PendingSteerCompareKey {
    pub(super) message: String,
    pub(super) image_count: usize,
}

impl ChatWidget {
    pub(super) fn rendered_user_message_event_from_parts(
        message: String,
        text_elements: Vec<TextElement>,
        local_images: Vec<PathBuf>,
        remote_image_urls: Vec<String>,
    ) -> RenderedUserMessageEvent {
        RenderedUserMessageEvent {
            message,
            remote_image_urls,
            local_images,
            text_elements,
        }
    }

    pub(super) fn rendered_user_message_event_from_event(
        event: &UserMessageEvent,
    ) -> RenderedUserMessageEvent {
        Self::rendered_user_message_event_from_parts(
            event.message.clone(),
            event.text_elements.clone(),
            event.local_images.clone(),
            event.images.clone().unwrap_or_default(),
        )
    }

    /// Build the compare key for a submitted pending steer without invoking the
    /// expensive request-serialization path. Pending steers only need to match the
    /// committed `ItemCompleted(UserMessage)` emitted after core drains input, which
    /// preserves flattened text and total image count but not UI-only text ranges or
    /// local image paths.
    pub(super) fn pending_steer_compare_key_from_items(
        items: &[UserInput],
    ) -> PendingSteerCompareKey {
        let mut message = String::new();
        let mut image_count = 0;

        for item in items {
            match item {
                UserInput::Text { text, .. } => message.push_str(text),
                UserInput::Image { .. } | UserInput::LocalImage { .. } => image_count += 1,
                UserInput::Skill { .. } | UserInput::Mention { .. } => {}
                _ => {}
            }
        }

        PendingSteerCompareKey {
            message,
            image_count,
        }
    }

    pub(super) fn pending_steer_compare_key_from_item(
        item: &codex_protocol::items::UserMessageItem,
    ) -> PendingSteerCompareKey {
        Self::pending_steer_compare_key_from_items(&item.content)
    }

    #[cfg(test)]
    pub(super) fn rendered_user_message_event_from_inputs(
        items: &[UserInput],
    ) -> RenderedUserMessageEvent {
        let mut message = String::new();
        let mut remote_image_urls = Vec::new();
        let mut local_images = Vec::new();
        let mut text_elements = Vec::new();

        for item in items {
            match item {
                UserInput::Text {
                    text,
                    text_elements: current_text_elements,
                } => append_text_with_rebased_elements(
                    &mut message,
                    &mut text_elements,
                    text,
                    current_text_elements.iter().map(|element| {
                        TextElement::new(
                            element.byte_range,
                            element.placeholder(text).map(str::to_string),
                        )
                    }),
                ),
                UserInput::Image { image_url } => remote_image_urls.push(image_url.clone()),
                UserInput::LocalImage { path } => local_images.push(path.clone()),
                UserInput::Skill { .. } | UserInput::Mention { .. } => {}
                _ => {}
            }
        }

        Self::rendered_user_message_event_from_parts(
            message,
            text_elements,
            local_images,
            remote_image_urls,
        )
    }

    pub(super) fn should_render_realtime_user_message_event(
        &self,
        event: &UserMessageEvent,
    ) -> bool {
        if !self.realtime_conversation.is_live() {
            return false;
        }
        let key = Self::rendered_user_message_event_from_event(event);
        self.last_rendered_user_message_event.as_ref() != Some(&key)
    }

    pub(super) fn maybe_submit_user_message_for_realtime(
        &mut self,
        user_message: UserMessage,
    ) -> Option<UserMessage> {
        if !self.realtime_conversation.is_live() {
            return Some(user_message);
        }

        let UserMessage {
            mut text,
            local_images,
            remote_image_urls,
            mut text_elements,
            mention_bindings,
        } = user_message;

        if !local_images.is_empty() || !remote_image_urls.is_empty() {
            self.restore_user_message_to_composer(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            if !self
                .realtime_conversation
                .warned_unsupported_composer_submission
            {
                self.realtime_conversation
                    .warned_unsupported_composer_submission = true;
                self.add_info_message(
                    "Realtime voice mode accepts typed text, but not image attachments. Remove attachments or use /realtime to stop."
                        .to_string(),
                    None,
                );
            } else {
                self.request_redraw();
            }
            return None;
        }

        self.realtime_conversation
            .warned_unsupported_composer_submission = false;

        if self.realtime_conversation.meter_placeholder_id.is_some() {
            let mut stripped_bytes = 0usize;
            let mut stripped_chars = 0usize;
            for (idx, ch) in text.char_indices() {
                if matches!(ch, '⠤' | '⠴' | '⠶' | '⠷' | '⡷' | '⡿' | '⣿') && stripped_chars < 4
                {
                    stripped_chars += 1;
                    stripped_bytes = idx + ch.len_utf8();
                } else {
                    break;
                }
            }

            if stripped_chars == 4 {
                text = text[stripped_bytes..].to_string();
                text_elements = text_elements
                    .into_iter()
                    .filter_map(|element| {
                        if element.byte_range.end <= stripped_bytes {
                            None
                        } else {
                            Some(element.map_range(|range| ByteRange {
                                start: range.start.saturating_sub(stripped_bytes),
                                end: range.end.saturating_sub(stripped_bytes),
                            }))
                        }
                    })
                    .collect();
            }
        }

        if !self.submit_op(Op::RealtimeConversationText(ConversationTextParams {
            text: text.clone(),
        })) {
            return None;
        }

        if !text.is_empty() {
            let encoded_mentions = mention_bindings
                .iter()
                .map(|binding| LinkedMention {
                    mention: binding.mention.clone(),
                    path: binding.path.clone(),
                })
                .collect::<Vec<_>>();
            let history_text = encode_history_mentions(&text, &encoded_mentions);
            self.codex_op_tx
                .send(Op::AddToHistory { text: history_text })
                .unwrap_or_else(|e| {
                    tracing::error!("failed to send AddHistory op: {e}");
                });
        }

        self.on_user_message_event(UserMessageEvent {
            message: text,
            images: None,
            local_images: Vec::new(),
            text_elements,
        });
        self.request_redraw();

        None
    }

    fn realtime_footer_hint_items() -> Vec<(String, String)> {
        vec![("/realtime".to_string(), "stop live voice".to_string())]
    }

    pub(super) fn start_realtime_conversation(&mut self) {
        self.realtime_conversation.phase = RealtimeConversationPhase::Starting;
        self.realtime_conversation.requested_close = false;
        self.realtime_conversation.close_when_idle = false;
        self.realtime_conversation.session_id = None;
        self.realtime_conversation
            .warned_unsupported_composer_submission = false;
        self.set_footer_hint_override(Some(Self::realtime_footer_hint_items()));
        self.submit_op(Op::RealtimeConversationStart(ConversationStartParams {
            prompt: REALTIME_CONVERSATION_PROMPT.to_string(),
            session_id: None,
        }));
        self.request_redraw();
    }

    pub(super) fn request_realtime_conversation_close(&mut self, info_message: Option<String>) {
        if !self.realtime_conversation.is_live() {
            if let Some(message) = info_message {
                self.add_info_message(message, None);
            }
            return;
        }

        self.realtime_conversation.requested_close = true;
        self.realtime_conversation.phase = RealtimeConversationPhase::Stopping;
        self.stop_realtime_local_audio();
        self.set_footer_hint_override(None);

        if self.bottom_pane.is_task_running() {
            self.realtime_conversation.close_when_idle = true;
            if !self.submit_op(Op::Interrupt) {
                self.realtime_conversation.close_when_idle = false;
                self.submit_op(Op::RealtimeConversationClose);
            }
        } else {
            self.realtime_conversation.close_when_idle = false;
            self.submit_op(Op::RealtimeConversationClose);
        }

        if let Some(message) = info_message {
            self.add_info_message(message, None);
        } else {
            self.request_redraw();
        }
    }

    pub(super) fn force_close_realtime_conversation(&mut self, info_message: Option<String>) {
        if !self.realtime_conversation.is_live() {
            if let Some(message) = info_message {
                self.add_info_message(message, None);
            }
            return;
        }

        self.realtime_conversation.requested_close = true;
        self.realtime_conversation.close_when_idle = false;
        self.stop_realtime_local_audio();
        self.set_footer_hint_override(None);
        self.submit_op(Op::RealtimeConversationClose);
        self.reset_realtime_conversation_state();

        if let Some(message) = info_message {
            self.add_info_message(message, None);
        } else {
            self.request_redraw();
        }
    }

    pub(super) fn maybe_close_realtime_conversation_when_idle(&mut self) -> bool {
        if !self.realtime_conversation.close_when_idle || self.bottom_pane.is_task_running() {
            return false;
        }

        self.realtime_conversation.close_when_idle = false;
        self.submit_op(Op::RealtimeConversationClose)
    }

    pub(super) fn reset_realtime_conversation_state(&mut self) {
        self.stop_realtime_local_audio();
        self.set_footer_hint_override(None);
        self.realtime_conversation.phase = RealtimeConversationPhase::Inactive;
        self.realtime_conversation.requested_close = false;
        self.realtime_conversation.close_when_idle = false;
        self.realtime_conversation.session_id = None;
        self.realtime_conversation
            .warned_unsupported_composer_submission = false;
    }

    pub(super) fn on_realtime_conversation_started(
        &mut self,
        ev: RealtimeConversationStartedEvent,
    ) {
        if !self.realtime_conversation_enabled() {
            self.submit_op(Op::RealtimeConversationClose);
            self.reset_realtime_conversation_state();
            return;
        }
        self.realtime_conversation.phase = RealtimeConversationPhase::Active;
        self.realtime_conversation.session_id = ev.session_id;
        self.realtime_conversation
            .warned_unsupported_composer_submission = false;
        self.set_footer_hint_override(Some(Self::realtime_footer_hint_items()));
        self.start_realtime_local_audio();
        self.request_redraw();
    }

    pub(super) fn on_realtime_conversation_realtime(
        &mut self,
        ev: RealtimeConversationRealtimeEvent,
    ) {
        match ev.payload {
            RealtimeEvent::SessionUpdated { session_id, .. } => {
                self.realtime_conversation.session_id = Some(session_id);
            }
            RealtimeEvent::InputAudioSpeechStarted(_) => {
                self.interrupt_realtime_audio_playback();
            }
            RealtimeEvent::InputTranscriptDelta(_) => {}
            RealtimeEvent::OutputTranscriptDelta(_) => {}
            RealtimeEvent::AudioOut(frame) => self.enqueue_realtime_audio_out(&frame),
            RealtimeEvent::ResponseCancelled(_) => {
                self.interrupt_realtime_audio_playback();
            }
            RealtimeEvent::ConversationItemAdded(_item) => {}
            RealtimeEvent::ConversationItemDone { .. } => {}
            RealtimeEvent::HandoffRequested(handoff) => {
                self.on_realtime_handoff_requested(handoff);
            }
            RealtimeEvent::InterruptRequested(_) => {}
            RealtimeEvent::CloseRequested(_) => {
                self.request_realtime_conversation_close(Some(
                    if self.bottom_pane.is_task_running() {
                        "Stopping realtime voice mode after current work is cancelled."
                    } else {
                        "Realtime voice mode turned off."
                    }
                    .to_string(),
                ));
            }
            RealtimeEvent::ToolActionRequested(request) => {
                self.on_realtime_tool_action_requested(request);
            }
            RealtimeEvent::Error(message) => {
                self.add_error_message(format!("Realtime voice error: {message}"));
                self.reset_realtime_conversation_state();
            }
        }
    }

    pub(super) fn on_realtime_conversation_closed(&mut self, ev: RealtimeConversationClosedEvent) {
        let requested = self.realtime_conversation.requested_close;
        let reason = ev.reason;
        self.reset_realtime_conversation_state();
        if !requested && let Some(reason) = reason {
            self.add_info_message(format!("Realtime voice mode closed: {reason}"), None);
        }
        self.request_redraw();
    }

    fn enqueue_realtime_audio_out(&mut self, frame: &RealtimeOutputAudioDelta) {
        #[cfg(not(target_os = "linux"))]
        {
            if self.realtime_conversation.audio_player.is_none() {
                self.realtime_conversation.audio_player =
                    crate::voice::RealtimeAudioPlayer::start(&self.config).ok();
            }
            if let Some(player) = &self.realtime_conversation.audio_player
                && let Err(err) = player.enqueue_frame(frame)
            {
                warn!("failed to play realtime audio: {err}");
            }
        }
        #[cfg(target_os = "linux")]
        {
            let _ = frame;
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn interrupt_realtime_audio_playback(&mut self) {
        let Some(player) = &self.realtime_conversation.audio_player else {
            return;
        };

        let Some(position) = player.interrupt() else {
            return;
        };

        self.submit_op(Op::RealtimeConversationAudioTruncate(
            ConversationAudioTruncateParams {
                item_id: position.item_id,
                content_index: 0,
                audio_end_ms: position.audio_end_ms,
            },
        ));
    }

    #[cfg(target_os = "linux")]
    fn interrupt_realtime_audio_playback(&mut self) {}

    fn on_realtime_handoff_requested(&mut self, handoff: RealtimeHandoffRequested) {
        let Some(text) = realtime_text_from_handoff_request(&handoff) else {
            return;
        };

        let user_message = UserMessage {
            text,
            local_images: Vec::new(),
            remote_image_urls: Vec::new(),
            text_elements: Vec::new(),
            mention_bindings: Vec::new(),
        };

        if handoff.send_immediately {
            self.submit_user_message(user_message);
        } else {
            self.queue_user_message(user_message);
        }
    }

    fn on_realtime_tool_action_requested(&mut self, request: RealtimeToolActionRequested) {
        let output_text = match request.action {
            RealtimeToolAction::ManageMessageQueue { action, message } => {
                self.manage_message_queue_via_realtime(action, message)
            }
            RealtimeToolAction::ListMessageQueue => self.realtime_message_queue_output(),
            RealtimeToolAction::ReplaceLastQueuedMessage { message } => {
                self.replace_last_queued_message_via_realtime(message)
            }
            RealtimeToolAction::RemoveLastQueuedMessage => {
                self.remove_last_queued_message_via_realtime()
            }
            RealtimeToolAction::ClearQueuedMessages => self.clear_queued_messages_via_realtime(),
            RealtimeToolAction::ManageRuntimeSettings {
                model,
                working_directory,
                reasoning_effort,
                fast_mode,
                personality,
                collaboration_mode,
            } => self.update_runtime_settings_via_realtime(
                model,
                working_directory,
                reasoning_effort,
                fast_mode,
                personality,
                collaboration_mode,
            ),
            RealtimeToolAction::ListRuntimeSettings => self.realtime_runtime_settings_output(),
            RealtimeToolAction::UpdateRuntimeSettings {
                model,
                working_directory,
                reasoning_effort,
                fast_mode,
                personality,
                collaboration_mode,
            } => self.update_runtime_settings_via_realtime(
                model,
                working_directory,
                reasoning_effort,
                fast_mode,
                personality,
                collaboration_mode,
            ),
            RealtimeToolAction::RunTuiCommand { command, prompt } => {
                self.run_tui_command_via_realtime(command, prompt)
            }
            RealtimeToolAction::CompactConversation => self.compact_conversation_via_realtime(),
        };
        self.complete_realtime_tool_call(request.call_id, output_text);
    }

    fn complete_realtime_tool_call(&self, call_id: String, output_text: String) {
        self.app_event_tx
            .send(AppEvent::CodexOp(Op::RealtimeConversationToolCallComplete(
                RealtimeToolCallCompleteParams {
                    call_id,
                    output_text,
                },
            )));
    }

    fn realtime_message_queue_output(&self) -> String {
        let queued_messages = self
            .queued_user_messages
            .iter()
            .enumerate()
            .map(|(index, message)| {
                json!({
                    "position": index + 1,
                    "text": message.text,
                    "local_image_count": message.local_images.len(),
                    "remote_image_count": message.remote_image_urls.len(),
                })
            })
            .collect::<Vec<_>>();
        let pending_steers = self
            .pending_steers
            .iter()
            .enumerate()
            .map(|(index, pending)| {
                json!({
                    "position": index + 1,
                    "text": pending.user_message.text,
                    "local_image_count": pending.user_message.local_images.len(),
                    "remote_image_count": pending.user_message.remote_image_urls.len(),
                })
            })
            .collect::<Vec<_>>();

        json!({
            "queued_messages": queued_messages,
            "pending_steers": pending_steers,
            "supported_actions": ["list", "replace_last", "remove_last", "clear"],
            "editable_scope": "Queue editing tools affect queued_messages only.",
        })
        .to_string()
    }

    fn manage_message_queue_via_realtime(
        &mut self,
        action: String,
        message: Option<String>,
    ) -> String {
        match action.trim().to_ascii_lowercase().as_str() {
            "list" => self.realtime_message_queue_output(),
            "replace_last" => self.replace_last_queued_message_via_realtime(
                message.unwrap_or_default(),
            ),
            "remove_last" => self.remove_last_queued_message_via_realtime(),
            "clear" => self.clear_queued_messages_via_realtime(),
            other => json!({
                "status": "error",
                "message": format!(
                    "Unsupported queue action `{other}`. Use list, replace_last, remove_last, or clear."
                ),
                "supported_actions": ["list", "replace_last", "remove_last", "clear"],
            })
            .to_string(),
        }
    }

    fn replace_last_queued_message_via_realtime(&mut self, message: String) -> String {
        let replacement = message.trim().to_string();
        if replacement.is_empty() {
            return json!({
                "status": "error",
                "message": "The replacement message must not be empty.",
            })
            .to_string();
        }

        let Some(last_message) = self.queued_user_messages.back_mut() else {
            return json!({
                "status": "error",
                "message": "There are no queued draft messages to replace.",
            })
            .to_string();
        };

        if !last_message.local_images.is_empty() || !last_message.remote_image_urls.is_empty() {
            return json!({
                "status": "error",
                "message": "The last queued draft includes attachments and cannot be replaced via realtime tools.",
            })
            .to_string();
        }

        let previous_message = std::mem::replace(&mut last_message.text, replacement.clone());
        last_message.text_elements.clear();
        last_message.mention_bindings.clear();
        self.refresh_pending_input_preview();
        self.request_redraw();

        json!({
            "status": "ok",
            "previous_message": previous_message,
            "message": replacement,
            "queued_message_count": self.queued_user_messages.len(),
        })
        .to_string()
    }

    fn remove_last_queued_message_via_realtime(&mut self) -> String {
        let Some(removed_message) = self.queued_user_messages.pop_back() else {
            return json!({
                "status": "error",
                "message": "There are no queued draft messages to remove.",
            })
            .to_string();
        };

        self.refresh_pending_input_preview();
        self.request_redraw();

        json!({
            "status": "ok",
            "removed_message": removed_message.text,
            "queued_message_count": self.queued_user_messages.len(),
        })
        .to_string()
    }

    fn clear_queued_messages_via_realtime(&mut self) -> String {
        let removed_count = self.queued_user_messages.len();
        let cleared_messages = self
            .queued_user_messages
            .drain(..)
            .map(|message| message.text)
            .collect::<Vec<_>>();
        self.refresh_pending_input_preview();
        self.request_redraw();

        json!({
            "status": "ok",
            "cleared_count": removed_count,
            "cleared_messages": cleared_messages,
        })
        .to_string()
    }

    fn realtime_runtime_settings_output(&self) -> String {
        json!({
            "status": "ok",
            "current_settings": self.current_runtime_settings_json(),
            "possible_settings": self.possible_runtime_settings_json(),
        })
        .to_string()
    }

    fn current_runtime_settings_json(&self) -> serde_json::Value {
        json!({
            "working_directory": self.config.cwd.display().to_string(),
            "model": self.current_model(),
            "reasoning_effort": Self::status_line_reasoning_effort_label(
                self.effective_reasoning_effort(),
            ),
            "fast_mode": matches!(self.current_service_tier(), Some(ServiceTier::Fast)),
            "personality": self.realtime_personality_name(
                self.config.personality.unwrap_or(Personality::None),
            ),
            "collaboration_mode": self.realtime_collaboration_mode_name(self.active_mode_kind()),
        })
    }

    fn possible_runtime_settings_json(&self) -> serde_json::Value {
        let available_models = self
            .models_manager
            .try_list_models()
            .map(|models| {
                models
                    .into_iter()
                    .map(|preset| preset.model)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|_| Vec::new());
        let plan_available = collaboration_modes::plan_mask(self.models_manager.as_ref()).is_some();

        json!({
            "keys": [
                "model",
                "working_directory",
                "reasoning_effort",
                "fast_mode",
                "personality",
                "collaboration_mode",
            ],
            "model_values": available_models,
            "working_directory": {
                "description": "Absolute path or path relative to the current working directory.",
            },
            "reasoning_effort_values": ["default", "none", "minimal", "low", "medium", "high", "xhigh"],
            "fast_mode_values": [false, true],
            "personality_values": ["none", "friendly", "pragmatic"],
            "collaboration_mode_values": if plan_available {
                json!(["default", "plan"])
            } else {
                json!(["default"])
            },
        })
    }

    fn update_runtime_settings_via_realtime(
        &mut self,
        model: Option<String>,
        working_directory: Option<String>,
        reasoning_effort: Option<String>,
        fast_mode: Option<bool>,
        personality: Option<String>,
        collaboration_mode: Option<String>,
    ) -> String {
        let requested_anything = model.is_some()
            || working_directory.is_some()
            || reasoning_effort.is_some()
            || fast_mode.is_some()
            || personality.is_some()
            || collaboration_mode.is_some();
        if !requested_anything {
            return self.realtime_runtime_settings_output();
        }

        let resolved_cwd = match working_directory {
            Some(working_directory) => {
                match self.resolve_realtime_working_directory(working_directory.as_str()) {
                    Ok(path) => Some(path),
                    Err(message) => {
                        return json!({
                            "status": "error",
                            "message": message,
                            "current_settings": self.current_runtime_settings_json(),
                            "possible_settings": self.possible_runtime_settings_json(),
                        })
                        .to_string();
                    }
                }
            }
            None => None,
        };

        let parsed_reasoning_effort = match reasoning_effort {
            Some(reasoning_effort) => {
                match self.parse_realtime_reasoning_effort(reasoning_effort.as_str()) {
                    Ok(effort) => Some(effort),
                    Err(message) => {
                        return json!({
                            "status": "error",
                            "message": message,
                            "current_settings": self.current_runtime_settings_json(),
                            "possible_settings": self.possible_runtime_settings_json(),
                        })
                        .to_string();
                    }
                }
            }
            None => None,
        };

        let parsed_personality = match personality {
            Some(personality) => match self.parse_realtime_personality(personality.as_str()) {
                Ok(personality) => Some(personality),
                Err(message) => {
                    return json!({
                        "status": "error",
                        "message": message,
                        "current_settings": self.current_runtime_settings_json(),
                        "possible_settings": self.possible_runtime_settings_json(),
                    })
                    .to_string();
                }
            },
            None => None,
        };

        let parsed_collaboration_mode = match collaboration_mode {
            Some(collaboration_mode) => {
                match self.parse_realtime_collaboration_mode(collaboration_mode.as_str()) {
                    Ok(mode) => Some(mode),
                    Err(message) => {
                        return json!({
                            "status": "error",
                            "message": message,
                            "current_settings": self.current_runtime_settings_json(),
                            "possible_settings": self.possible_runtime_settings_json(),
                        })
                        .to_string();
                    }
                }
            }
            None => None,
        };

        let needs_model_lookup =
            model.is_some() || parsed_reasoning_effort.is_some() || parsed_personality.is_some();
        let target_preset = if needs_model_lookup {
            let requested_model = model
                .as_deref()
                .unwrap_or_else(|| self.current_model())
                .to_string();
            match self.find_realtime_model_preset(requested_model.as_str()) {
                Ok(preset) => Some(preset),
                Err(message) => {
                    return json!({
                        "status": "error",
                        "message": message,
                        "current_settings": self.current_runtime_settings_json(),
                        "possible_settings": self.possible_runtime_settings_json(),
                    })
                    .to_string();
                }
            }
        } else {
            None
        };

        let target_model = target_preset
            .as_ref()
            .map(|preset| preset.model.clone())
            .unwrap_or_else(|| self.current_model().to_string());
        let target_effort = if let Some(effort) = parsed_reasoning_effort {
            effort
        } else if model.is_some() && target_model != self.current_model() {
            target_preset
                .as_ref()
                .map(|preset| preset.default_reasoning_effort)
        } else {
            self.effective_reasoning_effort()
        };

        if let Some(preset) = &target_preset
            && let Some(effort) = target_effort
            && !preset
                .supported_reasoning_efforts
                .iter()
                .any(|option| option.effort == effort)
        {
            return json!({
                "status": "error",
                "message": format!(
                    "Model {} does not support reasoning effort {}.",
                    preset.model,
                    Self::status_line_reasoning_effort_label(Some(effort)),
                ),
                "current_settings": self.current_runtime_settings_json(),
                "possible_settings": self.possible_runtime_settings_json(),
            })
            .to_string();
        }

        if let Some(personality) = parsed_personality
            && personality != Personality::None
            && !self.config.features.enabled(Feature::Personality)
        {
            return json!({
                "status": "error",
                "message": "The personality feature is disabled in this session.",
                "current_settings": self.current_runtime_settings_json(),
                "possible_settings": self.possible_runtime_settings_json(),
            })
            .to_string();
        }

        if let Some(personality) = parsed_personality
            && personality != Personality::None
            && !target_preset
                .as_ref()
                .is_some_and(|preset| preset.supports_personality)
        {
            return json!({
                "status": "error",
                "message": format!(
                    "Model {} does not support personalities.",
                    target_model
                ),
                "current_settings": self.current_runtime_settings_json(),
                "possible_settings": self.possible_runtime_settings_json(),
            })
            .to_string();
        }

        let target_collaboration_mask = match parsed_collaboration_mode {
            Some(ModeKind::Default) => {
                collaboration_modes::default_mask(self.models_manager.as_ref())
            }
            Some(ModeKind::Plan) => collaboration_modes::plan_mask(self.models_manager.as_ref()),
            Some(_) => None,
            None => None,
        };
        if parsed_collaboration_mode.is_some() && target_collaboration_mask.is_none() {
            return json!({
                "status": "error",
                "message": "The requested collaboration mode is not available right now.",
                "current_settings": self.current_runtime_settings_json(),
                "possible_settings": self.possible_runtime_settings_json(),
            })
            .to_string();
        }

        let mut updated_fields = serde_json::Map::new();
        let mut override_cwd = None;
        let mut override_model = None;
        let mut override_effort = None;
        let mut override_service_tier = None;
        let mut override_personality = None;

        if let Some(path) = resolved_cwd {
            self.set_working_directory(path.clone());
            self.app_event_tx
                .send(AppEvent::UpdateWorkingDirectory(path.clone()));
            self.app_event_tx.send(AppEvent::CodexOp(Op::ListSkills {
                cwds: Vec::new(),
                force_reload: false,
            }));
            updated_fields.insert(
                "working_directory".to_string(),
                json!(path.display().to_string()),
            );
            override_cwd = Some(path);
        }

        if model.is_some() || parsed_reasoning_effort.is_some() {
            if model.is_some() {
                updated_fields.insert("model".to_string(), json!(target_model.clone()));
                override_model = Some(target_model.clone());
            }
            updated_fields.insert(
                "reasoning_effort".to_string(),
                json!(Self::status_line_reasoning_effort_label(target_effort)),
            );
            override_effort = Some(target_effort);
            self.set_model(&target_model);
            self.set_reasoning_effort(target_effort);
        }

        if let Some(enabled) = fast_mode {
            let next_service_tier = enabled.then_some(ServiceTier::Fast);
            self.set_service_tier(next_service_tier);
            updated_fields.insert("fast_mode".to_string(), json!(enabled));
            override_service_tier = Some(next_service_tier);
        }

        if let Some(personality) = parsed_personality {
            self.set_personality(personality);
            updated_fields.insert(
                "personality".to_string(),
                json!(self.realtime_personality_name(personality)),
            );
            override_personality = Some(personality);
        }

        if let Some(mask) = target_collaboration_mask {
            self.set_collaboration_mask(mask);
            updated_fields.insert(
                "collaboration_mode".to_string(),
                json!(self.realtime_collaboration_mode_name(self.active_mode_kind())),
            );
        }

        if override_cwd.is_some()
            || override_model.is_some()
            || override_effort.is_some()
            || override_service_tier.is_some()
            || override_personality.is_some()
        {
            self.app_event_tx
                .send(AppEvent::CodexOp(Op::OverrideTurnContext {
                    cwd: override_cwd,
                    approval_policy: None,
                    sandbox_policy: None,
                    windows_sandbox_level: None,
                    model: override_model,
                    effort: override_effort,
                    summary: None,
                    service_tier: override_service_tier,
                    collaboration_mode: None,
                    personality: override_personality,
                }));
        }

        self.refresh_status_line();
        self.request_redraw();

        json!({
            "status": "ok",
            "updated": updated_fields,
            "current_settings": self.current_runtime_settings_json(),
            "possible_settings": self.possible_runtime_settings_json(),
        })
        .to_string()
    }

    fn compact_conversation_via_realtime(&mut self) -> String {
        if self.bottom_pane.is_task_running() {
            return json!({
                "status": "error",
                "message": "Codex must be idle before compacting the conversation.",
                "possible_commands": ["compact", "review", "plan", "diff", "agent"],
            })
            .to_string();
        }

        self.clear_token_usage();
        self.app_event_tx.send(AppEvent::CodexOp(Op::Compact));
        json!({
            "status": "ok",
            "message": "Started compacting the current conversation.",
            "possible_commands": ["compact", "review", "plan", "diff", "agent"],
        })
        .to_string()
    }

    fn run_tui_command_via_realtime(&mut self, command: String, prompt: Option<String>) -> String {
        match command.trim().to_ascii_lowercase().as_str() {
            "compact" => self.compact_conversation_via_realtime(),
            "review" => self.review_via_realtime(prompt),
            "plan" => self.plan_via_realtime(prompt),
            "diff" => self.diff_via_realtime(),
            "agent" => self.agent_via_realtime(),
            other => json!({
                "status": "error",
                "message": format!(
                    "Unsupported TUI command `{other}`. Use compact, review, plan, diff, or agent."
                ),
                "possible_commands": ["compact", "review", "plan", "diff", "agent"],
            })
            .to_string(),
        }
    }

    fn review_via_realtime(&mut self, prompt: Option<String>) -> String {
        let review_request = match prompt.as_deref().map(str::trim) {
            Some(prompt) if !prompt.is_empty() => ReviewRequest {
                target: ReviewTarget::Custom {
                    instructions: prompt.to_string(),
                },
                user_facing_hint: None,
            },
            _ => ReviewRequest {
                target: ReviewTarget::UncommittedChanges,
                user_facing_hint: None,
            },
        };
        if !self.submit_op(Op::Review {
            review_request: review_request.clone(),
        }) {
            return json!({
                "status": "error",
                "message": "Failed to start review.",
                "possible_commands": ["compact", "review", "plan", "diff", "agent"],
            })
            .to_string();
        }

        json!({
            "status": "ok",
            "command": "review",
            "target": review_request.target,
            "message": "Started review.",
            "possible_commands": ["compact", "review", "plan", "diff", "agent"],
        })
        .to_string()
    }

    fn plan_via_realtime(&mut self, prompt: Option<String>) -> String {
        if !self.collaboration_modes_enabled() {
            return json!({
                "status": "error",
                "message": "Collaboration modes are disabled.",
                "possible_commands": ["compact", "review", "plan", "diff", "agent"],
            })
            .to_string();
        }

        let Some(mask) = collaboration_modes::plan_mask(self.models_manager.as_ref()) else {
            return json!({
                "status": "error",
                "message": "Plan mode is unavailable right now.",
                "possible_commands": ["compact", "review", "plan", "diff", "agent"],
            })
            .to_string();
        };

        self.set_collaboration_mask(mask);

        let trimmed_prompt = prompt.unwrap_or_default().trim().to_string();
        if trimmed_prompt.is_empty() {
            return json!({
                "status": "ok",
                "command": "plan",
                "message": "Switched to Plan mode.",
                "current_settings": self.current_runtime_settings_json(),
                "possible_commands": ["compact", "review", "plan", "diff", "agent"],
            })
            .to_string();
        }

        let user_message = UserMessage {
            text: trimmed_prompt,
            local_images: Vec::new(),
            remote_image_urls: Vec::new(),
            text_elements: Vec::new(),
            mention_bindings: Vec::new(),
        };
        if !self.is_session_configured()
            || self.bottom_pane.is_task_running()
            || self.is_review_mode
        {
            self.queue_user_message(user_message);
            json!({
                "status": "ok",
                "command": "plan",
                "message": "Switched to Plan mode and queued the plan request.",
                "current_settings": self.current_runtime_settings_json(),
                "possible_commands": ["compact", "review", "plan", "diff", "agent"],
            })
            .to_string()
        } else {
            self.submit_user_message(user_message);
            json!({
                "status": "ok",
                "command": "plan",
                "message": "Switched to Plan mode and submitted the plan request.",
                "current_settings": self.current_runtime_settings_json(),
                "possible_commands": ["compact", "review", "plan", "diff", "agent"],
            })
            .to_string()
        }
    }

    fn diff_via_realtime(&mut self) -> String {
        self.add_diff_in_progress();
        let tx = self.app_event_tx.clone();
        tokio::spawn(async move {
            let text = match get_git_diff().await {
                Ok((is_git_repo, diff_text)) => {
                    if is_git_repo {
                        diff_text
                    } else {
                        "`/diff` — _not inside a git repository_".to_string()
                    }
                }
                Err(err) => format!("Failed to compute diff: {err}"),
            };
            tx.send(AppEvent::DiffResult(text));
        });

        json!({
            "status": "ok",
            "command": "diff",
            "message": "Started computing the current git diff.",
            "possible_commands": ["compact", "review", "plan", "diff", "agent"],
        })
        .to_string()
    }

    fn agent_via_realtime(&mut self) -> String {
        self.app_event_tx.send(AppEvent::OpenAgentPicker);
        json!({
            "status": "ok",
            "command": "agent",
            "message": "Opened the agent picker.",
            "possible_commands": ["compact", "review", "plan", "diff", "agent"],
        })
        .to_string()
    }

    fn resolve_realtime_working_directory(
        &self,
        working_directory: &str,
    ) -> Result<PathBuf, String> {
        let trimmed = working_directory.trim();
        if trimmed.is_empty() {
            return Err("The working_directory must not be empty.".to_string());
        }

        let requested_path = PathBuf::from(trimmed);
        let resolved_path = if requested_path.is_absolute() {
            requested_path
        } else {
            self.config.cwd.join(requested_path)
        };
        let canonical_path = std::fs::canonicalize(&resolved_path).map_err(|err| {
            format!(
                "Failed to resolve working_directory {}: {err}",
                resolved_path.display()
            )
        })?;
        if !canonical_path.is_dir() {
            return Err(format!(
                "The working_directory must point to a directory: {}",
                canonical_path.display()
            ));
        }
        Ok(canonical_path)
    }

    fn find_realtime_model_preset(
        &self,
        model: &str,
    ) -> Result<codex_protocol::openai_models::ModelPreset, String> {
        let models = self.models_manager.try_list_models().map_err(|_| {
            "Models are being updated right now. Try again in a moment.".to_string()
        })?;
        models
            .into_iter()
            .find(|preset| preset.model == model)
            .ok_or_else(|| {
                format!(
                    "Unknown model `{model}`. Call manage_runtime_settings with no setting fields or use a model from /model."
                )
            })
    }

    fn parse_realtime_reasoning_effort(
        &self,
        reasoning_effort: &str,
    ) -> Result<Option<ReasoningEffortConfig>, String> {
        match reasoning_effort.trim().to_ascii_lowercase().as_str() {
            "default" => Ok(None),
            "none" => Ok(Some(ReasoningEffortConfig::None)),
            "minimal" => Ok(Some(ReasoningEffortConfig::Minimal)),
            "low" => Ok(Some(ReasoningEffortConfig::Low)),
            "medium" => Ok(Some(ReasoningEffortConfig::Medium)),
            "high" => Ok(Some(ReasoningEffortConfig::High)),
            "xhigh" => Ok(Some(ReasoningEffortConfig::XHigh)),
            other => Err(format!(
                "Unsupported reasoning_effort `{other}`. Use default, none, minimal, low, medium, high, or xhigh."
            )),
        }
    }

    fn parse_realtime_personality(&self, personality: &str) -> Result<Personality, String> {
        match personality.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(Personality::None),
            "friendly" => Ok(Personality::Friendly),
            "pragmatic" => Ok(Personality::Pragmatic),
            other => Err(format!(
                "Unsupported personality `{other}`. Use none, friendly, or pragmatic."
            )),
        }
    }

    fn parse_realtime_collaboration_mode(&self, mode: &str) -> Result<ModeKind, String> {
        match mode.trim().to_ascii_lowercase().as_str() {
            "default" => Ok(ModeKind::Default),
            "plan" => Ok(ModeKind::Plan),
            other => Err(format!(
                "Unsupported collaboration_mode `{other}`. Use default or plan."
            )),
        }
    }

    fn realtime_personality_name(&self, personality: Personality) -> &'static str {
        match personality {
            Personality::None => "none",
            Personality::Friendly => "friendly",
            Personality::Pragmatic => "pragmatic",
        }
    }

    fn realtime_collaboration_mode_name(&self, mode: ModeKind) -> &'static str {
        match mode {
            ModeKind::Plan => "plan",
            ModeKind::Default | ModeKind::PairProgramming | ModeKind::Execute => "default",
        }
    }

    #[cfg(all(not(target_os = "linux"), feature = "voice-input"))]
    fn start_realtime_local_audio(&mut self) {
        if self.realtime_conversation.capture_stop_flag.is_some() {
            return;
        }

        let placeholder_id = self.bottom_pane.insert_transcription_placeholder("⠤⠤⠤⠤");
        self.realtime_conversation.meter_placeholder_id = Some(placeholder_id.clone());
        self.request_redraw();

        let capture = match crate::voice::VoiceCapture::start_realtime(
            &self.config,
            self.app_event_tx.clone(),
        ) {
            Ok(capture) => capture,
            Err(err) => {
                self.remove_transcription_placeholder(&placeholder_id);
                self.realtime_conversation.meter_placeholder_id = None;
                self.add_error_message(format!("Failed to start microphone capture: {err}"));
                return;
            }
        };

        let stop_flag = capture.stopped_flag();
        let peak = capture.last_peak_arc();
        let meter_placeholder_id = placeholder_id;
        let app_event_tx = self.app_event_tx.clone();

        self.realtime_conversation.capture_stop_flag = Some(stop_flag.clone());
        self.realtime_conversation.capture = Some(capture);
        if self.realtime_conversation.audio_player.is_none() {
            self.realtime_conversation.audio_player =
                crate::voice::RealtimeAudioPlayer::start(&self.config).ok();
        }

        std::thread::spawn(move || {
            let mut meter = crate::voice::RecordingMeterState::new();

            loop {
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }

                let meter_text = meter.next_text(peak.load(Ordering::Relaxed));
                app_event_tx.send(AppEvent::UpdateRecordingMeter {
                    id: meter_placeholder_id.clone(),
                    text: meter_text,
                });

                std::thread::sleep(Duration::from_millis(60));
            }
        });
    }

    #[cfg(target_os = "linux")]
    fn start_realtime_local_audio(&mut self) {}

    #[cfg(all(not(target_os = "linux"), not(feature = "voice-input")))]
    fn start_realtime_local_audio(&mut self) {}

    #[cfg(all(not(target_os = "linux"), feature = "voice-input"))]
    pub(crate) fn restart_realtime_audio_device(&mut self, kind: RealtimeAudioDeviceKind) {
        if !self.realtime_conversation.is_active() {
            return;
        }

        match kind {
            RealtimeAudioDeviceKind::Microphone => {
                self.stop_realtime_microphone();
                self.start_realtime_local_audio();
            }
            RealtimeAudioDeviceKind::Speaker => {
                self.stop_realtime_speaker();
                match crate::voice::RealtimeAudioPlayer::start(&self.config) {
                    Ok(player) => {
                        self.realtime_conversation.audio_player = Some(player);
                    }
                    Err(err) => {
                        self.add_error_message(format!("Failed to start speaker output: {err}"));
                    }
                }
            }
        }
        self.request_redraw();
    }

    #[cfg(any(target_os = "linux", not(feature = "voice-input")))]
    pub(crate) fn restart_realtime_audio_device(&mut self, kind: RealtimeAudioDeviceKind) {
        let _ = kind;
    }

    #[cfg(not(target_os = "linux"))]
    fn stop_realtime_local_audio(&mut self) {
        self.stop_realtime_microphone();
        self.stop_realtime_speaker();
    }

    #[cfg(target_os = "linux")]
    fn stop_realtime_local_audio(&mut self) {
        self.realtime_conversation.meter_placeholder_id = None;
    }

    #[cfg(not(target_os = "linux"))]
    fn stop_realtime_microphone(&mut self) {
        if let Some(flag) = self.realtime_conversation.capture_stop_flag.take() {
            flag.store(true, Ordering::Relaxed);
        }
        if let Some(capture) = self.realtime_conversation.capture.take() {
            let _ = capture.stop();
        }
        if let Some(id) = self.realtime_conversation.meter_placeholder_id.take() {
            self.remove_transcription_placeholder(&id);
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn stop_realtime_speaker(&mut self) {
        if let Some(player) = self.realtime_conversation.audio_player.take() {
            player.clear();
        }
    }
}

fn realtime_text_from_handoff_request(handoff: &RealtimeHandoffRequested) -> Option<String> {
    let active_transcript = handoff
        .active_transcript
        .iter()
        .map(|entry| format!("{}: {}", entry.role, entry.text))
        .collect::<Vec<_>>()
        .join("\n");
    (!active_transcript.is_empty())
        .then_some(active_transcript)
        .or_else(|| {
            (!handoff.input_transcript.is_empty()).then(|| handoff.input_transcript.clone())
        })
}
