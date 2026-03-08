use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::highlight::Highlighter;

/// Conceal level for markdown rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConcealLevel {
    /// Level 0: Syntax highlighted but markdown chars visible (dimmed).
    Visible = 0,
    /// Level 1: Concealed — markers hidden, content styled.
    Concealed = 1,
}

impl ConcealLevel {
    pub fn toggle(self) -> Self {
        match self {
            ConcealLevel::Visible => ConcealLevel::Concealed,
            ConcealLevel::Concealed => ConcealLevel::Visible,
        }
    }

    pub fn from_u8(v: u8) -> Self {
        if v == 0 {
            ConcealLevel::Visible
        } else {
            ConcealLevel::Concealed
        }
    }
}

/// A parsed block of markdown content, ready for rendering.
#[derive(Debug, Clone)]
pub enum MarkdownBlock {
    /// A block of styled text lines (paragraphs, headers, lists, etc).
    Text(Vec<Line<'static>>),
    /// A fenced code block with language and raw content.
    CodeBlock {
        language: String,
        code: String,
        /// Pre-highlighted lines (lazily filled).
        highlighted: Option<Vec<Line<'static>>>,
    },
}

/// Style constants for markdown rendering.
mod styles {
    use super::*;

    pub const DIM: Style = Style::new().add_modifier(Modifier::DIM);
    pub const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
    pub const ITALIC: Style = Style::new().add_modifier(Modifier::ITALIC);
    pub const CODE_INLINE: Style = Style::new().fg(Color::Rgb(180, 180, 200));
    pub const HEADING: Style = Style::new()
        .fg(Color::Rgb(130, 170, 255))
        .add_modifier(Modifier::BOLD);
    pub const LINK: Style = Style::new()
        .fg(Color::Rgb(100, 180, 255))
        .add_modifier(Modifier::UNDERLINED);
    pub const BLOCKQUOTE: Style = Style::new()
        .fg(Color::Rgb(150, 150, 150))
        .add_modifier(Modifier::ITALIC);
    pub const LIST_BULLET: Style = Style::new().fg(Color::Rgb(200, 160, 80));
}

/// Incremental markdown parser that caches stable (complete) blocks and only
/// re-parses the tail of the content that changed.
///
/// During streaming, tokens are appended to the content string. Rather than
/// re-parsing the entire document, we:
/// 1. Keep a cache of "stable" blocks — blocks whose source text hasn't changed.
/// 2. Track the byte offset in the source where stable content ends.
/// 3. On each update, only re-parse from `stable_offset` onward.
/// 4. After parsing the tail, any fully-closed blocks become the new stable blocks.
#[derive(Debug, Clone)]
pub struct IncrementalParser {
    /// Blocks that are fully parsed and won't change.
    stable_blocks: Vec<MarkdownBlock>,
    /// Flattened lines from stable blocks (cached to avoid re-flattening).
    stable_lines: Vec<Line<'static>>,
    /// Byte offset in the source text up to which blocks are stable.
    /// Everything before this offset produced `stable_blocks`.
    stable_offset: usize,
    /// The flattened lines cache (stable + tail combined).
    cached_lines: Vec<Line<'static>>,
    /// All blocks (stable + tail), kept for codeblock extraction.
    cached_blocks: Vec<MarkdownBlock>,
}

impl IncrementalParser {
    pub fn new() -> Self {
        Self {
            stable_blocks: Vec::new(),
            stable_lines: Vec::new(),
            stable_offset: 0,
            cached_lines: Vec::new(),
            cached_blocks: Vec::new(),
        }
    }

