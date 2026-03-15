use crossterm::event::MouseButton;
use crossterm::event::MouseEvent;
use crossterm::event::MouseEventKind;
use ratatui::layout::Rect;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PopupDragState {
    column_offset: u16,
    row_offset: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OverlayMouseAction {
    Ignore,
    FocusBackground,
    FocusPopup {
        popup_index: usize,
        drag_state: PopupDragState,
    },
    MovePopup(Rect),
    EndDrag,
}

pub(crate) fn overlay_mouse_action(
    area: Rect,
    popups: &[Rect],
    drag_state: Option<PopupDragState>,
    mouse_event: MouseEvent,
) -> OverlayMouseAction {
    match mouse_event.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some((popup_index, popup)) =
                hit_popup(popups, mouse_event.column, mouse_event.row)
            {
                OverlayMouseAction::FocusPopup {
                    popup_index,
                    drag_state: PopupDragState {
                        column_offset: mouse_event.column.saturating_sub(popup.x),
                        row_offset: mouse_event.row.saturating_sub(popup.y),
                    },
                }
            } else {
                OverlayMouseAction::FocusBackground
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(drag_state) = drag_state
                && let Some(popup) = popups.last().copied()
            {
                let max_x = area.right().saturating_sub(popup.width);
                let max_y = area.bottom().saturating_sub(popup.height);
                let x = mouse_event
                    .column
                    .saturating_sub(drag_state.column_offset)
                    .clamp(area.x, max_x);
                let y = mouse_event
                    .row
                    .saturating_sub(drag_state.row_offset)
                    .clamp(area.y, max_y);
                OverlayMouseAction::MovePopup(Rect::new(x, y, popup.width, popup.height))
            } else {
                OverlayMouseAction::Ignore
            }
        }
        MouseEventKind::Up(MouseButton::Left) => OverlayMouseAction::EndDrag,
        MouseEventKind::Down(_)
        | MouseEventKind::Up(_)
        | MouseEventKind::Drag(_)
        | MouseEventKind::Moved
        | MouseEventKind::ScrollDown
        | MouseEventKind::ScrollUp
        | MouseEventKind::ScrollLeft
        | MouseEventKind::ScrollRight => OverlayMouseAction::Ignore,
    }
}

fn popup_contains_position(popup: Rect, column: u16, row: u16) -> bool {
    column >= popup.x && column < popup.right() && row >= popup.y && row < popup.bottom()
}

fn hit_popup(popups: &[Rect], column: u16, row: u16) -> Option<(usize, Rect)> {
    popups
        .iter()
        .copied()
        .enumerate()
        .rev()
        .find(|(_, popup)| popup_contains_position(*popup, column, row))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn mouse_down_outside_popup_focuses_background() {
        let popups = [Rect::new(20, 8, 40, 16)];
        let area = Rect::new(0, 0, 120, 40);
        let mouse_event = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 4,
            row: 3,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };

        assert_eq!(
            overlay_mouse_action(area, &popups, None, mouse_event),
            OverlayMouseAction::FocusBackground
        );
    }

    #[test]
    fn mouse_down_inside_popup_starts_drag() {
        let popups = [Rect::new(20, 8, 40, 16)];
        let area = Rect::new(0, 0, 120, 40);
        let mouse_event = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 27,
            row: 10,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };

        assert_eq!(
            overlay_mouse_action(area, &popups, None, mouse_event),
            OverlayMouseAction::FocusPopup {
                popup_index: 0,
                drag_state: PopupDragState {
                    column_offset: 7,
                    row_offset: 2,
                },
            }
        );
    }

    #[test]
    fn mouse_drag_moves_popup_and_clamps_to_viewport() {
        let area = Rect::new(0, 0, 120, 40);
        let popups = [Rect::new(20, 8, 40, 16)];
        let drag_state = PopupDragState {
            column_offset: 7,
            row_offset: 2,
        };
        let mouse_event = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 118,
            row: 39,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };

        assert_eq!(
            overlay_mouse_action(area, &popups, Some(drag_state), mouse_event),
            OverlayMouseAction::MovePopup(Rect::new(80, 24, 40, 16))
        );
    }

    #[test]
    fn mouse_up_ends_drag() {
        let popups = [Rect::new(20, 8, 40, 16)];
        let area = Rect::new(0, 0, 120, 40);
        let mouse_event = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 27,
            row: 10,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };

        assert_eq!(
            overlay_mouse_action(area, &popups, None, mouse_event),
            OverlayMouseAction::EndDrag
        );
    }

    #[test]
    fn mouse_down_hits_topmost_popup_first() {
        let popups = [Rect::new(20, 8, 40, 16), Rect::new(30, 12, 40, 16)];
        let area = Rect::new(0, 0, 120, 40);
        let mouse_event = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 35,
            row: 15,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };

        assert_eq!(
            overlay_mouse_action(area, &popups, None, mouse_event),
            OverlayMouseAction::FocusPopup {
                popup_index: 1,
                drag_state: PopupDragState {
                    column_offset: 5,
                    row_offset: 3,
                },
            }
        );
    }
}
