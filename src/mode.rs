/// Application modes forming a linear state machine.
///
/// Insert (default) -> Browse (Ctrl+P/N) -> CodeblockSelect (Tab)
///   -> Visual/VisualLine (v/V)
///
/// Esc/Enter returns toward Insert.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Default mode. Input area is active, user can type and send messages.
    Insert,
    /// Navigate messages with Ctrl+P/N or j/k. Selected message highlighted.
    Browse,
    /// Tab/Shift-Tab cycles codeblocks in the selected message.
    CodeblockSelect,
    /// Character-level selection within a codeblock.
    Visual,
    /// Line-level selection within a codeblock.
    VisualLine,
}

impl Mode {
    /// Whether this mode allows editing the input area.
    pub fn is_insert(self) -> bool {
        self == Mode::Insert
    }

    /// The mode to return to when pressing Esc.
    pub fn esc_target(self) -> Mode {
        match self {
            Mode::Insert => Mode::Insert,
            Mode::Browse => Mode::Insert,
            Mode::CodeblockSelect => Mode::Browse,
            Mode::Visual | Mode::VisualLine => Mode::CodeblockSelect,
        }
    }
}

impl Default for Mode {
    fn default() -> Self {
        Mode::Insert
    }
}

/// Selection state for visual modes within a codeblock.
#[derive(Debug, Clone)]
pub struct VisualSelection {
    /// Index of the codeblock within the message.
    pub block_index: usize,
    /// Anchor position (where selection started).
    pub anchor: CursorPos,
    /// Current cursor position (selection extends from anchor to cursor).
    pub cursor: CursorPos,
}

impl VisualSelection {
    pub fn new(block_index: usize, row: usize, col: usize) -> Self {
        let pos = CursorPos { row, col };
        Self {
            block_index,
            anchor: pos,
            cursor: pos,
        }
    }

    /// Returns (start, end) rows of the selection, ordered.
    pub fn row_range(&self) -> (usize, usize) {
        let a = self.anchor.row;
        let b = self.cursor.row;
        (a.min(b), a.max(b))
    }

    /// Returns ordered (start, end) positions for characterwise selection.
    /// start is the position with the smaller (row, col) tuple.
    pub fn ordered(&self) -> (CursorPos, CursorPos) {
        if (self.anchor.row, self.anchor.col) <= (self.cursor.row, self.cursor.col) {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }

    /// Swap anchor and cursor, preserving the selection shape.
    pub fn swap(&mut self) {
        std::mem::swap(&mut self.anchor, &mut self.cursor);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CursorPos {
    pub row: usize,
    pub col: usize,
}
