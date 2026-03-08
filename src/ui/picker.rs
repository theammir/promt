/// Fuzzy picker overlay for model/provider selection and history browsing.
/// Used by C-x m to pick a provider/model pair. Uses nucleo for fuzzy matching.
use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthChar;

/// State for the fuzzy picker overlay.
pub struct PickerState {
    pub query: String,
    pub items: Vec<String>,
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub visible: bool,
    /// The nucleo matcher instance (reused across queries).
    matcher: Matcher,
}

impl PickerState {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            items: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            visible: false,
            matcher: Matcher::new(Config::DEFAULT),
        }
    }

    pub fn open(&mut self, items: Vec<String>) {
        self.items = items;
        self.query.clear();
        self.selected = 0;
        self.visible = true;
        self.update_filter();
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.query.clear();
    }

    pub fn update_filter(&mut self) {
        if self.query.is_empty() {
            self.filtered = (0..self.items.len()).collect();
        } else {
            let pattern = Pattern::parse(&self.query, CaseMatching::Ignore, Normalization::Smart);

            // Score each item with nucleo, collecting (index, score) for matches.
            let mut scored: Vec<(usize, u32)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, item)| {
                    let haystack = nucleo::Utf32String::from(item.as_str());
                    pattern
                        .score(haystack.slice(..), &mut self.matcher)
                        .map(|score| (i, score))
                })
                .collect();

            // Sort by score descending (best matches first).
            scored.sort_by(|a, b| b.1.cmp(&a.1));
            self.filtered = scored.into_iter().map(|(i, _)| i).collect();
        }
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
    }

    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn select_next(&mut self) {
        if !self.filtered.is_empty() && self.selected < self.filtered.len() - 1 {
            self.selected += 1;
        }
    }

    pub fn selected_item(&self) -> Option<&str> {
        self.filtered
            .get(self.selected)
            .and_then(|&i| self.items.get(i))
            .map(|s| s.as_str())
    }
}

/// Widget for rendering the picker overlay.
pub struct PickerWidget<'a> {
    pub state: &'a PickerState,
}

impl Widget for PickerWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.state.visible || area.height < 4 || area.width < 10 {
            return;
        }

        // The picker draws as an overlay centered in the area.
        let picker_width = area.width.min(50).max(20);
        let picker_height = area.height.min(16).max(4);
        let x = area.x + (area.width.saturating_sub(picker_width)) / 2;
        let y = area.y + (area.height.saturating_sub(picker_height)) / 2;

        let bg = Style::default()
            .bg(Color::Rgb(25, 25, 35))
            .fg(Color::Rgb(200, 200, 210));
        let border_style = Style::default()
            .fg(Color::Rgb(80, 80, 100))
            .bg(Color::Rgb(25, 25, 35));
        let selected_style = Style::default()
            .bg(Color::Rgb(50, 50, 70))
            .fg(Color::Rgb(255, 255, 255))
            .add_modifier(Modifier::BOLD);
        let query_style = Style::default()
            .bg(Color::Rgb(35, 35, 50))
            .fg(Color::Rgb(220, 220, 240));
        let prompt_style = Style::default()
            .bg(Color::Rgb(35, 35, 50))
            .fg(Color::Rgb(100, 180, 255));

        // Clear the picker area.
        for dy in 0..picker_height {
            for dx in 0..picker_width {
                let cx = x + dx;
                let cy = y + dy;
                if cy < area.y + area.height && cx < area.x + area.width {
                    buf.cell_mut((cx, cy)).map(|cell| {
                        cell.set_char(' ');
                        cell.set_style(bg);
                    });
                }
            }
        }

        // Top border.
        let title = " Select Model ";
        let border_len = picker_width as usize;
        let mut border_chars: Vec<char> = std::iter::repeat('\u{2500}').take(border_len).collect();
        let title_start = border_len.saturating_sub(title.chars().count()) / 2;
        for (i, ch) in title.chars().enumerate() {
            if title_start + i < border_chars.len() {
                border_chars[title_start + i] = ch;
            }
        }
        let top_border: String = border_chars.into_iter().collect();
        buf.set_string(x, y, &top_border, border_style);

        // Query input line.
        let query_y = y + 1;
        if query_y < area.y + area.height {
            // Fill query line background.
            for dx in 0..picker_width {
                buf.cell_mut((x + dx, query_y)).map(|cell| {
                    cell.set_char(' ');
                    cell.set_style(query_style);
                });
            }
            buf.set_string(x + 1, query_y, "> ", prompt_style);
            let max_query_w = (picker_width as usize).saturating_sub(4);
            let display_query = truncate_to_width(&self.state.query, max_query_w);
            buf.set_string(x + 3, query_y, &display_query, query_style);
        }

        // Separator.
        let sep_y = y + 2;
        if sep_y < area.y + area.height {
            let sep: String = std::iter::repeat('\u{2500}')
                .take(picker_width as usize)
                .collect();
            buf.set_string(x, sep_y, &sep, border_style);
        }

        // Items list.
        let list_start_y = y + 3;
        let list_height = picker_height.saturating_sub(4) as usize; // 3 for top + query + sep, 1 for bottom border

        // Compute scroll offset so selected item is visible.
        let scroll = if self.state.selected >= list_height {
            self.state.selected - list_height + 1
        } else {
            0
        };

        for (vi, fi) in (scroll..).enumerate() {
            if vi >= list_height {
                break;
            }
            let item_y = list_start_y + vi as u16;
            if item_y >= area.y + area.height {
                break;
            }
            if fi >= self.state.filtered.len() {
                break;
            }

            let item_idx = self.state.filtered[fi];
            let item = &self.state.items[item_idx];
            let is_selected = fi == self.state.selected;

            let style = if is_selected { selected_style } else { bg };

            // Fill line background.
            for dx in 0..picker_width {
                buf.cell_mut((x + dx, item_y)).map(|cell| {
                    cell.set_char(' ');
                    cell.set_style(style);
                });
            }

            // Draw selection indicator.
            let prefix = if is_selected { "\u{25b8} " } else { "  " };
            buf.set_string(x + 1, item_y, prefix, style);

            // Draw item text (truncate to fit).
            let max_w = (picker_width as usize).saturating_sub(4);
            let display = truncate_to_width(item, max_w);
            buf.set_string(x + 3, item_y, &display, style);
        }

        // Bottom border.
        let bottom_y = y + picker_height - 1;
        if bottom_y < area.y + area.height {
            let count_text = format!(" {}/{} ", self.state.filtered.len(), self.state.items.len());
            let bottom_border: String = std::iter::repeat('\u{2500}')
                .take(picker_width as usize)
                .collect();
            buf.set_string(x, bottom_y, &bottom_border, border_style);
            // Overlay count on the right (ASCII-only, so len() == display width).
            let count_w = count_text.len() as u16;
            if picker_width > count_w + 1 {
                let count_x = x + picker_width - count_w - 1;
                buf.set_string(count_x, bottom_y, &count_text, border_style);
            }
        }
    }
}

/// Truncate a string to fit within `max_width` display columns.
fn truncate_to_width(s: &str, max_width: usize) -> String {
    let mut width = 0;
    let mut result = String::new();
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + cw > max_width {
            break;
        }
        result.push(ch);
        width += cw;
    }
    result
}
