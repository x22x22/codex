use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::ListSelectionView;
use crate::bottom_pane::MultiSelectItem;
use crate::bottom_pane::MultiSelectPicker;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::SideContentWidth;
use crate::bottom_pane::custom_prompt_view::CustomPromptView;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::render::renderable::ColumnRenderable;
use anyhow::Result;
use ratatui::style::Stylize;
use ratatui::text::Line;

pub(crate) use crate::config_wizard_state::ConfigWizardAccessMode;
pub(crate) use crate::config_wizard_state::ConfigWizardApplyRequest;
pub(crate) use crate::config_wizard_state::ConfigWizardState;
pub(crate) use crate::config_wizard_state::ConfigWizardTextStep;
pub(crate) use crate::config_wizard_state::ConfigWizardWorkspaceWriteOption;

impl ConfigWizardState {
    pub(crate) fn access_mode_view(&self, app_event_tx: AppEventSender) -> ListSelectionView {
        let items = [
            ConfigWizardAccessMode::ReadOnly,
            ConfigWizardAccessMode::WorkspaceWrite,
            ConfigWizardAccessMode::FullAccess,
        ]
        .into_iter()
        .map(|access_mode| SelectionItem {
            name: access_mode.label().to_string(),
            description: Some(access_mode.description().to_string()),
            is_current: self.access_mode == access_mode,
            actions: vec![Box::new(move |tx| {
                tx.send(AppEvent::ConfigWizardAccessModeSelected { access_mode });
            })],
            dismiss_on_select: true,
            ..Default::default()
        })
        .collect();

        ListSelectionView::new(
            SelectionViewParams {
                title: Some("Sandbox Setup: Access".to_string()),
                subtitle: Some(self.access_mode_subtitle()),
                footer_hint: Some(standard_popup_hint_line()),
                items,
                side_content: preview_renderable(self),
                side_content_width: SideContentWidth::Half,
                side_content_min_width: 36,
                ..Default::default()
            },
            app_event_tx,
        )
    }

    pub(crate) fn workspace_write_options_picker(
        &self,
        app_event_tx: AppEventSender,
    ) -> MultiSelectPicker {
        let selected_options = self.selected_workspace_write_options();
        MultiSelectPicker::builder(
            "Sandbox Setup: Workspace-Write Options".to_string(),
            Some(self.workspace_write_subtitle()),
            app_event_tx,
        )
        .items(
            ConfigWizardWorkspaceWriteOption::ALL
                .into_iter()
                .map(|option| MultiSelectItem {
                    id: option.key().to_string(),
                    name: option.label().to_string(),
                    description: Some(option.description().to_string()),
                    enabled: selected_options.contains(&option),
                })
                .collect(),
        )
        .on_preview(|items| Some(Line::from(selection_preview(items, "Enabled"))))
        .on_confirm(|selected_ids: &[String], tx: &AppEventSender| {
            let selected = selected_ids
                .iter()
                .filter_map(|id| ConfigWizardWorkspaceWriteOption::from_key(id))
                .collect();
            tx.send(AppEvent::ConfigWizardWorkspaceWriteOptionsSelected { selected });
        })
        .on_cancel(|tx: &AppEventSender| {
            tx.send(AppEvent::OpenConfigWizardAccessMode);
        })
        .left_arrow_cancels()
        .build()
    }

    pub(crate) fn text_step_view(
        &self,
        step: ConfigWizardTextStep,
        app_event_tx: AppEventSender,
    ) -> CustomPromptView {
        let submit_tx = app_event_tx.clone();
        let mut view = CustomPromptView::new(
            ConfigWizardState::prompt_title(step).to_string(),
            ConfigWizardState::prompt_placeholder(step).to_string(),
            self.prompt_context(step),
            Box::new(move |value: String| {
                submit_tx.send(AppEvent::ConfigWizardTextSubmitted { step, value });
            }),
        )
        .with_initial_text(self.prompt_initial_value(step).unwrap_or_default())
        .submit_empty_input()
        .left_arrow_cancels_at_start();

        if Self::back_event_for_text_step(step).is_some() {
            let cancel_tx = app_event_tx;
            let cancel_step = step;
            view = view.with_cancel_handler(Box::new(move || {
                if let Some(event) = Self::back_event_for_text_step(cancel_step) {
                    cancel_tx.send(event);
                }
            }));
        }
        view
    }

    pub(crate) fn summary_view(&self, app_event_tx: AppEventSender) -> Result<ListSelectionView> {
        let apply_request = self.build_apply_request()?;
        let uses_workspace_write = self.uses_workspace_write();
        let back_item = if self.uses_workspace_write() {
            SelectionItem {
                name: "Back to directories".to_string(),
                description: Some("Adjust the directories you want Codex to edit.".to_string()),
                actions: vec![Box::new(|tx| {
                    tx.send(AppEvent::OpenConfigWizardTextStep {
                        step: ConfigWizardTextStep::WritableRoots,
                    });
                })],
                dismiss_on_select: true,
                ..Default::default()
            }
        } else {
            SelectionItem {
                name: "Back to access mode".to_string(),
                description: Some("Choose a different sandbox mode.".to_string()),
                actions: vec![Box::new(|tx| {
                    tx.send(AppEvent::OpenConfigWizardAccessMode);
                })],
                dismiss_on_select: true,
                ..Default::default()
            }
        };
        let items = vec![
            SelectionItem {
                name: "Apply sandbox settings".to_string(),
                description: Some("Save these sandbox settings to config.toml.".to_string()),
                actions: vec![Box::new(move |tx| {
                    tx.send(AppEvent::ApplyConfigWizard {
                        request: apply_request.clone(),
                    });
                })],
                dismiss_on_select: true,
                ..Default::default()
            },
            back_item,
        ];

        Ok(ListSelectionView::new(
            SelectionViewParams {
                title: Some("Sandbox Setup: Summary".to_string()),
                subtitle: Some("Review the sandbox settings before applying them.".to_string()),
                footer_hint: Some(standard_popup_hint_line()),
                items,
                side_content: preview_renderable(self),
                side_content_width: SideContentWidth::Half,
                side_content_min_width: 36,
                on_cancel: Some(Box::new(move |tx| {
                    tx.send(Self::summary_back_event(uses_workspace_write));
                })),
                left_arrow_cancels: true,
                ..Default::default()
            },
            app_event_tx,
        ))
    }

