use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;

use crate::terminal_palette::indexed_color;
use crate::terminal_palette::rgb_color;

pub(crate) fn render_screen(
    screen: &vt100::Screen,
    area: Rect,
    buf: &mut Buffer,
) -> Option<(u16, u16)> {
    if area.is_empty() {
        return None;
    }

    for row in 0..area.height {
        for col in 0..area.width {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            let mut fg = vt100_color_to_ratatui(cell.fgcolor());
            let mut bg = vt100_color_to_ratatui(cell.bgcolor());
            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
            }

            let mut style = Style::default().fg(fg).bg(bg);
            if cell.bold() {
                style = style.add_modifier(Modifier::BOLD);
            }
            if cell.dim() {
                style = style.add_modifier(Modifier::DIM);
            }
            if cell.italic() {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if cell.underline() {
                style = style.add_modifier(Modifier::UNDERLINED);
            }

            let symbol = if cell.is_wide_continuation() || cell.contents().is_empty() {
                " "
            } else {
                cell.contents()
            };
            buf[(area.x + col, area.y + row)]
                .set_symbol(symbol)
                .set_style(style);
        }
    }

    if screen.hide_cursor() {
        return None;
    }
    let (row, col) = screen.cursor_position();
    if row >= area.height || col >= area.width {
        return None;
    }
    Some((area.x + col, area.y + row))
}

fn vt100_color_to_ratatui(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(index) => indexed_color(index),
        vt100::Color::Rgb(red, green, blue) => rgb_color((red, green, blue)),
    }
}
