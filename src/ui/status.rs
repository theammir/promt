use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use crate::mode::Mode;

/// Status bar state.
#[derive(Debug)]
pub struct StatusState {
    pub provider: String,
    pub model: String,
    pub token_count: Option<usize>,
    pub is_streaming: bool,
    pub leader_pending: bool,
    pub mode: Mode,
    pub status_message: Option<String>,
}

impl StatusState {
    pub fn new(provider: &str, model: &str) -> Self {
        Self {
            provider: provider.to_string(),
            model: model.to_string(),
            token_count: None,
            is_streaming: false,
            leader_pending: false,
            mode: Mode::Insert,
            status_message: None,
        }
    }
}

/// Status bar widget.
pub struct StatusBarWidget<'a> {
    pub state: &'a StatusState,
}

impl Widget for StatusBarWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let bg = Style::default()
            .bg(Color::Rgb(30, 30, 40))
            .fg(Color::Rgb(180, 180, 190));

        // Fill background.
        for x in area.x..area.x + area.width {
            buf.cell_mut((x, area.y)).map(|cell| {
                cell.set_style(bg);
                cell.set_char(' ');
            });
        }

        let mut x = area.x + 1;

        // Mode indicator.
        let (mode_label, mode_color) = match self.state.mode {
            Mode::Insert => ("INSERT", Color::Rgb(100, 200, 100)),
            Mode::Browse => ("BROWSE", Color::Rgb(100, 149, 237)),
            Mode::CodeblockSelect => ("CODEBLOCK", Color::Rgb(200, 160, 80)),
            Mode::Visual => ("VISUAL", Color::Rgb(200, 100, 200)),
            Mode::VisualLine => ("V-LINE", Color::Rgb(200, 100, 200)),
        };
        let mode_style = Style::default()
            .fg(Color::Rgb(20, 20, 20))
            .bg(mode_color)
            .add_modifier(Modifier::BOLD);
        let mode_text = format!(" {mode_label} ");
        buf.set_string(x, area.y, &mode_text, mode_style);
        x += mode_text.len() as u16 + 1;

        // Provider/model.
        let model_text = format!("{}/{}", self.state.provider, self.state.model);
        buf.set_string(x, area.y, &model_text, bg);
        x += model_text.len() as u16;

        // Streaming indicator.
        if self.state.is_streaming {
            let spinner = " \u{25cf} streaming"; // ● streaming
            let style = bg.fg(Color::Rgb(255, 200, 80));
            buf.set_string(x + 1, area.y, spinner, style);
            x += spinner.len() as u16 + 1;
        }
        let _ = x; // consumed above, final value unused

        // Right-aligned section.
        let mut right_parts: Vec<(String, Style)> = Vec::new();

        // Token count.
        if let Some(tokens) = self.state.token_count {
            let text = if tokens >= 1000 {
                format!("{:.1}k tokens", tokens as f64 / 1000.0)
            } else {
                format!("{tokens} tokens")
            };
            right_parts.push((text, bg));
        }

        // Leader key indicator.
        if self.state.leader_pending {
            let text = "C-x ...".to_string();
            let style = bg
                .fg(Color::Rgb(255, 220, 100))
                .add_modifier(Modifier::BOLD);
            right_parts.push((text, style));
        }

        // Status message.
        if let Some(ref msg) = self.state.status_message {
            right_parts.push((msg.clone(), bg.fg(Color::Rgb(200, 100, 100))));
        }

        // Render right-aligned.
        let mut rx = area.x + area.width - 1;
        for (text, style) in right_parts.iter().rev() {
            let w = text.len() as u16;
            // Need at least w + 3 cells (text + separator + gap) to fit this part.
            if rx >= w + 3 + area.x {
                rx -= w;
                buf.set_string(rx, area.y, text, *style);
                rx -= 2; // separator gap
                buf.set_string(rx, area.y, "\u{2502}", bg); // │
                rx -= 1;
            } else {
                break;
            }
        }
    }
}
