/// Fuzzy search overlay for searching within messages.
/// Uses nucleo for fuzzy matching against message contents.
use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthChar;

/// A matched message entry.
#[derive(Debug, Clone)]
pub struct SearchMatch {
    /// Index into the conversation messages list.
    pub message_index: usize,
    /// Preview text (first line or truncated content).
    pub preview: String,
    /// Fuzzy match score.
    pub score: u32,
}

/// State for the search overlay.
pub struct SearchState {
    /// Whether the search overlay is visible.
    pub visible: bool,
    /// The current search query.
    pub query: String,
    /// All searchable items: (message_index, preview).
    items: Vec<(usize, String)>,
    /// Matched results, sorted by score descending.
    pub matches: Vec<SearchMatch>,
    /// Index into `matches` for the currently selected result.
    pub selected: usize,
    /// The nucleo matcher instance (reused across queries).
    matcher: Matcher,
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            visible: false,
            query: String::new(),
            items: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            matcher: Matcher::new(Config::DEFAULT),
        }
    }

    /// Open the search overlay, populating searchable items from conversation messages.
    /// Each message gets a preview string derived from its content.
    pub fn open(&mut self, messages: &[(usize, String)]) {
        self.visible = true;
        self.query.clear();
        self.items = messages.to_vec();
        self.selected = 0;
        // Initially show all items (no filter).
        self.matches = self
            .items
            .iter()
            .enumerate()
            .map(|(_, (msg_idx, preview))| SearchMatch {
                message_index: *msg_idx,
                preview: preview.clone(),
                score: 0,
            })
            .collect();
    }

    /// Close the search overlay.
    pub fn close(&mut self) {
        self.visible = false;
        self.query.clear();
        self.items.clear();
        self.matches.clear();
    }

    /// Update the matches based on the current query.
    pub fn update_filter(&mut self) {
        if self.query.is_empty() {
            // Show all items.
            self.matches = self
                .items
                .iter()
                .map(|(msg_idx, preview)| SearchMatch {
                    message_index: *msg_idx,
                    preview: preview.clone(),
                    score: 0,
                })
                .collect();
        } else {
            let pattern = Pattern::parse(&self.query, CaseMatching::Ignore, Normalization::Smart);

            self.matches.clear();
            for (msg_idx, preview) in &self.items {
                let haystack = nucleo::Utf32String::from(preview.as_str());
                if let Some(score) = pattern.score(haystack.slice(..), &mut self.matcher) {
                    self.matches.push(SearchMatch {
                        message_index: *msg_idx,
                        preview: preview.clone(),
                        score,
                    });
                }
            }

            // Sort by score descending.
            self.matches.sort_by(|a, b| b.score.cmp(&a.score));
        }

        // Clamp selected.
        if self.selected >= self.matches.len() {
            self.selected = self.matches.len().saturating_sub(1);
        }
    }

    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn select_next(&mut self) {
        if !self.matches.is_empty() && self.selected < self.matches.len() - 1 {
            self.selected += 1;
        }
    }

    /// Get the message index of the currently selected match.
    pub fn selected_message_index(&self) -> Option<usize> {
        self.matches.get(self.selected).map(|m| m.message_index)
    }
}

/// Widget for rendering the search overlay.
pub struct SearchWidget<'a> {
    pub state: &'a SearchState,
}

impl Widget for SearchWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.state.visible || area.height < 4 || area.width < 10 {
            return;
        }

        let overlay_width = area.width.min(60).max(20);
        let overlay_height = area.height.min(16).max(4);
        let x = area.x + (area.width.saturating_sub(overlay_width)) / 2;
        let y = area.y + (area.height.saturating_sub(overlay_height)) / 2;

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

        // Clear overlay area.
        for dy in 0..overlay_height {
            for dx in 0..overlay_width {
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
        let title = " Search Messages ";
        let border_len = overlay_width as usize;
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
            for dx in 0..overlay_width {
                buf.cell_mut((x + dx, query_y)).map(|cell| {
                    cell.set_char(' ');
                    cell.set_style(query_style);
                });
            }
            buf.set_string(x + 1, query_y, "/ ", prompt_style);
            let max_query_w = (overlay_width as usize).saturating_sub(4);
            let display_query = truncate_to_width(&self.state.query, max_query_w);
            buf.set_string(x + 3, query_y, &display_query, query_style);
        }

        // Separator.
        let sep_y = y + 2;
        if sep_y < area.y + area.height {
            let sep: String = std::iter::repeat('\u{2500}')
                .take(overlay_width as usize)
                .collect();
            buf.set_string(x, sep_y, &sep, border_style);
        }

        // Results list.
        let list_start_y = y + 3;
        let list_height = overlay_height.saturating_sub(4) as usize;

        let scroll = if self.state.selected >= list_height {
            self.state.selected - list_height + 1
        } else {
            0
        };

        for (vi, mi) in (scroll..).enumerate() {
            if vi >= list_height {
                break;
            }
            let item_y = list_start_y + vi as u16;
            if item_y >= area.y + area.height {
                break;
            }
            if mi >= self.state.matches.len() {
                break;
            }

            let m = &self.state.matches[mi];
            let is_selected = mi == self.state.selected;
            let style = if is_selected { selected_style } else { bg };

            // Fill line bg.
            for dx in 0..overlay_width {
                buf.cell_mut((x + dx, item_y)).map(|cell| {
                    cell.set_char(' ');
                    cell.set_style(style);
                });
            }

            // Selection indicator.
            let prefix = if is_selected { "\u{25b8} " } else { "  " };
            buf.set_string(x + 1, item_y, prefix, style);

            // Truncated preview.
            let max_w = (overlay_width as usize).saturating_sub(4);
            let display = truncate_to_width(&m.preview, max_w);
            buf.set_string(x + 3, item_y, &display, style);
        }

        // Bottom border.
        let bottom_y = y + overlay_height - 1;
        if bottom_y < area.y + area.height {
            let count_text = format!(" {}/{} ", self.state.matches.len(), self.state.items.len());
            let bottom_border: String = std::iter::repeat('\u{2500}')
                .take(overlay_width as usize)
                .collect();
            buf.set_string(x, bottom_y, &bottom_border, border_style);
            let count_w = count_text.len() as u16;
            if overlay_width > count_w + 1 {
                let count_x = x + overlay_width - count_w - 1;
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
