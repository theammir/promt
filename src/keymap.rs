use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::mode::Mode;

/// An action the app should perform in response to a key event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    // -- Global (leader combos) --
    ToggleConceal,
    OpenModelPicker,
    OpenHistory,
    SaveConversation,
    NewConversation,
    Quit,
    CycleFavorite,
    JumpFavorite(u8),
    ToggleSystemPrompt,
    EditSystemPrompt,
    ShowHelp,

    // -- Insert mode --
    SendMessage,
    InsertNewline,
    InsertChar(char),
    DeleteCharBackward,
    DeleteCharForward,
    DeleteWordBackward,
    DeleteToLineStart,
    CursorLeft,
    CursorRight,
    CursorUp,
    CursorDown,
    CursorHome,
    CursorEnd,
    CancelOrClear,
    ScrollInputUp,
    ScrollInputDown,

    // -- Browse mode --
    PrevMessage,
    NextMessage,
    FirstMessage,
    LastMessage,
    EnterCodeblockSelect,
    SearchMessages,
    ReturnToInsert,
    ScrollHalfDown,
    ScrollHalfUp,
    ScrollLineDown,
    ScrollLineUp,

    // -- CodeblockSelect --
    NextCodeblock,
    PrevCodeblock,
    YankCodeblock,
    EditCodeblock,
    EnterVisual,
    EnterVisualLine,
    ExitToParent,
    CodeblockCursorUp,
    CodeblockCursorDown,
    CodeblockCursorLeft,
    CodeblockCursorRight,
    ScrollCodeblockDown,
    ScrollCodeblockUp,

    // -- Visual / VisualLine --
    SelectLeft,
    SelectRight,
    SelectUp,
    SelectDown,
    YankSelection,
    EditSelection,
    SwitchVisualMode,
    SwapVisualEnd,

    /// Leader key pressed, waiting for second key.
    LeaderPending,

    /// No action / key not mapped.
    None,
}

/// Leader key state machine.
#[derive(Debug)]
pub struct LeaderState {
    /// Whether the leader key is active (pressed, waiting for second key).
    pending: bool,
}

impl LeaderState {
    pub fn new() -> Self {
        Self { pending: false }
    }

    /// Mark the leader key as pressed right now.
    pub fn activate(&mut self) {
        self.pending = true;
    }

    /// Check if the leader key is active (pressed, waiting for second key).
    pub fn is_active(&self) -> bool {
        self.pending
    }

    /// Consume the leader state (after processing the second key or cancelling).
    pub fn consume(&mut self) {
        self.pending = false;
    }
}

/// Resolve a key event into an action, considering the current mode and leader state.
pub fn resolve(key: KeyEvent, mode: Mode, leader: &mut LeaderState) -> Action {
    // If leader is active, try to resolve as a leader combo.
    if leader.is_active() {
        leader.consume();
        if let Some(action) = resolve_leader_combo(key) {
            return action;
        }
        // Unrecognized second key: cancel leader, fall through to mode-specific.
    }

    // Check for leader key press (Ctrl+X).
    if is_leader_key(key) {
        leader.activate();
        return Action::LeaderPending;
    }

    // Mode-specific dispatch.
    match mode {
        Mode::Insert => resolve_insert(key),
        Mode::Browse => resolve_browse(key),
        Mode::CodeblockSelect => resolve_codeblock_select(key),
        Mode::Visual => resolve_visual(key),
        Mode::VisualLine => resolve_visual_line(key),
    }
}

