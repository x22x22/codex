use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;

use crate::ascii_animation::AsciiAnimation;
use crate::onboarding::onboarding_screen::StepStateProvider;
use crate::tui::FrameRequester;

use super::onboarding_screen::StepState;

const MIN_ANIMATION_HEIGHT: u16 = 21;
const MIN_ANIMATION_WIDTH: u16 = 60;

pub(crate) struct WelcomeWidget {
    pub is_logged_in: bool,
    animation: AsciiAnimation,
}

impl WelcomeWidget {
    pub(crate) fn new(is_logged_in: bool, request_frame: FrameRequester) -> Self {
        Self {
            is_logged_in,
            animation: AsciiAnimation::new(request_frame),
        }
    }

    pub(crate) fn handle_key_event(&mut self, key_event: KeyEvent) {
        if key_event.kind == KeyEventKind::Press
            && key_event.code == KeyCode::Char('.')
            && key_event.modifiers.contains(KeyModifiers::CONTROL)
        {
            let _ = self.animation.pick_random_variant();
        }
    }
}

impl WidgetRef for &WelcomeWidget {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        self.animation.schedule_next_frame();

        // Skip the animation entirely when the viewport is too small so we don't clip frames.
        let show_animation =
            area.height >= MIN_ANIMATION_HEIGHT && area.width >= MIN_ANIMATION_WIDTH;

        let mut lines: Vec<Line> = Vec::new();
        if show_animation {
            let frame = self.animation.current_frame();
            // let frame_line_count = frame.lines().count();
            // lines.reserve(frame_line_count + 2);
            lines.extend(frame.lines().map(|l| l.into()));
            lines.push("".into());
        }
        lines.push(Line::from(vec![
            "  ".into(),
            "Welcome to ".into(),
            "Codex".bold(),
            ", OpenAI's command-line coding agent".into(),
        ]));

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }
}

impl StepStateProvider for WelcomeWidget {
    fn get_step_state(&self) -> StepState {
        match self.is_logged_in {
            true => StepState::Hidden,
            false => StepState::Complete,
        }
    }
}
