# promt — Inline TUI for LLM Prompting

## Summary

Rust CLI + inline TUI (fzf-style, not fullscreen) for LLM chat. Uses the `llm` crate for multi-provider inference. Vim-inspired keybindings, syntax-highlighted markdown with conceal levels, codeblock visual selection/editing, conversation persistence.

## Dependencies

```toml
[dependencies]
llm = { version = "1.3", features = [
  "openai", "anthropic", "google", "ollama", "deepseek", "xai",
  "groq", "azure-openai", "aws-bedrock", "openrouter", "cohere",
  "mistral", "huggingface", "phind"
] }
ratatui = { version = "0.30", default-features = true }
crossterm = "0.29"
syntect = { version = "5.3", default-features = false, features = ["default-fancy"] }
pulldown-cmark = "0.13"
tokio = { version = "1", features = ["full"] }
futures = "0.3"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4", features = ["derive"] }
dirs = "6"
arboard = "3"
nucleo = "0.5"
unicode-width = "0.2"
chrono = { version = "0.4", features = ["serde"] }
```

## Project Structure

```
src/
  main.rs              Entry, CLI (clap), bootstrap
  app.rs               App state, event loop, mode transitions
  config.rs            Config load/save, provider credentials
  provider.rs          LLM client construction, model switching
  conversation.rs      Message types, history, JSON persistence
  ui/
    mod.rs             Root render fn, layout
    messages.rs        Message list + halfblock borders
    input.rs           Multi-line input widget
    codeblock.rs       Codeblock extraction, selection overlay
    markdown.rs        MD parsing + conceal levels
    status.rs          Status bar + leader key indicator
    picker.rs          Fuzzy picker overlay (models, history)
    search.rs          Fuzzy search overlay (nucleo)
  keymap.rs            Leader key system, mode-aware dispatch
  mode.rs              Mode enum + state machine
  clipboard.rs         System clipboard + $EDITOR
  highlight.rs         syntect wrapper, color conversion
```

## Modes

There are 5 modes forming a linear state machine:

- Insert (default): Input area active. Type, paste, Shift+Enter for newlines, Enter to send.
- Browse: Navigate messages with Ctrl+P/Ctrl+N (or j/k). Selected message highlighted.
- CodeblockSelect: Tab/Shift-Tab cycles codeblocks in selected message. y yanks, e opens $EDITOR.
- Visual: Character selection within a codeblock. h/j/k/l to move, y to yank.
- VisualLine: Line selection within a codeblock. j/k to extend, y to yank, e to edit in $EDITOR.

Transitions:
- Insert -> Browse: Ctrl+P or Ctrl+N
- Browse -> Insert: Esc, Enter, or i
- Browse -> CodeblockSelect: Tab (if current message has codeblocks)
- CodeblockSelect -> Browse: Esc
- CodeblockSelect -> Visual: v
- CodeblockSelect -> VisualLine: V
- Visual -> CodeblockSelect: Esc
- VisualLine -> CodeblockSelect: Esc
- Visual <-> VisualLine: V or v to switch between them

## Keymap

### Leader Key: Ctrl+X (configurable)

Global combos (work in every mode). After pressing Ctrl+X, status bar shows "C-x ..." with ~500ms timeout waiting for the second key.

- C-x c: Toggle conceal level (0 <-> 1)
- C-x m: Open model/provider picker
- C-x h: Open conversation history browser
- C-x s: Save current conversation
- C-x n: New conversation (prompts to save if unsaved)
- C-x q: Quit (prompts to save if unsaved)
- C-x f: Cycle favorite models
- C-x 1..9: Jump to favorite model N
- C-x t: Toggle system prompt display
- C-x e: Edit system prompt in $EDITOR
- C-x ?: Show keybind help overlay

### Insert Mode

- Enter: Send message
- Shift+Enter: Insert newline
- Ctrl+P: Enter Browse mode (select previous message)
- Ctrl+N: Enter Browse mode (select next message)
- Ctrl+W: Delete word backwards
- Ctrl+U: Delete to start of line
- Ctrl+A: Cursor to start of line
- Ctrl+E: Cursor to end of line
- Ctrl+C: Cancel streaming response / clear input
- Backspace/Delete: Standard editing
- Up/Down: Move cursor within multi-line input
- Left/Right: Move cursor within line

### Browse Mode

- Ctrl+P / k: Previous message
- Ctrl+N / j: Next message
- g / G: First / last message
- Tab: Enter CodeblockSelect (if message has codeblocks)
- /: Fuzzy search across messages (nucleo)
- Esc / Enter / i: Return to Insert mode

### CodeblockSelect Mode

- Tab: Next codeblock
- Shift+Tab: Previous codeblock
- y: Yank entire codeblock to clipboard
- e: Open codeblock in $EDITOR
- v: Enter Visual mode
- V: Enter VisualLine mode
- Esc: Return to Browse mode

### Visual Mode (character selection in codeblock)

- h/j/k/l: Move selection cursor
- V: Switch to VisualLine
- y: Yank selection, exit to CodeblockSelect
- Esc: Cancel, return to CodeblockSelect

### VisualLine Mode (line selection in codeblock)

- j/k: Extend/shrink line selection
- v: Switch to Visual (character) mode
- y: Yank selected lines, exit to CodeblockSelect
- e: Open selected lines in $EDITOR
- Esc: Cancel, return to CodeblockSelect

## Viewport

Inline (not fullscreen). Uses ratatui Viewport::Inline. Grow-to-fit behavior: starts at minimum height (~5 lines), grows as messages arrive, caps at terminal height minus a few lines.

### Layout

