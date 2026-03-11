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
            text,
            local_images,
            remote_image_urls,
            text_elements,
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
        self.submit_op(Op::RealtimeConversationClose);
        self.stop_realtime_local_audio();
        self.set_footer_hint_override(None);

        if let Some(message) = info_message {
            self.add_info_message(message, None);
        } else {
            self.request_redraw();
        }
    }

    pub(super) fn reset_realtime_conversation_state(&mut self) {
        self.stop_realtime_local_audio();
        self.set_footer_hint_override(None);
        self.realtime_conversation.phase = RealtimeConversationPhase::Inactive;
        self.realtime_conversation.requested_close = false;
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
                    "Realtime voice mode turned off.".to_string(),
                ));
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