    fn back_event_for_text_step(step: ConfigWizardTextStep) -> Option<AppEvent> {
        match step {
            ConfigWizardTextStep::WritableRoots => {
                Some(AppEvent::OpenConfigWizardWorkspaceWriteOptions)
            }
        }
    }

    fn summary_back_event(uses_workspace_write: bool) -> AppEvent {
        if uses_workspace_write {
            AppEvent::OpenConfigWizardTextStep {
                step: ConfigWizardTextStep::WritableRoots,
            }
        } else {
            AppEvent::OpenConfigWizardAccessMode
        }
    }
}

fn selection_preview(items: &[MultiSelectItem], prefix: &str) -> String {
    let selected = items
        .iter()
        .filter(|item| item.enabled)
        .map(|item| item.name.as_str())
        .collect::<Vec<_>>();
    if selected.is_empty() {
        format!("{prefix}: none")
    } else {
        format!("{prefix}: {}", selected.join(", "))
    }
}

fn preview_renderable(state: &ConfigWizardState) -> Box<dyn crate::render::renderable::Renderable> {
    let mut renderable = ColumnRenderable::new();
    renderable.push("Config preview:".bold());
    renderable.push("");
    for line in state.preview_toml().lines() {
        renderable.push(Line::from(line.to_string()));
    }
    Box::new(renderable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_event_sender::AppEventSender;
    use crate::bottom_pane::BottomPaneView;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc::UnboundedReceiver;
    use tokio::sync::mpsc::unbounded_channel;

    fn test_sender() -> (AppEventSender, UnboundedReceiver<AppEvent>) {
        let (tx, rx) = unbounded_channel();
        (AppEventSender::new(tx), rx)
    }

    fn test_state() -> ConfigWizardState {
        ConfigWizardState::test_state()
    }

    #[test]
    fn writable_roots_enter_allows_empty_submission() {
        let mut state = test_state();
        state.writable_roots.clear();
        let (tx, mut rx) = test_sender();
        let mut view = state.text_step_view(ConfigWizardTextStep::WritableRoots, tx);

        view.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(
            view.is_complete(),
            "expected writable-roots prompt to complete"
        );
        match rx
            .try_recv()
            .expect("expected config wizard submission event")
        {
            AppEvent::ConfigWizardTextSubmitted { step, value } => {
                assert_eq!(step, ConfigWizardTextStep::WritableRoots);
                assert!(
                    value.is_empty(),
                    "expected empty submission to preserve defaults"
                );
            }
            event => panic!("unexpected app event: {event:?}"),
        }
    }

    #[test]
    fn writable_roots_submits_prefilled_recommendations() {
        let mut state = test_state();
        state.access_mode = ConfigWizardAccessMode::WorkspaceWrite;
        state.writable_roots = vec![
            AbsolutePathBuf::from_absolute_path("/Users/viyatb/code/openai")
                .expect("absolute path"),
            AbsolutePathBuf::from_absolute_path("/Users/viyatb/code/infra").expect("absolute path"),
        ];
        state.network_access = true;
        let (tx, mut rx) = test_sender();
        let mut view = state.text_step_view(ConfigWizardTextStep::WritableRoots, tx);

        view.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        match rx
            .try_recv()
            .expect("expected config wizard submission event")
        {
            AppEvent::ConfigWizardTextSubmitted { step, value } => {
                assert_eq!(step, ConfigWizardTextStep::WritableRoots);
                assert_eq!(value, "/Users/viyatb/code/openai\n/Users/viyatb/code/infra");
            }
            event => panic!("unexpected app event: {event:?}"),
        }
    }

    #[test]
    fn writable_roots_escape_reopens_workspace_write_options() {
        let mut state = test_state();
        state.writable_roots.clear();
        let (tx, mut rx) = test_sender();
        let mut view = state.text_step_view(ConfigWizardTextStep::WritableRoots, tx);

        let cancellation = view.on_ctrl_c();

        assert_eq!(cancellation, crate::bottom_pane::CancellationEvent::Handled);
        assert!(
            view.is_complete(),
            "expected writable-roots prompt to complete on escape"
        );
        match rx.try_recv().expect("expected wizard back event") {
            AppEvent::OpenConfigWizardWorkspaceWriteOptions => {}
            event => panic!("unexpected app event: {event:?}"),
        }
    }

    #[test]
    fn workspace_write_options_left_arrow_reopens_access_mode() {
        let state = test_state();
        let (tx, mut rx) = test_sender();
        let mut view = state.workspace_write_options_picker(tx);

        view.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));

        assert!(
            view.is_complete(),
            "expected left arrow to close workspace-write options"
        );
        match rx.try_recv().expect("expected wizard back event") {
            AppEvent::OpenConfigWizardAccessMode => {}
            event => panic!("unexpected app event: {event:?}"),
        }
    }
}
