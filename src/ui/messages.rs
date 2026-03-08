use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use crate::conversation::{Message, Role};
use crate::highlight::Highlighter;
use crate::mode::{Mode, VisualSelection};
use crate::ui::codeblock::CodeblockSelectState;
use crate::ui::markdown::{self, ConcealLevel, IncrementalParser, MarkdownBlock};

/// Colors for the halfblock left border by role.
const USER_COLOR: Color = Color::Rgb(100, 149, 237); // cornflower blue
const ASSISTANT_COLOR: Color = Color::Rgb(134, 194, 50); // yellow-green
const SYSTEM_COLOR: Color = Color::Rgb(180, 160, 80); // muted yellow

/// A rendered message ready for display (cached).
#[derive(Debug, Clone)]
pub struct RenderedMessage {
    /// The parsed markdown blocks (needed for codeblock extraction).
    pub blocks: Vec<MarkdownBlock>,
    /// Flattened lines ready for rendering.
    pub lines: Vec<Line<'static>>,
    /// The role, for halfblock color.
    pub role: Role,
    /// Incremental parser state for streaming messages.
    /// Present only while the message is being streamed to.
    pub incremental: Option<IncrementalParser>,
}

impl RenderedMessage {
    pub fn render(message: &Message, conceal: ConcealLevel, highlighter: &Highlighter) -> Self {
        let blocks = markdown::parse(&message.content, conceal, highlighter);
        let lines = markdown::blocks_to_lines(&blocks);
        Self {
            blocks,
            lines,
            role: message.role,
            incremental: None,
        }
    }

    /// Create a rendered message with incremental parsing enabled (for streaming).
    pub fn render_streaming(
        message: &Message,
        conceal: ConcealLevel,
        highlighter: &Highlighter,
    ) -> Self {
        let mut inc = IncrementalParser::new();
        inc.update(&message.content, conceal, highlighter);
        Self {
            blocks: inc.blocks().to_vec(),
            lines: inc.lines().to_vec(),
            role: message.role,
            incremental: Some(inc),
        }
    }

    /// Incrementally update this message with new content (for streaming).
    /// Falls back to full re-render if no incremental parser is present.
    pub fn update_streaming(
        &mut self,
        message: &Message,
        conceal: ConcealLevel,
        highlighter: &Highlighter,
    ) {
        if let Some(ref mut inc) = self.incremental {
            inc.update(&message.content, conceal, highlighter);
            self.blocks = inc.blocks().to_vec();
            self.lines = inc.lines().to_vec();
        } else {
            // No incremental parser — do a full re-render.
            let blocks = markdown::parse(&message.content, conceal, highlighter);
            self.lines = markdown::blocks_to_lines(&blocks);
            self.blocks = blocks;
        }
    }

    /// Finalize the message: drop the incremental parser, do a final full render.
    /// This ensures the final state is clean and fully parsed.
    pub fn finalize(
        &mut self,
        message: &Message,
        conceal: ConcealLevel,
        highlighter: &Highlighter,
    ) {
        self.incremental = None;
        let blocks = markdown::parse(&message.content, conceal, highlighter);
        self.lines = markdown::blocks_to_lines(&blocks);
        self.blocks = blocks;
    }

    /// Number of display lines this message occupies (including the role header).
    pub fn height(&self) -> usize {
        // 1 for the role header + content lines.
        1 + self.lines.len()
    }

    fn border_color(&self) -> Color {
        match self.role {
            Role::User => USER_COLOR,
            Role::Assistant => ASSISTANT_COLOR,
            Role::System => SYSTEM_COLOR,
        }
    }

    /// Compute the line ranges for each codeblock within the flattened `lines` vec.
    /// Returns a vec of `(start_line, end_line_exclusive, fence_offset)` indexed by
    /// codeblock index. The line indices are relative to `self.lines` (not including
    /// the role header). `fence_offset` is 1 when fence markers are present
    /// (ConcealLevel::Visible) and 0 otherwise; callers must add this offset to
    /// convert between `cursor_row` (0-indexed into raw code) and the flat line index.
    pub fn codeblock_line_ranges(&self) -> Vec<(usize, usize, usize)> {
        let mut ranges = Vec::new();
        let mut line_idx = 0;
        for (i, block) in self.blocks.iter().enumerate() {
            let block_lines = match block {
                MarkdownBlock::Text(text_lines) => text_lines.len(),
                MarkdownBlock::CodeBlock { highlighted, .. } => {
                    highlighted.as_ref().map_or(0, |h| h.len())
                }
            };
            if let MarkdownBlock::CodeBlock {
                code, highlighted, ..
            } = block
            {
                // Determine fence offset: if highlighted has more lines than raw code,
                // fences are present (ConcealLevel::Visible wraps with ``` lines).
                let raw_lines = code.lines().count().max(1);
                let hl_lines = highlighted.as_ref().map_or(0, |h| h.len());
                let fence_offset = if hl_lines > raw_lines { 1 } else { 0 };
                ranges.push((line_idx, line_idx + block_lines, fence_offset));
            }
            line_idx += block_lines;
            // Account for blank separator line between blocks.
            if i + 1 < self.blocks.len() {
                line_idx += 1;
            }
        }
        ranges
    }
}

