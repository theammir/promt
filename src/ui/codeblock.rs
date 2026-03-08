/// Codeblock extraction, selection, and overlay rendering.
use crate::mode::CursorPos;
use crate::ui::markdown::MarkdownBlock;

/// State for codeblock selection within a message.
#[derive(Debug)]
pub struct CodeblockSelectState {
    /// Index of the selected codeblock within the message's blocks.
    pub selected: usize,
    /// Total number of codeblocks in the current message.
    total: usize,
    /// Cursor position within the selected codeblock (persistent across tab cycles).
    pub cursor: CursorPos,
    /// If true, Tab traverses upward (bottom-up) and Shift-Tab downward.
    /// Used when the selected message is the last/newest message.
    pub reverse: bool,
}

impl CodeblockSelectState {
    /// Create a new state. `reverse` = true means enter on the last codeblock
    /// (bottom-up for newest message), false = enter on the first (top-down).
    pub fn new(total: usize, reverse: bool) -> Option<Self> {
        if total == 0 {
            None
        } else {
            let selected = if reverse { total - 1 } else { 0 };
            Some(Self {
                selected,
                total,
                cursor: CursorPos { row: 0, col: 0 },
                reverse,
            })
        }
    }

    /// Advance in the primary direction (Tab).
    /// In reverse mode: moves upward (decrement). Otherwise: moves downward (increment).
    pub fn advance(&mut self) {
        if self.reverse {
            self.selected = if self.selected == 0 {
                self.total - 1
            } else {
                self.selected - 1
            };
        } else {
            self.selected = (self.selected + 1) % self.total;
        }
        self.cursor = CursorPos { row: 0, col: 0 };
    }

    /// Go in the opposite direction of advance (Shift-Tab).
    pub fn retreat(&mut self) {
        if self.reverse {
            self.selected = (self.selected + 1) % self.total;
        } else {
            self.selected = if self.selected == 0 {
                self.total - 1
            } else {
                self.selected - 1
            };
        }
        self.cursor = CursorPos { row: 0, col: 0 };
    }
}

/// Extract the code string from the nth codeblock in the markdown blocks.
pub fn get_codeblock_content(blocks: &[MarkdownBlock], index: usize) -> Option<&str> {
    blocks
        .iter()
        .filter_map(|b| match b {
            MarkdownBlock::CodeBlock { code, .. } => Some(code.as_str()),
            _ => None,
        })
        .nth(index)
}

/// Count codeblocks in the markdown blocks.
pub fn count_codeblocks(blocks: &[MarkdownBlock]) -> usize {
    blocks
        .iter()
        .filter(|b| matches!(b, MarkdownBlock::CodeBlock { .. }))
        .count()
}
