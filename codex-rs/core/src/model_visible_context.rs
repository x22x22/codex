//! Shared model-visible context abstractions.
//!
//! Use this path for any injected prompt context, whether it renders in the
//! developer envelope or the contextual-user envelope.
//!
//! Fragment registration and concrete fragment definitions live in
//! `model_visible_fragments.rs`. This module keeps only the shared rendering,
//! role, and turn-context parameter helpers that every fragment uses.
//!
//! Contributor guide for adding new model-visible context:
//!
//! 1. Start by deciding whether the new context should be model-visible at
//!    all. If it is only for UI/state bookkeeping and the model should never
//!    see it, do not add a fragment.
//! 2. If the model should see it, define a typed fragment in
//!    `model_visible_fragments.rs` and implement
//!    `ModelVisibleContextFragment`. Do not hand-construct raw
//!    `ResponseItem::Message` values for prompt context in new code.
//! 3. Choose the role carefully:
//!    - `DeveloperContextRole` is for developer guidance, policies, or other
//!      system-owned instructions that should live in the single developer
//!      envelope.
//!    - `ContextualUserContextRole` is for contextual state/runtime markers that
//!      are intentionally represented as user-role history so parsing can treat
//!      them as contextual state instead of user intent.
//! 4. Decide whether the fragment is turn-state-derived:
//!    - If it is derived from current durable turn/session state and should be
//!      reinjected or diffed correctly across resume, compaction, backtracking,
//!      or fork, implement `build(...)`.
//!    - `build(...)` receives the current `TurnContext`, an optional
//!      `reference_context_item`, and `TurnContextDiffParams`.
//!      `reference_context_item` is the baseline state already represented in
//!      model-visible history; compare against it to avoid emitting duplicate
//!      context, and fall back to `TurnContextDiffParams` when there is no
//!      baseline but previous runtime/session state still matters.
//!    - If the fragment is a one-off runtime marker or session-prefix message,
//!      leave `build(...)` as `None`; it should still be defined and
//!      registered so rendering/detection stay standardized.
//! 5. If the fragment has contextual-user role, make sure persisted history can
//!    recognize it:
//!    - Prefer `contextual_user_markers()` when the fragment has stable wrapper
//!      markers.
//!    - Override `matches_contextual_user_text()` only when fixed markers are
//!      not enough.
//!    - Every contextual-user fragment must have a stable detection path so old
//!      history is parsed as contextual state rather than as literal user
//!      input.
//! 6. Register the fragment exactly once in the central ordered registry in
//!    `model_visible_fragments.rs`.
//!    - Registration is what makes the fragment participate in contextual-user
//!      detection.
//!    - Registration is also what makes turn-state fragments participate in the
//!      shared initial-context and per-turn diff assembly.
//!    - The registry order is the canonical prompt order, so place the new
//!      fragment where it should appear relative to the others.
//! 7. Keep the two-envelope invariant intact:
//!    - turn-state developer fragments are grouped into one developer message
//!    - turn-state contextual-user fragments are grouped into one
//!      contextual-user message
//!    - runtime/session-prefix fragments may still be emitted as standalone
//!      messages when the event itself is the thing being recorded
//! 8. Prefer fragment-local logic over ad hoc helpers. The fragment type should
//!    own:
//!    - how the text is rendered
//!    - how current state is inspected
//!    - how diffing against an existing baseline works
//!    - how contextual-user detection works, if applicable
//! 9. Keep legacy compatibility bounded. If old shipped history needs special
//!    detection for a wrapper we no longer emit, add a small legacy shim in the
//!    detection path rather than inventing a fake new fragment type.
//!
//! Rule of thumb:
//! - “Should the model see this every turn as part of durable prompt state?”
//!   => typed fragment + `build(...)` + registry.
//! - “Should the model see this only because a runtime event just happened?”
//!   => typed fragment + registry, but no `build(...)`.
//! - “Should the model not see this at all?” => not a fragment.

use crate::codex::PreviousTurnSettings;
use crate::codex::TurnContext;
use crate::shell::Shell;
use codex_execpolicy::Policy;
use codex_protocol::models::ContentItem;
use codex_protocol::models::CustomDeveloperInstructions;
use codex_protocol::models::MessageRole;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::TurnContextItem;

pub(crate) const SKILL_OPEN_TAG: &str = "<skill>";
pub(crate) const SKILL_CLOSE_TAG: &str = "</skill>";
pub(crate) const USER_SHELL_COMMAND_OPEN_TAG: &str = "<user_shell_command>";
pub(crate) const USER_SHELL_COMMAND_CLOSE_TAG: &str = "</user_shell_command>";
pub(crate) const TURN_ABORTED_OPEN_TAG: &str = "<turn_aborted>";
pub(crate) const TURN_ABORTED_CLOSE_TAG: &str = "</turn_aborted>";
pub(crate) const PLUGINS_OPEN_TAG: &str = "<plugins>";
pub(crate) const PLUGINS_CLOSE_TAG: &str = "</plugins>";
pub(crate) const SUBAGENTS_OPEN_TAG: &str = "<subagents>";
pub(crate) const SUBAGENTS_CLOSE_TAG: &str = "</subagents>";
pub(crate) const SUBAGENT_NOTIFICATION_OPEN_TAG: &str = "<subagent_notification>";
pub(crate) const SUBAGENT_NOTIFICATION_CLOSE_TAG: &str = "</subagent_notification>";