    /// Update with new content. Only re-parses the changed tail.
    pub fn update(&mut self, full_text: &str, conceal: ConcealLevel, highlighter: &Highlighter) {
        // If the text shrank or changed before the stable offset, do a full re-parse.
        if full_text.len() < self.stable_offset {
            self.invalidate();
        }

        // Parse only the tail (from stable_offset onward).
        let tail = &full_text[self.stable_offset..];

        // Parse the tail into blocks.
        let tail_blocks = parse(tail, conceal, highlighter);

        // Determine which tail blocks are "complete" (can be promoted to stable).
        let (new_stable, new_offset, remaining_blocks) =
            find_stable_boundary(tail, self.stable_offset, &tail_blocks, conceal, highlighter);

        // Promote newly stable blocks and cache their lines.
        if !new_stable.is_empty() {
            // If there were already stable blocks, add a separator before the new ones.
            for (i, block) in new_stable.iter().enumerate() {
                if !self.stable_blocks.is_empty() || i > 0 {
                    self.stable_lines.push(Line::default());
                }
                match block {
                    MarkdownBlock::Text(text_lines) => {
                        self.stable_lines.extend(text_lines.iter().cloned());
                    }
                    MarkdownBlock::CodeBlock { highlighted, .. } => {
                        if let Some(hl) = highlighted {
                            self.stable_lines.extend(hl.iter().cloned());
                        }
                    }
                }
            }
            self.stable_blocks.extend(new_stable);
            self.stable_offset = new_offset;
        }

        // Build the full block list: stable + remaining unstable tail blocks.
        self.cached_blocks = self.stable_blocks.clone();
        self.cached_blocks.extend(remaining_blocks.clone());

        // Build the flattened lines: stable lines + separator + remaining block lines.
        self.cached_lines = self.stable_lines.clone();
        for (i, block) in remaining_blocks.iter().enumerate() {
            if !self.stable_blocks.is_empty() || i > 0 {
                self.cached_lines.push(Line::default());
            }
            match block {
                MarkdownBlock::Text(text_lines) => {
                    self.cached_lines.extend(text_lines.iter().cloned());
                }
                MarkdownBlock::CodeBlock { highlighted, .. } => {
                    if let Some(hl) = highlighted {
                        self.cached_lines.extend(hl.iter().cloned());
                    }
                }
            }
        }
    }

    /// Get the current rendered lines.
    pub fn lines(&self) -> &[Line<'static>] {
        &self.cached_lines
    }

    /// Get the current block list.
    pub fn blocks(&self) -> &[MarkdownBlock] {
        &self.cached_blocks
    }

    /// Invalidate all cached state (e.g., on conceal level change).
    pub fn invalidate(&mut self) {
        self.stable_blocks.clear();
        self.stable_lines.clear();
        self.stable_offset = 0;
        self.cached_lines.clear();
        self.cached_blocks.clear();
    }
}

/// Find the boundary between stable (complete) and unstable blocks in the tail text.
/// Uses pulldown-cmark's `into_offset_iter()` to track where top-level blocks end.
///
/// Returns: (newly_stable_blocks, new_stable_byte_offset, remaining_unstable_blocks)
fn find_stable_boundary(
    tail: &str,
    base_offset: usize,
    tail_blocks: &[MarkdownBlock],
    _conceal: ConcealLevel,
    _highlighter: &Highlighter,
) -> (Vec<MarkdownBlock>, usize, Vec<MarkdownBlock>) {
    if tail_blocks.is_empty() {
        return (Vec::new(), base_offset, Vec::new());
    }

    // Use pulldown-cmark's offset iterator to find where each top-level block ends.
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);

    let parser = Parser::new_ext(tail, opts);
    let offset_iter = parser.into_offset_iter();

    // Track end offsets of top-level block-ending events.
    let mut block_end_offsets: Vec<usize> = Vec::new();
    let mut depth: usize = 0;

    for (event, range) in offset_iter {
        match &event {
            Event::Start(
                Tag::Paragraph
                | Tag::CodeBlock(_)
                | Tag::Heading { .. }
                | Tag::BlockQuote(_)
                | Tag::List(_),
            ) => {
                depth += 1;
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::CodeBlock
                | TagEnd::Heading(_)
                | TagEnd::BlockQuote(_)
                | TagEnd::List(_),
            ) => {
                if depth > 0 {
                    depth -= 1;
                }
                if depth == 0 {
                    block_end_offsets.push(range.end);
                }
            }
            _ => {}
        }
    }