```
+--------------------------------------------------+
| [halfblock] You: message text                    |  <- user msg (blue halfblock)
|                                                  |
| [halfblock] Assistant: response text             |  <- LLM msg (green halfblock)
| [halfblock] ```rust                              |  <- syntax-highlighted codeblock
| [halfblock] fn main() { }                        |
| [halfblock] ```                                  |
|--------------------------------------------------|
| > input area (multi-line)                        |  <- input
|--------------------------------------------------|
| anthropic/claude-sonnet | 1.2k tokens | C-x ... |  <- status bar
+--------------------------------------------------+
```

Halfblock left border:
- User messages: blue
- Assistant messages: green
- System messages: yellow/dim

## Markdown Rendering

Two conceal levels, toggled by C-x c:

- Level 0 (highlighted, chars visible): Markdown syntax chars are shown but dimmed. **bold** displays as **bold** with ** dimmed. Code fences visible. Content is styled (bold, italic, etc).
- Level 1 (concealed, default): Markdown markers hidden. bold renders as bold text. Code fences hidden, content syntax-highlighted with background. Headers styled, # hidden. Lists show bullets.

Parsing: pulldown-cmark. Code highlighting: syntect with language detection from fence info string. Theme configurable (default: base16-ocean.dark).

## Input Area

Custom multi-line widget. Enter sends, Shift+Enter inserts newline. Bracketed paste support via crossterm: large pastes (>5 lines) displayed as "[Pasted ~N lines]" but full content stored and sent. Standard editing: Ctrl+W (delete word), Ctrl+U (delete to line start), Ctrl+A/E (home/end).

## Streaming

Uses llm crate chat_stream(). Tokens appended incrementally to current assistant message. Markdown re-rendered progressively. Codeblock detection works as fences appear. Ctrl+C cancels, keeps partial response. Spinner in status bar during generation.

## Provider & Model Switching

Model picker (C-x m): fzf-style fuzzy list overlay showing provider/model pairs. Uses nucleo for fuzzy matching. Selection changes active model for subsequent messages.

Favorites (config): C-x 1..9 for quick switch. C-x f cycles through favorites list.

Runtime model listing via llm ModelsProvider trait where available.

## Configuration

Location: ~/.config/promt/config.toml

```toml
[general]
default_provider = "anthropic"
default_model = "claude-sonnet-4-20250514"
conceal_level = 1
theme = "base16-ocean.dark"
leader_key = "C-x"

[providers.openai]
api_key = "sk-..."

[providers.anthropic]
api_key = "sk-ant-..."

[providers.ollama]
base_url = "http://localhost:11434"

[favorites]
1 = { provider = "anthropic", model = "claude-sonnet-4-20250514" }
2 = { provider = "openai", model = "gpt-4.1" }
3 = { provider = "ollama", model = "llama3" }
```

Env var overrides: OPENAI_API_KEY, ANTHROPIC_API_KEY, etc. override config values.

## Conversation Persistence

Location: ~/.local/share/promt/conversations/*.json

Format:
```json
{
  "metadata": {
    "id": "uuid",
    "created": "2026-03-07T15:30:00Z",
    "updated": "2026-03-07T15:35:00Z",
    "provider": "anthropic",
    "model": "claude-sonnet-4-20250514",
    "title": "What is a monad?"
  },
  "messages": [
    {
      "role": "user",
      "content": "What is a monad?",
      "timestamp": "2026-03-07T15:30:00Z"
    },
    {
      "role": "assistant",
      "content": "A monad is...",
      "timestamp": "2026-03-07T15:30:02Z"
    }
  ]
}
```

## CLI

```
promt [OPTIONS] [PROMPT]       Interactive or one-shot
promt history                  Browse past conversations
promt config                   Print/open config
promt providers                List providers + models

Options:
  -p, --provider <PROVIDER>    Override provider
  -m, --model <MODEL>          Override model
  -c, --continue               Continue last conversation
  -s, --system <PROMPT>        Set system prompt
  --no-stream                  Disable streaming
  --raw                        Raw text output, no TUI (for piping)
```

Pipe mode: "echo 'explain monads' | promt --raw" for non-interactive use.

## Implementation Phases

### Phase 1: Foundation
- Project scaffolding (modules, Cargo.toml dependencies)
- Config loading (config.rs)
- Basic app event loop with crossterm + ratatui inline viewport (app.rs)
- Mode enum and state machine (mode.rs)
- Keymap dispatch with leader key system (keymap.rs)
- Input area with editing and paste handling (ui/input.rs)
- Basic message rendering with halfblock borders (ui/messages.rs)
- Status bar with leader key indicator (ui/status.rs)

### Phase 2: LLM Integration
- Provider abstraction, build LLM client from config (provider.rs)
- Send messages and stream responses
- Conversation data model and in-memory history (conversation.rs)
- Model/provider picker overlay (ui/picker.rs)

### Phase 3: Markdown & Highlighting
- Markdown parser with 2 conceal levels (ui/markdown.rs)
- syntect integration for code blocks (highlight.rs)
- Incremental rendering during streaming

### Phase 4: Visual Modes
- Browse mode: Ctrl+P/N message navigation
- CodeblockSelect mode: Tab/Shift-Tab cycling, codeblock extraction (ui/codeblock.rs)
- Visual and VisualLine selection modes
- Clipboard integration (clipboard.rs)
- $EDITOR integration

### Phase 5: Persistence & Polish
- Conversation save/load to JSON
- History browser subcommand with fuzzy search (ui/search.rs)
- Favorites quick-switching
- Non-interactive / pipe mode
- First-run setup experience
- Error handling, edge cases