/// State for the message list display.
#[derive(Debug)]
pub struct MessageListState {
    /// Rendered messages cache.
    pub rendered: Vec<RenderedMessage>,
    /// Scroll offset (index of the first visible rendered line, counting from top).
    pub scroll: usize,
    /// Currently selected message index (in Browse mode).
    pub selected: Option<usize>,
}

impl MessageListState {
    pub fn new() -> Self {
        Self {
            rendered: Vec::new(),
            scroll: 0,
            selected: None,
        }
    }

    /// Re-render all messages.
    pub fn render_all(
        &mut self,
        messages: &[Message],
        conceal: ConcealLevel,
        highlighter: &Highlighter,
    ) {
        self.rendered = messages
            .iter()
            .map(|m| RenderedMessage::render(m, conceal, highlighter))
            .collect();
    }

    /// Append a new rendered message (for when a new message arrives).
    pub fn push(&mut self, message: &Message, conceal: ConcealLevel, highlighter: &Highlighter) {
        self.rendered
            .push(RenderedMessage::render(message, conceal, highlighter));
    }

    /// Append a new message with incremental parsing enabled (for streaming).
    pub fn push_streaming(
        &mut self,
        message: &Message,
        conceal: ConcealLevel,
        highlighter: &Highlighter,
    ) {
        self.rendered.push(RenderedMessage::render_streaming(
            message,
            conceal,
            highlighter,
        ));
    }

    /// Incrementally update the last message during streaming.
    pub fn update_last(
        &mut self,
        message: &Message,
        conceal: ConcealLevel,
        highlighter: &Highlighter,
    ) {
        if let Some(last) = self.rendered.last_mut() {
            last.update_streaming(message, conceal, highlighter);
        }
    }

    /// Finalize the last message after streaming completes.
    /// Does a full re-parse to ensure correctness.
    pub fn finalize_last(
        &mut self,
        message: &Message,
        conceal: ConcealLevel,
        highlighter: &Highlighter,
    ) {
        if let Some(last) = self.rendered.last_mut() {
            last.finalize(message, conceal, highlighter);
        }
    }

    /// Total number of display lines across all messages (with gaps).
    pub fn total_lines(&self) -> usize {
        if self.rendered.is_empty() {
            return 0;
        }
        let content: usize = self.rendered.iter().map(|r| r.height()).sum();
        // One blank line between each message.
        content + self.rendered.len().saturating_sub(1)
    }

    /// Scroll to show the bottom of the message list.
    pub fn scroll_to_bottom(&mut self, viewport_height: usize) {
        let total = self.total_lines();
        if total > viewport_height {
            self.scroll = total - viewport_height;
        } else {
            self.scroll = 0;
        }
    }

    /// Scroll so that the selected message is visible.
    pub fn scroll_to_selected(&mut self, viewport_height: usize) {
        let Some(sel) = self.selected else { return };
        // Compute the starting line of the selected message.
        let mut line = 0;
        for (i, r) in self.rendered.iter().enumerate() {
            if i == sel {
                break;
            }
            line += r.height();
            if i + 1 < self.rendered.len() {
                line += 1; // separator
            }
        }
        // If the selected message's start is above the current scroll, scroll up.
        if line < self.scroll {
            self.scroll = line;
        }
        // If it's below the viewport, scroll down.
        let msg_height = self.rendered.get(sel).map(|r| r.height()).unwrap_or(1);
        let msg_end = line + msg_height;
        if msg_end > self.scroll + viewport_height {
            self.scroll = msg_end.saturating_sub(viewport_height);
        }
    }

    /// Select previous message.
    pub fn select_prev(&mut self) {
        if self.rendered.is_empty() {
            return;
        }
        self.selected = Some(match self.selected {
            Some(i) if i > 0 => i - 1,
            Some(i) => i,
            None => self.rendered.len().saturating_sub(1),
        });
    }