fn is_leader_key(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('x') && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn resolve_leader_combo(key: KeyEvent) -> Option<Action> {
    // Leader combos don't use modifiers (just plain keys after C-x).
    match key.code {
        KeyCode::Char('c') => Some(Action::ToggleConceal),
        KeyCode::Char('m') => Some(Action::OpenModelPicker),
        KeyCode::Char('h') => Some(Action::OpenHistory),
        KeyCode::Char('s') => Some(Action::SaveConversation),
        KeyCode::Char('n') => Some(Action::NewConversation),
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char('f') => Some(Action::CycleFavorite),
        KeyCode::Char('t') => Some(Action::ToggleSystemPrompt),
        KeyCode::Char('e') => Some(Action::EditSystemPrompt),
        KeyCode::Char('?') => Some(Action::ShowHelp),
        KeyCode::Char(c @ '1'..='9') => Some(Action::JumpFavorite(c as u8 - b'0')),
        _ => Option::None,
    }
}

fn resolve_insert(key: KeyEvent) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    match key.code {
        KeyCode::Enter if shift => Action::InsertNewline,
        KeyCode::Enter if alt => Action::InsertNewline,
        KeyCode::Enter => Action::SendMessage,
        KeyCode::Char('p') if ctrl => Action::PrevMessage,
        KeyCode::Char('n') if ctrl => Action::NextMessage,
        KeyCode::Char('w') if ctrl => Action::DeleteWordBackward,
        KeyCode::Char('u') if ctrl => Action::DeleteToLineStart,
        KeyCode::Char('a') if ctrl => Action::CursorHome,
        KeyCode::Char('e') if ctrl => Action::CursorEnd,
        KeyCode::Char('c') if ctrl => Action::CancelOrClear,
        KeyCode::Char('j') if alt => Action::ScrollInputDown,
        KeyCode::Char('k') if alt => Action::ScrollInputUp,
        KeyCode::Char(c) => Action::InsertChar(c),
        KeyCode::Backspace => Action::DeleteCharBackward,
        KeyCode::Delete => Action::DeleteCharForward,
        KeyCode::Left => Action::CursorLeft,
        KeyCode::Right => Action::CursorRight,
        KeyCode::Up => Action::CursorUp,
        KeyCode::Down => Action::CursorDown,
        KeyCode::Home => Action::CursorHome,
        KeyCode::End => Action::CursorEnd,
        _ => Action::None,
    }
}

fn resolve_browse(key: KeyEvent) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Char('p') if ctrl => Action::PrevMessage,
        KeyCode::Char('n') if ctrl => Action::NextMessage,
        KeyCode::Char('d') if ctrl => Action::ScrollHalfDown,
        KeyCode::Char('u') if ctrl => Action::ScrollHalfUp,
        KeyCode::Char('k') => Action::ScrollLineUp,
        KeyCode::Char('j') => Action::ScrollLineDown,
        KeyCode::Char('g') => Action::FirstMessage,
        KeyCode::Char('G') => Action::LastMessage,
        KeyCode::Tab => Action::EnterCodeblockSelect,
        KeyCode::Char('/') => Action::SearchMessages,
        KeyCode::Esc | KeyCode::Enter => Action::ReturnToInsert,
        KeyCode::Char('i') => Action::ReturnToInsert,
        _ => Action::None,
    }
}

fn resolve_codeblock_select(key: KeyEvent) -> Action {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Char('d') if ctrl => Action::ScrollCodeblockDown,
        KeyCode::Char('u') if ctrl => Action::ScrollCodeblockUp,
        KeyCode::Tab if shift => Action::NextCodeblock,
        KeyCode::BackTab => Action::NextCodeblock,
        KeyCode::Tab => Action::PrevCodeblock,
        KeyCode::Char('y') => Action::YankCodeblock,
        KeyCode::Char('e') => Action::EditCodeblock,
        KeyCode::Char('v') => Action::EnterVisual,
        KeyCode::Char('V') => Action::EnterVisualLine,
        KeyCode::Char('h') => Action::CodeblockCursorLeft,
        KeyCode::Char('l') => Action::CodeblockCursorRight,
        KeyCode::Char('k') => Action::CodeblockCursorUp,
        KeyCode::Char('j') => Action::CodeblockCursorDown,
        KeyCode::Esc => Action::ExitToParent,
        _ => Action::None,
    }
}

fn resolve_visual(key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Char('h') => Action::SelectLeft,
        KeyCode::Char('l') => Action::SelectRight,
        KeyCode::Char('j') => Action::SelectDown,
        KeyCode::Char('k') => Action::SelectUp,
        KeyCode::Char('V') => Action::SwitchVisualMode,
        KeyCode::Char('y') => Action::YankSelection,
        KeyCode::Char('o') => Action::SwapVisualEnd,
        KeyCode::Esc => Action::ExitToParent,
        _ => Action::None,
    }
}

fn resolve_visual_line(key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Char('j') => Action::SelectDown,
        KeyCode::Char('k') => Action::SelectUp,
        KeyCode::Char('v') => Action::SwitchVisualMode,
        KeyCode::Char('y') => Action::YankSelection,
        KeyCode::Char('e') => Action::EditSelection,
        KeyCode::Char('o') => Action::SwapVisualEnd,
        KeyCode::Esc => Action::ExitToParent,
        _ => Action::None,
    }
}
