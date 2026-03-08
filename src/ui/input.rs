use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthChar;

/// A multi-line text input widget with cursor, editing, and paste support.
#[derive(Debug)]
pub struct InputState {
    /// Lines of text in the input buffer.
    pub lines: Vec<String>,
    /// Cursor row (line index).
    pub cursor_row: usize,
    /// Cursor column (byte offset within the line — kept at char boundary).
    pub cursor_col: usize,
    /// Scroll offset (first visible row).
    pub scroll: usize,
    /// Whether a large paste is collapsed in the display.
    pub paste_collapsed: bool,
    /// Number of pasted lines (for display purposes).
    pub paste_line_count: usize,
}

impl InputState {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_row: 0,
            cursor_col: 0,
            scroll: 0,
            paste_collapsed: false,
            paste_line_count: 0,
        }
    }

    /// Get the full text content (joined with newlines).
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Whether the input is empty.
    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    /// Clear all content and reset cursor.
    pub fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.scroll = 0;
        self.paste_collapsed = false;
        self.paste_line_count = 0;
    }

    /// Insert a character at the cursor position.
    pub fn insert_char(&mut self, c: char) {
        self.uncollapse_paste();
        let line = &mut self.lines[self.cursor_row];
        // Find the byte index for the current cursor_col (char index).
        let byte_idx = char_to_byte(line, self.cursor_col);
        line.insert(byte_idx, c);
        self.cursor_col += 1;
    }

    /// Insert a newline at the cursor position.
    pub fn insert_newline(&mut self) {
        self.uncollapse_paste();
        let line = &self.lines[self.cursor_row];
        let byte_idx = char_to_byte(line, self.cursor_col);
        let rest = self.lines[self.cursor_row][byte_idx..].to_string();
        self.lines[self.cursor_row].truncate(byte_idx);
        self.cursor_row += 1;
        self.cursor_col = 0;
        self.lines.insert(self.cursor_row, rest);
    }

    /// Handle a paste event. If >5 lines, collapse the display.
    pub fn paste(&mut self, text: &str) {
        let paste_lines: Vec<&str> = text.lines().collect();
        let count = paste_lines.len();

        if count <= 5 {
            // Insert directly.
            for (i, pl) in paste_lines.iter().enumerate() {
                for c in pl.chars() {
                    self.insert_char(c);
                }
                if i + 1 < count {
                    self.insert_newline();
                }
            }
        } else {
            // Insert the full text but mark as collapsed.
            for (i, pl) in paste_lines.iter().enumerate() {
                for c in pl.chars() {
                    self.insert_char(c);
                }
                if i + 1 < count {
                    self.insert_newline();
                }
            }
            self.paste_collapsed = true;
            self.paste_line_count = count;
        }
    }

    /// Delete the character before the cursor.
    pub fn delete_backward(&mut self) {
        self.uncollapse_paste();
        if self.cursor_col > 0 {
            let line = &mut self.lines[self.cursor_row];
            let byte_idx = char_to_byte(line, self.cursor_col);
            let prev_byte_idx = char_to_byte(line, self.cursor_col - 1);
            line.drain(prev_byte_idx..byte_idx);
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            // Merge with previous line.
            let current = self.lines.remove(self.cursor_row);
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].chars().count();
            self.lines[self.cursor_row].push_str(&current);
        }
    }

    /// Delete the character at the cursor.
    pub fn delete_forward(&mut self) {
        self.uncollapse_paste();
        let line_len = self.lines[self.cursor_row].chars().count();
        if self.cursor_col < line_len {
            let line = &mut self.lines[self.cursor_row];
            let byte_idx = char_to_byte(line, self.cursor_col);
            let next_byte_idx = char_to_byte(line, self.cursor_col + 1);
            line.drain(byte_idx..next_byte_idx);
        } else if self.cursor_row + 1 < self.lines.len() {
            // Merge next line into current.
            let next = self.lines.remove(self.cursor_row + 1);
            self.lines[self.cursor_row].push_str(&next);
        }
    }

    /// Delete the word before the cursor (Ctrl+W).
    pub fn delete_word_backward(&mut self) {
        self.uncollapse_paste();
        if self.cursor_col == 0 {
            return;
        }
        let line = &self.lines[self.cursor_row];
        let chars: Vec<char> = line.chars().collect();
        let mut new_col = self.cursor_col;
        // Skip trailing spaces.
        while new_col > 0 && chars[new_col - 1] == ' ' {
            new_col -= 1;
        }
        // Skip word characters.
        while new_col > 0 && chars[new_col - 1] != ' ' {
            new_col -= 1;
        }
        let start_byte = char_to_byte(line, new_col);
        let end_byte = char_to_byte(line, self.cursor_col);
        self.lines[self.cursor_row].drain(start_byte..end_byte);
        self.cursor_col = new_col;
    }

    /// Delete to start of line (Ctrl+U).
    pub fn delete_to_line_start(&mut self) {
        self.uncollapse_paste();
        if self.cursor_col == 0 {
            return;
        }
        let byte_idx = char_to_byte(&self.lines[self.cursor_row], self.cursor_col);
        self.lines[self.cursor_row].drain(..byte_idx);
        self.cursor_col = 0;
    }

    /// Move cursor left.
    pub fn cursor_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].chars().count();
        }
    }

    /// Move cursor right.
    pub fn cursor_right(&mut self) {
        let line_len = self.lines[self.cursor_row].chars().count();
        if self.cursor_col < line_len {
            self.cursor_col += 1;
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }

    /// Move cursor up.
    pub fn cursor_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.clamp_col();
        }
    }

    /// Move cursor down.
    pub fn cursor_down(&mut self) {
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.clamp_col();
        }
    }

    /// Move cursor to start of line.
    pub fn cursor_home(&mut self) {
        self.cursor_col = 0;
    }

    /// Move cursor to end of line.
    pub fn cursor_end(&mut self) {
        self.cursor_col = self.lines[self.cursor_row].chars().count();
    }

    /// Ensure scroll keeps the cursor visible given a viewport height.
    pub fn ensure_visible(&mut self, viewport_height: usize) {
        if viewport_height == 0 {
            return;
        }
        if self.cursor_row < self.scroll {
            self.scroll = self.cursor_row;
        } else if self.cursor_row >= self.scroll + viewport_height {
            self.scroll = self.cursor_row - viewport_height + 1;
        }
    }

    /// Scroll the input viewport down by one line without moving the cursor.
    pub fn scroll_down(&mut self) {
        let max_scroll = self.lines.len().saturating_sub(1);
        if self.scroll < max_scroll {
            self.scroll += 1;
        }
    }

    /// Scroll the input viewport up by one line without moving the cursor.
    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    fn clamp_col(&mut self) {
        let line_len = self.lines[self.cursor_row].chars().count();
        if self.cursor_col > line_len {
            self.cursor_col = line_len;
        }
    }

    fn uncollapse_paste(&mut self) {
        if self.paste_collapsed {
            self.paste_collapsed = false;
            self.paste_line_count = 0;
        }
    }
}