    /// Select next message.
    pub fn select_next(&mut self) {
        if self.rendered.is_empty() {
            return;
        }
        let max = self.rendered.len().saturating_sub(1);
        self.selected = Some(match self.selected {
            Some(i) if i < max => i + 1,
            Some(i) => i,
            None => 0,
        });
    }

    /// Select first message.
    pub fn select_first(&mut self) {
        if !self.rendered.is_empty() {
            self.selected = Some(0);
        }
    }

    /// Select last message.
    pub fn select_last(&mut self) {
        if !self.rendered.is_empty() {
            self.selected = Some(self.rendered.len() - 1);
        }
    }

    /// Clear selection (return to insert mode).
    pub fn deselect(&mut self) {
        self.selected = None;
    }

    /// Compute the starting flat line index for a given message index.
    pub fn message_start_line(&self, msg_idx: usize) -> usize {
        let mut line = 0;
        for (i, r) in self.rendered.iter().enumerate() {
            if i == msg_idx {
                break;
            }
            line += r.height();
            if i + 1 < self.rendered.len() {
                line += 1; // separator
            }
        }
        line
    }

    /// Find which message contains a given flat line index.
    pub fn message_at_line(&self, flat_line: usize) -> Option<usize> {
        let mut line = 0;
        for (i, r) in self.rendered.iter().enumerate() {
            let end = line + r.height();
            if flat_line < end {
                return Some(i);
            }
            line = end;
            if i + 1 < self.rendered.len() {
                // separator line — belongs to the gap, attribute to next message
                if flat_line == line {
                    return Some(i + 1);
                }
                line += 1;
            }
        }
        // Past the end: return last message.
        if !self.rendered.is_empty() {
            Some(self.rendered.len() - 1)
        } else {
            None
        }
    }

    /// Update `self.selected` based on the current scroll position using a
    /// sticky middle-band focus model. The current selection is kept if it
    /// still overlaps the center third of the viewport. Otherwise, the
    /// message containing the viewport midpoint is selected.
    pub fn update_focus_from_scroll(&mut self, viewport_height: usize) {
        if self.rendered.is_empty() || viewport_height == 0 {
            return;
        }

        // Check if current selection still overlaps the center band.
        let band_start = self.scroll + viewport_height / 3;
        let band_end = self.scroll + viewport_height * 2 / 3;

        if let Some(sel) = self.selected {
            let msg_start = self.message_start_line(sel);
            let msg_end = msg_start + self.rendered[sel].height();
            // Keep current selection if it overlaps the center band.
            if msg_start < band_end && msg_end > band_start {
                return;
            }
        }

        // Select the message at the viewport midpoint.
        let midpoint = self.scroll + viewport_height / 2;
        if let Some(idx) = self.message_at_line(midpoint) {
            self.selected = Some(idx);
        }
    }

    /// Compute the flat line range (start, end_exclusive, fence_offset) for a
    /// specific codeblock within a specific message. `fence_offset` is 1 when
    /// fence markers are present in the rendered lines (ConcealLevel::Visible).
    pub fn codeblock_flat_range(
        &self,
        msg_idx: usize,
        cb_idx: usize,
    ) -> Option<(usize, usize, usize)> {
        let rendered = self.rendered.get(msg_idx)?;
        let ranges = rendered.codeblock_line_ranges();
        let (local_start, local_end, fence_offset) = ranges.get(cb_idx).copied()?;
        let msg_start = self.message_start_line(msg_idx);
        // +1 for the role header line
        Some((
            msg_start + 1 + local_start,
            msg_start + 1 + local_end,
            fence_offset,
        ))
    }

    /// Scroll so that a specific cursor row within a codeblock is visible.
    /// `cursor_row` is 0-indexed within the codeblock's code lines.
    /// Keeps a 2-line margin from the viewport edges when possible.
    pub fn scroll_codeblock_cursor_into_view(
        &mut self,
        msg_idx: usize,
        cb_idx: usize,
        cursor_row: usize,
        viewport_height: usize,
    ) {
        if let Some((cb_start, _cb_end, fence_offset)) = self.codeblock_flat_range(msg_idx, cb_idx)
        {
            // cursor_row is 0-indexed into raw code lines; add fence_offset to
            // skip the opening fence marker in ConcealLevel::Visible.
            let cursor_flat = cb_start + fence_offset + cursor_row;
            let margin = 2.min(viewport_height / 4);
            let view_top = self.scroll + margin;
            let view_bot = self.scroll + viewport_height.saturating_sub(1 + margin);

            if cursor_flat <= view_top {
                self.scroll = cursor_flat.saturating_sub(margin);
            } else if cursor_flat >= view_bot {
                self.scroll = (cursor_flat + 1 + margin).saturating_sub(viewport_height);
            }
            // Clamp to valid range.
            let total = self.total_lines();
            let max_scroll = total.saturating_sub(viewport_height);
            self.scroll = self.scroll.min(max_scroll);
        }
    }
}

