use ratatui::layout::Rect;

use super::fork_session_overlay::OverlayCommandState;
use super::fork_session_overlay_mouse::PopupDragState;
use super::fork_session_terminal::ForkSessionTerminal;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum OverlayFocusedPane {
    Background,
    #[default]
    Popup,
}

pub(crate) struct ForkSessionOverlayState {
    pub(crate) terminal: ForkSessionTerminal,
    pub(crate) popup: Rect,
    pub(crate) command_state: OverlayCommandState,
    pub(crate) drag_state: Option<PopupDragState>,
}

pub(crate) struct ForkSessionOverlayStack {
    popups: Vec<ForkSessionOverlayState>,
    focused_pane: OverlayFocusedPane,
}

impl ForkSessionOverlayStack {
    pub(crate) fn new(popup: ForkSessionOverlayState) -> Self {
        Self {
            popups: vec![popup],
            focused_pane: OverlayFocusedPane::Popup,
        }
    }

    pub(crate) fn popups(&self) -> &[ForkSessionOverlayState] {
        &self.popups
    }

    pub(crate) fn popups_mut(&mut self) -> &mut [ForkSessionOverlayState] {
        &mut self.popups
    }

    pub(crate) fn active_popup(&self) -> Option<&ForkSessionOverlayState> {
        self.popups.last()
    }

    pub(crate) fn active_popup_mut(&mut self) -> Option<&mut ForkSessionOverlayState> {
        self.popups.last_mut()
    }

    pub(crate) fn active_popup_index(&self) -> Option<usize> {
        self.popups.len().checked_sub(1)
    }

    pub(crate) fn focused_pane(&self) -> OverlayFocusedPane {
        self.focused_pane
    }

    pub(crate) fn has_background_focus(&self) -> bool {
        self.focused_pane == OverlayFocusedPane::Background
    }

    pub(crate) fn push_popup(&mut self, popup: ForkSessionOverlayState) {
        self.clear_active_interaction();
        self.popups.push(popup);
        self.focused_pane = OverlayFocusedPane::Popup;
    }

    pub(crate) fn set_focused_pane(&mut self, focused_pane: OverlayFocusedPane) {
        if focused_pane == OverlayFocusedPane::Popup && self.popups.is_empty() {
            return;
        }
        self.clear_active_interaction();
        self.focused_pane = focused_pane;
    }

    pub(crate) fn bring_popup_to_front(
        &mut self,
        index: usize,
    ) -> Option<&mut ForkSessionOverlayState> {
        if index >= self.popups.len() {
            return None;
        }
        self.clear_active_interaction();
        if index + 1 != self.popups.len() {
            let popup = self.popups.remove(index);
            self.popups.push(popup);
        }
        self.focused_pane = OverlayFocusedPane::Popup;
        self.popups.last_mut()
    }

    pub(crate) fn close_active_popup(&mut self) -> Option<ForkSessionOverlayState> {
        self.clear_active_interaction();
        let closed = self.popups.pop();
        if self.popups.is_empty() {
            self.focused_pane = OverlayFocusedPane::Background;
        }
        closed
    }

    pub(crate) fn remove_exited_popups(&mut self) -> bool {
        let before = self.popups.len();
        self.popups
            .retain(|popup| popup.terminal.exit_code().is_none());
        if self.popups.is_empty() {
            self.focused_pane = OverlayFocusedPane::Background;
        }
        self.popups.len() != before
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.popups.is_empty()
    }

    fn clear_active_interaction(&mut self) {
        if let Some(active_popup) = self.popups.last_mut() {
            active_popup.command_state = OverlayCommandState::PassThrough;
            active_popup.drag_state = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::fork_session_overlay::OverlayCommandState;
    use pretty_assertions::assert_eq;

    fn popup(x: u16, y: u16) -> ForkSessionOverlayState {
        ForkSessionOverlayState {
            terminal: ForkSessionTerminal::for_test(
                vt100::Parser::new(1, 1, 0),
                /*exit_code*/ None,
            ),
            popup: Rect::new(/*x*/ x, /*y*/ y, 40, 16),
            command_state: OverlayCommandState::PassThrough,
            drag_state: None,
        }
    }

    #[test]
    fn bring_popup_to_front_makes_clicked_popup_active() {
        let mut stack = ForkSessionOverlayStack::new(popup(/*x*/ 10, /*y*/ 10));
        stack.push_popup(popup(/*x*/ 20, /*y*/ 20));
        stack.push_popup(popup(/*x*/ 30, /*y*/ 30));

        let active = stack
            .bring_popup_to_front(/*index*/ 0)
            .expect("bring first popup to front");

        assert_eq!(active.popup, Rect::new(10, 10, 40, 16));
        assert_eq!(stack.active_popup_index(), Some(2));
        assert_eq!(stack.focused_pane(), OverlayFocusedPane::Popup);
    }

    #[test]
    fn close_active_popup_keeps_stack_alive_until_last_popup() {
        let mut stack = ForkSessionOverlayStack::new(popup(/*x*/ 10, /*y*/ 10));
        stack.push_popup(popup(/*x*/ 20, /*y*/ 20));

        let closed = stack.close_active_popup().expect("close topmost popup");

        assert_eq!(closed.popup, Rect::new(20, 20, 40, 16));
        assert_eq!(stack.popups().len(), 1);
        assert_eq!(stack.focused_pane(), OverlayFocusedPane::Popup);

        stack.close_active_popup().expect("close final popup");

        assert!(stack.is_empty());
        assert_eq!(stack.focused_pane(), OverlayFocusedPane::Background);
    }
}