    // The number of block_end_offsets should correspond to the number of tail_blocks.
    // All blocks except the last are stable (they have a following block, so they're complete).
    // If there's only one block, it's unstable (still being streamed to).
    if tail_blocks.len() <= 1 || block_end_offsets.is_empty() {
        // Nothing to promote — everything is unstable.
        return (Vec::new(), base_offset, tail_blocks.to_vec());
    }

    // Promote all blocks except the last one to stable.
    let stable_count = tail_blocks.len() - 1;
    let new_stable: Vec<MarkdownBlock> = tail_blocks[..stable_count].to_vec();
    let remaining: Vec<MarkdownBlock> = tail_blocks[stable_count..].to_vec();

    // The new stable offset is base_offset + end of the second-to-last block.
    let new_offset = if stable_count <= block_end_offsets.len() {
        base_offset + block_end_offsets[stable_count - 1]
    } else {
        base_offset
    };

    (new_stable, new_offset, remaining)
}

/// Parse markdown text into blocks for rendering.
pub fn parse(text: &str, conceal: ConcealLevel, highlighter: &Highlighter) -> Vec<MarkdownBlock> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);

    let parser = Parser::new_ext(text, opts);
    let mut blocks: Vec<MarkdownBlock> = Vec::new();

    // State tracking.
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut current_lines: Vec<Line<'static>> = Vec::new();
    let mut style_stack: Vec<Style> = vec![Style::default()];
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_content = String::new();
    let mut _in_heading = false;
    let mut list_depth: usize = 0;

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::CodeBlock(kind) => {
                    // Flush any pending text.
                    flush_line(&mut current_spans, &mut current_lines);
                    flush_block(&mut current_lines, &mut blocks);

                    in_code_block = true;
                    code_content.clear();
                    code_lang = match kind {
                        CodeBlockKind::Fenced(lang) => lang.to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                }
                Tag::Emphasis => {
                    style_stack.push(styles::ITALIC);
                }
                Tag::Strong => {
                    style_stack.push(styles::BOLD);
                }
                Tag::Heading { .. } => {
                    _in_heading = true;
                    style_stack.push(styles::HEADING);
                }
                Tag::BlockQuote(_) => {
                    style_stack.push(styles::BLOCKQUOTE);
                }
                Tag::Link { .. } => {
                    style_stack.push(styles::LINK);
                }
                Tag::List(_) => {
                    list_depth += 1;
                }
                Tag::Item => {
                    flush_line(&mut current_spans, &mut current_lines);
                    let indent = "  ".repeat(list_depth.saturating_sub(1));
                    let bullet = match conceal {
                        ConcealLevel::Concealed => format!("{indent}\u{2022} "),
                        ConcealLevel::Visible => format!("{indent}- "),
                    };
                    current_spans.push(Span::styled(bullet, styles::LIST_BULLET));
                }
                Tag::Paragraph => {}
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::CodeBlock => {
                    in_code_block = false;
                    let mut block = MarkdownBlock::CodeBlock {
                        language: code_lang.clone(),
                        code: code_content.clone(),
                        highlighted: None,
                    };
                    // Pre-highlight the code block.
                    if let MarkdownBlock::CodeBlock {
                        ref language,
                        ref code,
                        ref mut highlighted,
                    } = block
                    {
                        let hl = highlighter.highlight(code, language);
                        let lines: Vec<Line<'static>> = hl
                            .into_iter()
                            .map(|frags| {
                                Line::from(
                                    frags
                                        .into_iter()
                                        .map(|f| Span::styled(f.text, f.style))
                                        .collect::<Vec<_>>(),
                                )
                            })
                            .collect();
                        *highlighted = Some(lines);
                    }

                    if conceal == ConcealLevel::Visible {
                        // Show fence markers dimmed.
                        let fence_open = format!("```{code_lang}");
                        let mut fence_lines =
                            vec![Line::from(Span::styled(fence_open, styles::DIM))];
                        if let MarkdownBlock::CodeBlock {
                            highlighted: Some(ref hl),
                            ..
                        } = block
                        {
                            fence_lines.extend(hl.iter().cloned());
                        }
                        fence_lines.push(Line::from(Span::styled("```", styles::DIM)));
                        // Replace the highlighted with fence-wrapped version.
                        if let MarkdownBlock::CodeBlock {
                            ref mut highlighted,
                            ..
                        } = block
                        {
                            *highlighted = Some(fence_lines);
                        }
                    }

                    blocks.push(block);
                }
                TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Heading(_)
                | TagEnd::BlockQuote(_)
                | TagEnd::Link => {
                    style_stack.pop();
                    if matches!(tag_end, TagEnd::Heading(_)) {
                        _in_heading = false;
                        flush_line(&mut current_spans, &mut current_lines);
                    }
                    if matches!(tag_end, TagEnd::BlockQuote(_)) {
                        flush_line(&mut current_spans, &mut current_lines);
                    }
                }
                TagEnd::Paragraph => {
                    flush_line(&mut current_spans, &mut current_lines);
                    flush_block(&mut current_lines, &mut blocks);
                }
                TagEnd::List(_) => {
                    list_depth = list_depth.saturating_sub(1);
                    flush_line(&mut current_spans, &mut current_lines);
                    if list_depth == 0 {
                        flush_block(&mut current_lines, &mut blocks);
                    }
                }
                TagEnd::Item => {
                    flush_line(&mut current_spans, &mut current_lines);
                }
                _ => {}
            },
            Event::Text(text) => {
                if in_code_block {
                    code_content.push_str(&text);
                } else {
                    let style = current_style(&style_stack);
                    current_spans.push(Span::styled(text.to_string(), style));
                }
            }
            Event::Code(code) => {
                // Inline code.
                match conceal {
                    ConcealLevel::Visible => {
                        current_spans.push(Span::styled("`", styles::DIM));
                        current_spans.push(Span::styled(code.to_string(), styles::CODE_INLINE));
                        current_spans.push(Span::styled("`", styles::DIM));
                    }
                    ConcealLevel::Concealed => {
                        current_spans.push(Span::styled(code.to_string(), styles::CODE_INLINE));
                    }
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                flush_line(&mut current_spans, &mut current_lines);
            }
            _ => {}
        }
    }

    // Flush remaining content.
    flush_line(&mut current_spans, &mut current_lines);
    flush_block(&mut current_lines, &mut blocks);

    blocks
}

fn current_style(stack: &[Style]) -> Style {
    let mut result = Style::default();
    for s in stack {
        result = result.patch(*s);
    }
    result
}

fn flush_line(spans: &mut Vec<Span<'static>>, lines: &mut Vec<Line<'static>>) {
    if !spans.is_empty() {
        lines.push(Line::from(spans.drain(..).collect::<Vec<_>>()));
    }
}

fn flush_block(lines: &mut Vec<Line<'static>>, blocks: &mut Vec<MarkdownBlock>) {
    if !lines.is_empty() {
        blocks.push(MarkdownBlock::Text(lines.drain(..).collect()));
    }
}

/// Convert all markdown blocks into flat ratatui Lines for rendering.
pub fn blocks_to_lines(blocks: &[MarkdownBlock]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        match block {
            MarkdownBlock::Text(text_lines) => {
                lines.extend(text_lines.iter().cloned());
            }
            MarkdownBlock::CodeBlock { highlighted, .. } => {
                if let Some(hl) = highlighted {
                    lines.extend(hl.iter().cloned());
                }
            }
        }
        // Add a blank line between blocks.
        if i + 1 < blocks.len() {
            lines.push(Line::default());
        }
    }
    lines
}