/// Widget for rendering the message list.
pub struct MessageListWidget<'a> {
    pub state: &'a MessageListState,
    pub mode: Mode,
    pub codeblock_select: Option<&'a CodeblockSelectState>,
    pub visual_selection: Option<&'a VisualSelection>,
}

/// Background color for selected codeblock.
const CODEBLOCK_SELECT_BG: Color = Color::Rgb(35, 45, 55);
/// Background color for visual selection.
const VISUAL_SELECT_BG: Color = Color::Rgb(60, 50, 30);
/// Style for the codeblock cursor cell (inverted).
const CURSOR_STYLE: Style = Style::new()
    .fg(Color::Rgb(25, 25, 35))
    .bg(Color::Rgb(220, 220, 230));
/// Style for the visual active end (cursor position in visual mode).
const VISUAL_ACTIVE_END_STYLE: Style = Style::new()
    .fg(Color::Rgb(25, 25, 35))
    .bg(Color::Rgb(255, 180, 80));

impl Widget for MessageListWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 3 {
            return;
        }

        let halfblock = "\u{258c}"; // ▌
        let content_x = area.x + 2; // halfblock + space
        let _content_width = area.width.saturating_sub(2);

        // Compute codeblock line ranges and visual highlight info for the selected message.
        let selected_msg_idx = self.state.selected;
        let cb_ranges: Vec<(usize, usize, usize)> = selected_msg_idx
            .and_then(|idx| self.state.rendered.get(idx))
            .map(|r| r.codeblock_line_ranges())
            .unwrap_or_default();
        let selected_cb_idx = self.codeblock_select.map(|cs| cs.selected);

        // Build a flat list of (line, color, is_selected_msg, is_header, content_line_idx, msg_idx) tuples.
        struct FlatLine {
            line: Line<'static>,
            color: Color,
            is_selected_msg: bool,
            /// Line index within the message's content lines (not counting header).
            content_line_idx: Option<usize>,
        }

        let mut flat: Vec<FlatLine> = Vec::new();

        for (msg_idx, rendered) in self.state.rendered.iter().enumerate() {
            let color = rendered.border_color();
            let selected = self.state.selected == Some(msg_idx);

            // Role header line.
            let role_label = match rendered.role {
                Role::User => "You",
                Role::Assistant => "Assistant",
                Role::System => "System",
            };
            let header = Line::from(Span::styled(
                role_label.to_string(),
                Style::default()
                    .fg(color)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ));
            flat.push(FlatLine {
                line: header,
                color,
                is_selected_msg: selected,
                content_line_idx: None,
            });

            // Content lines.
            for (li, line) in rendered.lines.iter().enumerate() {
                flat.push(FlatLine {
                    line: line.clone(),
                    color,
                    is_selected_msg: selected,
                    content_line_idx: Some(li),
                });
            }

            // Blank separator between messages.
            if msg_idx + 1 < self.state.rendered.len() {
                flat.push(FlatLine {
                    line: Line::default(),
                    color,
                    is_selected_msg: false,
                    content_line_idx: None,
                });
            }
        }

        // Render visible lines based on scroll.
        let visible_height = area.height as usize;
        for (vi, idx) in (self.state.scroll..).enumerate() {
            if vi >= visible_height {
                break;
            }
            if idx >= flat.len() {
                break;
            }

            let fl = &flat[idx];
            let y = area.y + vi as u16;

            // Draw halfblock border.
            let border_style = if fl.is_selected_msg {
                Style::default()
                    .fg(fl.color)
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                Style::default().fg(fl.color)
            };
            buf.set_string(area.x, y, halfblock, border_style);

            // Determine background highlight.
            // bg_color: base background for the whole line (selected msg, codeblock, etc.)
            // visual_cols: optional (start_col, end_col_inclusive) for characterwise visual highlight
            let (bg, visual_cols): (Option<Color>, Option<(usize, usize)>) = if fl.is_selected_msg {
                // Check if this line is within a highlighted codeblock or visual selection.
                let mut line_bg = Some(Color::Rgb(40, 40, 50)); // Default selected message bg.
                let mut vcols: Option<(usize, usize)> = None;

                if let Some(cli) = fl.content_line_idx {
                    if let Some(cb_idx) = selected_cb_idx {
                        if let Some(&(start, end, fence_offset)) = cb_ranges.get(cb_idx) {
                            if cli >= start && cli < end {
                                // This line is in the selected codeblock.
                                line_bg = Some(CODEBLOCK_SELECT_BG);

                                // Map flat line index to code line (skip fence markers).
                                let code_start = start + fence_offset;
                                let code_end = end - fence_offset;

                                // Check for visual selection highlight.
                                if cli >= code_start && cli < code_end {
                                    let code_line = cli - code_start;
                                    if let Some(vs) = self.visual_selection {
                                        if vs.block_index == cb_idx {
                                            match self.mode {
                                                Mode::VisualLine => {
                                                    let (sr, er) = vs.row_range();
                                                    if code_line >= sr && code_line <= er {
                                                        line_bg = Some(VISUAL_SELECT_BG);
                                                    }
                                                }
                                                Mode::Visual => {
                                                    let (s, e) = vs.ordered();
                                                    let (sr, er) = (s.row, e.row);
                                                    if code_line >= sr && code_line <= er {
                                                        if sr == er {
                                                            // Single-line: highlight from s.col to e.col
                                                            vcols = Some((s.col, e.col));
                                                        } else if code_line == sr {
                                                            // First line: from s.col to end of line
                                                            vcols = Some((s.col, usize::MAX));
                                                        } else if code_line == er {
                                                            // Last line: from start to e.col
                                                            vcols = Some((0, e.col));
                                                        } else {
                                                            // Middle line: full line
                                                            line_bg = Some(VISUAL_SELECT_BG);
                                                        }
                                                    }
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                (line_bg, vcols)
            } else {
                (None, None)
            };

            if let Some(bg_color) = bg {
                let bg_style = Style::default().bg(bg_color);
                for x in area.x..area.x + area.width {
                    buf.cell_mut((x, y)).map(|cell| {
                        cell.set_style(bg_style);
                    });
                }
            }

            // Draw the content, tracking character positions for visual column highlighting.
            let mut x = content_x;
            let mut char_col: usize = 0;
            for span in &fl.line.spans {
                let remaining = (area.x + area.width).saturating_sub(x) as usize;
                if remaining == 0 {
                    break;
                }
                let text = truncate_to_width(&span.content, remaining);
                let w = UnicodeWidthStr::width(text.as_str()) as u16;
                buf.set_string(x, y, &text, span.style);

                // Apply characterwise visual highlight over specific columns.
                if let Some((sc, ec)) = visual_cols {
                    let visual_style = Style::default().bg(VISUAL_SELECT_BG);
                    for ch in text.chars() {
                        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                        if char_col >= sc && char_col <= ec {
                            let cx = content_x + char_col as u16;
                            if cx < area.x + area.width {
                                for dx in 0..cw as u16 {
                                    buf.cell_mut((cx + dx, y)).map(|cell| {
                                        cell.set_style(visual_style);
                                    });
                                }
                            }
                        }
                        char_col += 1;
                    }
                } else {
                    char_col += text.chars().count();
                }

                x += w;
            }

            // Render codeblock cursor or visual active end.
            if fl.is_selected_msg {
                if let Some(cli) = fl.content_line_idx {
                    if let Some(cb_idx) = selected_cb_idx {
                        if let Some(&(start, end, fence_offset)) = cb_ranges.get(cb_idx) {
                            if cli >= start && cli < end {
                                // Map flat line index to code line (skip fence markers).
                                let code_start = start + fence_offset;
                                let code_end = end - fence_offset;

                                if cli >= code_start && cli < code_end {
                                    let code_line = cli - code_start;

                                    // Visual active end (cursor in visual mode).
                                    if let Some(vs) = self.visual_selection {
                                        if vs.block_index == cb_idx && code_line == vs.cursor.row {
                                            let cx = content_x + vs.cursor.col as u16;
                                            if cx < area.x + area.width {
                                                buf.cell_mut((cx, y)).map(|cell| {
                                                    cell.set_style(VISUAL_ACTIVE_END_STYLE);
                                                });
                                            }
                                        }
                                    } else if self.mode == Mode::CodeblockSelect {
                                        // Codeblock cursor (only in CodeblockSelect, not visual modes).
                                        if let Some(cs) = self.codeblock_select {
                                            if code_line == cs.cursor.row {
                                                let cx = content_x + cs.cursor.col as u16;
                                                if cx < area.x + area.width {
                                                    buf.cell_mut((cx, y)).map(|cell| {
                                                        cell.set_style(CURSOR_STYLE);
                                                    });
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Truncate a string to fit within `max_width` display columns.
/// Respects multi-column characters (CJK, emoji, etc).
fn truncate_to_width(s: &str, max_width: usize) -> String {
    use unicode_width::UnicodeWidthChar;
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