/// Convert a char index to a byte index within a string.
fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
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

/// Renders the input area.
pub struct InputWidget<'a> {
    pub state: &'a InputState,
    pub active: bool,
}

impl Widget for InputWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let prompt = "> ";
        let prompt_width = prompt.len() as u16;

        if self.state.paste_collapsed {
            // Show collapsed paste indicator.
            let text = format!("{prompt}[Pasted ~{} lines]", self.state.paste_line_count);
            let style = Style::default().fg(Color::Rgb(150, 150, 150));
            buf.set_string(area.x, area.y, &text, style);
            return;
        }

        let visible_height = area.height as usize;
        let text_width = area.width.saturating_sub(prompt_width) as usize;

        for (vi, line_idx) in (self.state.scroll..).enumerate() {
            if vi >= visible_height {
                break;
            }
            if line_idx >= self.state.lines.len() {
                break;
            }

            let y = area.y + vi as u16;
            let line = &self.state.lines[line_idx];

            // Draw prompt on first visible line, continuation indent on subsequent lines.
            let prefix = if line_idx == 0 { prompt } else { "  " };
            let prefix_style = Style::default().fg(Color::Rgb(100, 100, 120));
            buf.set_string(area.x, y, prefix, prefix_style);

            // Draw the text content (truncate by display width, not char count).
            let display = truncate_to_width(line, text_width);
            buf.set_string(area.x + prompt_width, y, &display, Style::default());

            // Draw cursor (position based on display width of text before cursor).
            if self.active && line_idx == self.state.cursor_row {
                let cursor_display_offset: usize = line
                    .chars()
                    .take(self.state.cursor_col)
                    .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
                    .sum();
                let cursor_x = area.x + prompt_width + cursor_display_offset as u16;
                if cursor_x < area.x + area.width {
                    let cursor_style = Style::default()
                        .bg(Color::Rgb(200, 200, 200))
                        .fg(Color::Rgb(0, 0, 0));
                    let ch = line.chars().nth(self.state.cursor_col).unwrap_or(' ');
                    buf.set_string(cursor_x, y, ch.to_string(), cursor_style);
                }
            }
        }
    }
}