pub(crate) trait ModelVisibleContextRole {
    const MESSAGE_ROLE: MessageRole;
}

pub(crate) struct DeveloperContextRole;

impl ModelVisibleContextRole for DeveloperContextRole {
    const MESSAGE_ROLE: MessageRole = MessageRole::Developer;
}

pub(crate) struct ContextualUserContextRole;

impl ModelVisibleContextRole for ContextualUserContextRole {
    const MESSAGE_ROLE: MessageRole = MessageRole::User;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContextualUserFragmentMarkers {
    start_marker: &'static str,
    end_marker: &'static str,
}

impl ContextualUserFragmentMarkers {
    pub(crate) const fn new(start_marker: &'static str, end_marker: &'static str) -> Self {
        Self {
            start_marker,
            end_marker,
        }
    }

    pub(crate) fn matches_text(self, text: &str) -> bool {
        let trimmed = text.trim_start();
        let starts_with_marker = trimmed
            .get(..self.start_marker.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(self.start_marker));
        let trimmed = trimmed.trim_end();
        let ends_with_marker = trimmed
            .get(trimmed.len().saturating_sub(self.end_marker.len())..)
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(self.end_marker));
        starts_with_marker && ends_with_marker
    }

    pub(crate) fn wrap_body(self, body: String) -> String {
        format!("{}\n{}\n{}", self.start_marker, body, self.end_marker)
    }
}

pub(crate) fn model_visible_content_item(text: String) -> ContentItem {
    ContentItem::InputText { text }
}

pub(crate) fn model_visible_message<R: ModelVisibleContextRole>(text: String) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: R::MESSAGE_ROLE.to_string(),
        content: vec![model_visible_content_item(text)],
        end_turn: None,
        phase: None,
    }
}

pub(crate) fn model_visible_response_input_item<R: ModelVisibleContextRole>(
    text: String,
) -> ResponseInputItem {
    ResponseInputItem::Message {
        role: R::MESSAGE_ROLE.to_string(),
        content: vec![model_visible_content_item(text)],
    }
}

pub(crate) struct TurnContextDiffParams<'a> {
    pub(crate) shell: &'a Shell,
    pub(crate) previous_turn_settings: Option<&'a PreviousTurnSettings>,
    pub(crate) exec_policy: &'a Policy,
    pub(crate) personality_feature_enabled: bool,
    pub(crate) base_instructions: Option<&'a str>,
}

impl<'a> TurnContextDiffParams<'a> {
    pub(crate) fn new(
        shell: &'a Shell,
        previous_turn_settings: Option<&'a PreviousTurnSettings>,
        exec_policy: &'a Policy,
        personality_feature_enabled: bool,
        base_instructions: Option<&'a str>,
    ) -> Self {
        Self {
            shell,
            previous_turn_settings,
            exec_policy,
            personality_feature_enabled,
            base_instructions,
        }
    }
}

/// Implement this for any model-visible prompt fragment, regardless of which
/// envelope it renders into.
pub(crate) trait ModelVisibleContextFragment: Sized {
    type Role: ModelVisibleContextRole;

    fn render_text(&self) -> String;

    /// Build the fragment from the current turn state and an optional baseline
    /// context item that represents the turn state already reflected in
    /// model-visible history.
    ///
    /// Implementations that are not turn-state fragments should leave the
    /// default `None`.
    fn build(
        _turn_context: &TurnContext,
        _reference_context_item: Option<&TurnContextItem>,
        _params: &TurnContextDiffParams<'_>,
    ) -> Option<Self> {
        None
    }

    /// Stable markers used to recognize contextual-user fragments in persisted
    /// history. Developer fragments should keep the default `None`.
    fn contextual_user_markers() -> Option<ContextualUserFragmentMarkers> {
        None
    }

    fn matches_contextual_user_text(text: &str) -> bool {
        Self::contextual_user_markers().is_some_and(|markers| markers.matches_text(text))
    }

    fn wrap_contextual_user_body(body: String) -> String {
        let Some(markers) = Self::contextual_user_markers() else {
            panic!("contextual-user fragments using wrap_contextual_user_body must define markers");
        };
        markers.wrap_body(body)
    }

    fn into_content_item(self) -> ContentItem {
        model_visible_content_item(self.render_text())
    }

    fn into_message(self) -> ResponseItem {
        model_visible_message::<Self::Role>(self.render_text())
    }

    fn into_response_input_item(self) -> ResponseInputItem {
        model_visible_response_input_item::<Self::Role>(self.render_text())
    }
}

pub(crate) struct DeveloperTextFragment {
    text: String,
}

impl DeveloperTextFragment {
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

pub(crate) struct ContextualUserTextFragment {
    text: String,
}

impl ContextualUserTextFragment {
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

impl ModelVisibleContextFragment for CustomDeveloperInstructions {
    type Role = DeveloperContextRole;

    fn render_text(&self) -> String {
        self.clone().into_text()
    }
}

impl ModelVisibleContextFragment for DeveloperTextFragment {
    type Role = DeveloperContextRole;

    fn render_text(&self) -> String {
        self.text.clone()
    }
}

impl ModelVisibleContextFragment for ContextualUserTextFragment {
    type Role = ContextualUserContextRole;

    fn render_text(&self) -> String {
        self.text.clone()
    }
}
