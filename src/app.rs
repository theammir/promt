use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::{Terminal, TerminalOptions, Viewport};
use tokio::sync::mpsc;

use crate::clipboard;
use crate::config::Config;
use crate::conversation::{Conversation, Message};
use crate::highlight::Highlighter;
use crate::keymap::{self, Action, LeaderState};
use crate::mode::{Mode, VisualSelection};
use crate::provider::{self, ProviderState};
use crate::ui::codeblock::CodeblockSelectState;
use crate::ui::input::InputState;
use crate::ui::markdown::ConcealLevel;
use crate::ui::messages::MessageListState;
use crate::ui::picker::PickerState;
use crate::ui::search::SearchState;
use crate::ui::status::StatusState;

/// Events sent from the streaming task back to the main event loop.
#[derive(Debug)]
pub enum StreamEvent {
    /// A token chunk arrived.
    Token(String),
    /// Streaming completed successfully.
    Done,
    /// An error occurred during streaming.
    Error(String),
}

/// Core application state.
pub struct App {
    pub mode: Mode,
    pub input: InputState,
    pub message_list: MessageListState,
    pub status: StatusState,
    pub config: Config,
    pub conversation: Conversation,
    pub highlighter: Highlighter,
    pub conceal: ConcealLevel,
    pub provider: ProviderState,
    pub leader: LeaderState,
    pub should_quit: bool,
    pub picker: PickerState,
    /// Current viewport height (grows to fit).
    pub viewport_height: u16,
    /// Minimum viewport height.
    pub min_viewport_height: u16,
    /// Maximum viewport height (terminal height - margin).
    pub max_viewport_height: u16,
    /// Channel receiver for streaming events from the LLM task.
    stream_rx: Option<mpsc::UnboundedReceiver<StreamEvent>>,
    /// Cancellation flag for the active streaming task.
    stream_cancel: Option<Arc<AtomicBool>>,
    /// The partial assistant response being accumulated during streaming.
    streaming_content: String,
    /// Codeblock selection state (active in CodeblockSelect mode).
    pub codeblock_select: Option<CodeblockSelectState>,
    /// Visual selection state (active in Visual/VisualLine modes).
    pub visual_selection: Option<VisualSelection>,
    /// System prompt text (set via --system or C-x e).
    pub system_prompt: Option<String>,
    /// Whether to show the system prompt in the message list.
    pub show_system_prompt: bool,
    /// Whether the help overlay is visible.
    pub help_visible: bool,
    /// Fuzzy search state for searching within messages.
    pub search: SearchState,
    /// Timestamp when the status message was set (for auto-dismiss).
    pub status_message_at: Option<Instant>,
    /// Current favorite index for cycling (wraps around favorites list).
    pub favorite_cycle_index: usize,
    /// Paths from the last history listing (for picker selection).
    history_paths: Vec<std::path::PathBuf>,
    /// Flag: scroll message list to bottom before next render.
    /// Used to defer scroll until after the viewport has been resized.
    pub needs_scroll_bottom: bool,
}

impl App {
    pub fn new(config: Config) -> Self {
        let conceal = ConcealLevel::from_u8(config.general.conceal_level);
        let highlighter = Highlighter::new(&config.general.theme);
        let provider = ProviderState::from_config(&config);

        // Show build error in status if client failed.
        let mut status = StatusState::new(&provider.provider_name, &provider.model_name);
        if let Some(ref err) = provider.build_error {
            status.status_message = Some(format!("Provider error: {err}"));
        }

        let conversation = Conversation::new(&provider.provider_name, &provider.model_name);

        Self {
            mode: Mode::Insert,
            input: InputState::new(),
            message_list: MessageListState::new(),
            status,
            config,
            conversation,
            highlighter,
            conceal,
            provider,
            leader: LeaderState::new(),
            should_quit: false,
            picker: PickerState::new(),
            viewport_height: 5,
            min_viewport_height: 5,
            max_viewport_height: 30,
            stream_rx: None,
            stream_cancel: None,
            streaming_content: String::new(),
            codeblock_select: None,
            visual_selection: None,
            system_prompt: None,
            show_system_prompt: false,
            help_visible: false,
            search: SearchState::new(),
            status_message_at: None,
            favorite_cycle_index: 0,
            history_paths: Vec::new(),
            needs_scroll_bottom: false,
        }
    }

    /// Compute the desired viewport height based on content.
    fn desired_height(&self) -> u16 {
        let message_lines = (self.message_list.total_lines()).min(u16::MAX as usize) as u16;
        let input_lines = (self.input.lines.len()).min(u16::MAX as usize) as u16;
        let chrome = 3; // separator + status bar + margin
        let mut needed =
            message_lines.saturating_add(input_lines.clamp(1, 10)).saturating_add(chrome);
        // Help overlay needs enough room to be useful; expand viewport when visible.
        if self.help_visible {
            needed = needed.max(self.max_viewport_height);
        }
        needed.clamp(self.min_viewport_height, self.max_viewport_height)
    }

    /// Compute the actual height of the message pane (viewport minus input, separator, status).
    /// This must mirror the layout constraints in ui/mod.rs.
    pub fn message_pane_height(&self) -> usize {
        let input_h = (self.input.lines.len() as u16).clamp(1, 10);
        let chrome = 1 + 1; // separator + status bar
        self.viewport_height
            .saturating_sub(input_h)
            .saturating_sub(chrome) as usize
    }

    /// Run the main event loop with an inline viewport (async).
    pub async fn run(&mut self) -> io::Result<()> {
        // Determine max viewport height from terminal size.
        // Cap at a compact fzf-like size: min(18, max(8, 40% of terminal height)).
        let (_, term_h) = crossterm::terminal::size()?;
        let compact_cap = 18u16.min(8u16.max(term_h * 40 / 100));
        self.max_viewport_height = compact_cap.max(self.min_viewport_height);

        crossterm::terminal::enable_raw_mode()?;
        crossterm::execute!(io::stdout(), crossterm::event::EnableBracketedPaste)?;

        // Enable kitty keyboard protocol for terminals that support it.
        // This allows distinguishing Shift+Enter from plain Enter.
        // Terminals that don't support this will silently ignore it.
        let _ = crossterm::execute!(
            io::stdout(),
            crossterm::event::PushKeyboardEnhancementFlags(
                crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | crossterm::event::KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            )
        );

        let backend = CrosstermBackend::new(io::stdout());
        // Create the viewport at max height upfront. Ratatui's Viewport::Inline
        // uses the creation-time height as an immutable cap — resize() can never
        // grow past it. We reserve the full space now and use internal scrolling
        // to render only the portion we need.
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(self.max_viewport_height),
            },
        )?;
        // Immediately resize down to the initial desired height so we start small.
        self.viewport_height = self.desired_height();
        terminal.resize(Rect::new(
            0,
            0,
            crossterm::terminal::size()?.0,
            self.viewport_height,
        ))?;

        let result = self.event_loop(&mut terminal).await;

        // Erase the inline viewport so the UI disappears on quit (like fzf).
        terminal.clear()?;

        // Cleanup: reverse order of setup.
        let _ = crossterm::execute!(
            io::stdout(),
            crossterm::event::PopKeyboardEnhancementFlags
        );
        crossterm::execute!(io::stdout(), crossterm::event::DisableBracketedPaste)?;
        crossterm::terminal::disable_raw_mode()?;

        result
    }

    async fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> io::Result<()> {
        loop {
            // Update viewport size if needed — only grow, never shrink.
            // terminal.resize() causes a full clear, so we minimize calls.
            // Growth is acceptable (coincides with new content), but shrinking
            // would cause disruptive flicker with no benefit.
            let desired = self.desired_height();
            if desired > self.viewport_height {
                self.viewport_height = desired;
                terminal.resize(Rect::new(0, 0, crossterm::terminal::size()?.0, desired))?;
            }

            // Consume deferred scroll-to-bottom (now that viewport is up-to-date).
            if self.needs_scroll_bottom {
                self.needs_scroll_bottom = false;
                self.message_list
                    .scroll_to_bottom(self.message_pane_height());
            }

            // Render.
            let completed = terminal.draw(|frame| {
                let area = frame.area();
                crate::ui::render(self, area, frame.buffer_mut());
            })?;
            // Track actual rendered viewport height (may differ from self.viewport_height
            // because ratatui's Viewport::Inline uses the creation-time height internally).
            self.viewport_height = completed.area.height;

            // Auto-dismiss status messages.
            self.check_status_dismiss();

            if self.should_quit {
                break;
            }

            // Determine poll timeout.
            let timeout = if self.status.is_streaming {
                // Poll more frequently during streaming for responsiveness.
                Duration::from_millis(16)
            } else {
                Duration::from_millis(100)
            };

            // Multiplex: terminal events + stream events.
            // We use tokio::select! to handle both concurrently.
            tokio::select! {
                // Terminal input events (polled on a blocking thread).
                term_event = poll_terminal_event(timeout) => {
                    match term_event {
                        Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                            self.handle_key(key);
                        }
                        Some(Ok(Event::Paste(text))) => {
                            if self.mode.is_insert() {
                                self.input.paste(&text);
                                self.ensure_input_visible();
                            }
                        }
                        Some(Ok(Event::Resize(_w, h))) => {
                            // Recompute the compact cap using the same formula as startup.
                            let compact_cap = 18u16.min(8u16.max(h * 40 / 100));
                            self.max_viewport_height = compact_cap.max(self.min_viewport_height);
                            // If terminal shrank below our viewport, we must shrink too.
                            if self.viewport_height > self.max_viewport_height {
                                self.viewport_height = self.max_viewport_height;
                                terminal.resize(Rect::new(0, 0, _w, self.viewport_height))?;
                            }
                        }
                        Some(Err(_)) => {
                            // Terminal error; continue.
                        }
                        _ => {
                            // Timeout or unhandled event.
                        }
                    }
                }
                // Stream events from the LLM task.
                stream_event = recv_stream_event(&mut self.stream_rx) => {
                    if let Some(event) = stream_event {
                        self.handle_stream_event(event);
                    }
                }
            }
        }

        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // If the help overlay is visible, dismiss on any key.
        if self.help_visible {
            self.help_visible = false;
            return;
        }

        // If the search overlay is visible, route keys to search.
        if self.search.visible {
            self.handle_search_key(key);
            return;
        }

        // If the picker is visible, route keys to the picker.
        if self.picker.visible {
            self.handle_picker_key(key);
            return;
        }

        let action = keymap::resolve(key, self.mode, &mut self.leader);
        self.status.leader_pending = self.leader.is_active();
        self.dispatch(action);
    }

    fn handle_picker_key(&mut self, key: KeyEvent) {
        use crossterm::event::{KeyCode, KeyModifiers};

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Esc => {
                self.picker.close();
                self.history_paths.clear();
            }
            KeyCode::Enter => {
                if !self.history_paths.is_empty() {
                    // History picker: selected index maps to history_paths.
                    let sel_idx = self.picker.selected;
                    if let Some(&filtered_idx) = self.picker.filtered.get(sel_idx) {
                        self.picker.close();
                        self.load_history_conversation(filtered_idx);
                        self.history_paths.clear();
                    } else {
                        self.picker.close();
                        self.history_paths.clear();
                    }
                } else {
                    // Model picker: parse "provider/model" format.
                    if let Some(selected) = self.picker.selected_item() {
                        let selected = selected.to_string();
                        if let Some((prov, model)) = selected.split_once('/') {
                            self.provider.switch(&self.config, prov, model);
                            self.status.provider = prov.to_string();
                            self.status.model = model.to_string();
                            if let Some(ref err) = self.provider.build_error {
                                self.status.status_message =
                                    Some(format!("Provider error: {err}"));
                            } else {
                                self.status.status_message = None;
                            }
                        }
                    }
                    self.picker.close();
                }
            }
            KeyCode::Char('p') if ctrl => {
                self.picker.select_prev();
            }
            KeyCode::Char('n') if ctrl => {
                self.picker.select_next();
            }
            KeyCode::Up => {
                self.picker.select_prev();
            }
            KeyCode::Down => {
                self.picker.select_next();
            }
            KeyCode::Backspace => {
                self.picker.query.pop();
                self.picker.update_filter();
            }
            KeyCode::Char(c) => {
                self.picker.query.push(c);
                self.picker.update_filter();
            }
            _ => {}
        }
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        use crossterm::event::{KeyCode, KeyModifiers};

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Esc => {
                self.search.close();
            }
            KeyCode::Enter => {
                if let Some(msg_idx) = self.search.selected_message_index() {
                    self.search.close();
                    // Jump to the matched message in browse mode.
                    self.mode = Mode::Browse;
                    self.status.mode = self.mode;
                    self.message_list.selected = Some(msg_idx);
                    self.message_list
                        .scroll_to_selected(self.message_pane_height());
                }
            }
            KeyCode::Char('p') if ctrl => {
                self.search.select_prev();
            }
            KeyCode::Char('n') if ctrl => {
                self.search.select_next();
            }
            KeyCode::Up => {
                self.search.select_prev();
            }
            KeyCode::Down => {
                self.search.select_next();
            }
            KeyCode::Backspace => {
                self.search.query.pop();
                self.search.update_filter();
            }
            KeyCode::Char(c) => {
                self.search.query.push(c);
                self.search.update_filter();
            }
            _ => {}
        }
    }

    fn handle_stream_event(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::Token(token) => {
                self.streaming_content.push_str(&token);

                // Update the last message in conversation and re-render it.
                if let Some(last) = self.conversation.messages.last_mut() {
                    last.content = self.streaming_content.clone();
                }
                self.message_list.update_last(
                    self.conversation.messages.last().unwrap(),
                    self.conceal,
                    &self.highlighter,
                );
                // Defer scroll until after viewport resize.
                self.needs_scroll_bottom = true;
            }
            StreamEvent::Done => {
                self.status.is_streaming = false;
                // Finalize the last message with a full re-parse for correctness.
                if let Some(last) = self.conversation.messages.last() {
                    self.message_list
                        .finalize_last(last, self.conceal, &self.highlighter);
                }
                self.stream_rx = None;
                self.stream_cancel = None;
                self.streaming_content.clear();
            }
            StreamEvent::Error(err) => {
                self.status.is_streaming = false;
                self.status.status_message = Some(format!("Error: {err}"));
                // Finalize the partial message so incremental state is dropped.
                if let Some(last) = self.conversation.messages.last() {
                    self.message_list
                        .finalize_last(last, self.conceal, &self.highlighter);
                }
                self.stream_rx = None;
                self.stream_cancel = None;
                self.streaming_content.clear();
            }
        }
    }

    /// Spawn a streaming LLM request in a background task.
    fn spawn_stream(&mut self) {
        let Some(client) = self.provider.client.clone() else {
            self.status.status_message = Some("No LLM client configured".to_string());
            return;
        };

        let messages = provider::to_chat_messages(&self.conversation.messages);
        let (tx, rx) = mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_clone = cancel.clone();

        self.stream_rx = Some(rx);
        self.stream_cancel = Some(cancel);
        self.status.is_streaming = true;
        self.streaming_content.clear();

        tokio::spawn(async move {
            let result = client.chat_stream(&messages).await;

            match result {
                Ok(mut stream) => {
                    while let Some(chunk) = stream.next().await {
                        if cancel_clone.load(Ordering::Relaxed) {
                            let _ = tx.send(StreamEvent::Done);
                            return;
                        }
                        match chunk {
                            Ok(token) => {
                                if tx.send(StreamEvent::Token(token)).is_err() {
                                    return; // Receiver dropped.
                                }
                            }
                            Err(e) => {
                                let _ = tx.send(StreamEvent::Error(e.to_string()));
                                return;
                            }
                        }
                    }
                    let _ = tx.send(StreamEvent::Done);
                }
                Err(e) => {
                    let _ = tx.send(StreamEvent::Error(e.to_string()));
                }
            }
        });
    }

    fn dispatch(&mut self, action: Action) {
        match action {
            // -- Global --
            Action::Quit => self.should_quit = true,
            Action::ToggleConceal => {
                self.conceal = self.conceal.toggle();
                self.re_render_messages();
            }
            Action::LeaderPending => {
                self.status.leader_pending = true;
            }
            Action::NewConversation => {
                // Cancel any active stream first.
                if self.status.is_streaming {
                    self.cancel_stream();
                }
                self.conversation =
                    Conversation::new(&self.provider.provider_name, &self.provider.model_name);
                self.message_list = MessageListState::new();
                self.input.clear();
                self.codeblock_select = None;
                self.visual_selection = None;
                self.mode = Mode::Insert;
                self.status.mode = self.mode;
                self.help_visible = false;
                self.search.visible = false;
                self.favorite_cycle_index = 0;
            }
            Action::SaveConversation => {
                if let Some(dir) = Config::conversations_dir() {
                    match self.conversation.save(&dir) {
                        Ok(_) => {
                            self.status.status_message = Some("Saved".to_string());
                        }
                        Err(e) => {
                            self.status.status_message = Some(format!("Save error: {e}"));
                        }
                    }
                } else {
                    self.status.status_message =
                        Some("Cannot determine data directory".to_string());
                }
                self.set_status_timer();
            }
            Action::OpenModelPicker => {
                self.open_model_picker();
            }

            // -- Insert mode --
            Action::SendMessage => {
                if !self.input.is_empty() {
                    self.send_message();
                }
            }
            Action::InsertNewline => {
                if self.mode.is_insert() {
                    self.input.insert_newline();
                    self.ensure_input_visible();
                }
            }
            Action::InsertChar(c) => {
                if self.mode.is_insert() {
                    self.input.insert_char(c);
                }
            }
            Action::DeleteCharBackward => {
                if self.mode.is_insert() {
                    self.input.delete_backward();
                    self.ensure_input_visible();
                }
            }
            Action::DeleteCharForward => {
                if self.mode.is_insert() {
                    self.input.delete_forward();
                }
            }
            Action::DeleteWordBackward => {
                if self.mode.is_insert() {
                    self.input.delete_word_backward();
                }
            }
            Action::DeleteToLineStart => {
                if self.mode.is_insert() {
                    self.input.delete_to_line_start();
                }
            }
            Action::CursorLeft => {
                if self.mode.is_insert() {
                    self.input.cursor_left();
                    self.ensure_input_visible();
                }
            }
            Action::CursorRight => {
                if self.mode.is_insert() {
                    self.input.cursor_right();
                    self.ensure_input_visible();
                }
            }
            Action::CursorUp => {
                if self.mode.is_insert() {
                    self.input.cursor_up();
                    self.ensure_input_visible();
                }
            }
            Action::CursorDown => {
                if self.mode.is_insert() {
                    self.input.cursor_down();
                    self.ensure_input_visible();
                }
            }
            Action::CursorHome => {
                if self.mode.is_insert() {
                    self.input.cursor_home();
                }
            }
            Action::CursorEnd => {
                if self.mode.is_insert() {
                    self.input.cursor_end();
                }
            }
            Action::CancelOrClear => {
                if self.status.is_streaming {
                    // Cancel active stream.
                    self.cancel_stream();
                } else if self.mode.is_insert() {
                    self.input.clear();
                }
            }
            Action::ScrollInputDown => {
                if self.mode.is_insert() {
                    self.input.scroll_down();
                }
            }
            Action::ScrollInputUp => {
                if self.mode.is_insert() {
                    self.input.scroll_up();
                }
            }

            // -- Browse mode navigation --
            Action::PrevMessage => {
                if self.mode == Mode::Insert {
                    self.mode = Mode::Browse;
                }
                self.message_list.select_prev();
                self.message_list.scroll_to_selected(self.message_pane_height());
                self.status.mode = self.mode;
            }
            Action::NextMessage => {
                if self.mode == Mode::Insert {
                    self.mode = Mode::Browse;
                }
                self.message_list.select_next();
                self.message_list.scroll_to_selected(self.message_pane_height());
                self.status.mode = self.mode;
            }
            Action::FirstMessage => {
                self.message_list.select_first();
                self.message_list.scroll_to_selected(self.message_pane_height());
            }
            Action::LastMessage => {
                self.message_list.select_last();
                self.message_list.scroll_to_selected(self.message_pane_height());
            }
            Action::ScrollLineDown => {
                let total = self.message_list.total_lines();
                let vp = self.message_pane_height();
                let max_scroll = total.saturating_sub(vp);
                self.message_list.scroll = (self.message_list.scroll + 1).min(max_scroll);
                self.message_list.update_focus_from_scroll(vp);
            }
            Action::ScrollLineUp => {
                self.message_list.scroll = self.message_list.scroll.saturating_sub(1);
                let vp = self.message_pane_height();
                self.message_list.update_focus_from_scroll(vp);
            }
            Action::ScrollHalfDown => {
                let vp = self.message_pane_height();
                let half = (vp / 2).max(1);
                let total = self.message_list.total_lines();
                let max_scroll = total.saturating_sub(vp);
                self.message_list.scroll = (self.message_list.scroll + half).min(max_scroll);
                self.message_list.update_focus_from_scroll(vp);
            }
            Action::ScrollHalfUp => {
                let half = (self.message_pane_height() / 2).max(1);
                self.message_list.scroll = self.message_list.scroll.saturating_sub(half);
                let vp = self.message_pane_height();
                self.message_list.update_focus_from_scroll(vp);
            }
            Action::ReturnToInsert => {
                self.mode = Mode::Insert;
                self.message_list.deselect();
                self.codeblock_select = None;
                self.visual_selection = None;
                self.status.mode = self.mode;
            }
            Action::EnterCodeblockSelect => {
                if let Some(idx) = self.message_list.selected {
                    if let Some(rendered) = self.message_list.rendered.get(idx) {
                        let count = crate::ui::codeblock::count_codeblocks(&rendered.blocks);
                        // Contextual direction: last message = reverse (bottom-up).
                        let is_last = idx + 1 == self.message_list.rendered.len();
                        if let Some(state) = CodeblockSelectState::new(count, is_last) {
                            self.codeblock_select = Some(state);
                            self.mode = Mode::CodeblockSelect;
                            self.status.mode = self.mode;
                            self.ensure_codeblock_cursor_visible();
                        }
                    }
                }
            }
            Action::ExitToParent => {
                let old_mode = self.mode;
                self.mode = self.mode.esc_target();
                self.status.mode = self.mode;
                // Clean up state when leaving modes.
                match old_mode {
                    Mode::CodeblockSelect => {
                        self.codeblock_select = None;
                    }
                    Mode::Visual | Mode::VisualLine => {
                        self.visual_selection = None;
                    }
                    _ => {}
                }
            }

            // -- Codeblock select --
            Action::NextCodeblock => {
                if let Some(ref mut cs) = self.codeblock_select {
                    cs.retreat();
                }
                self.ensure_codeblock_cursor_visible();
            }
            Action::PrevCodeblock => {
                if let Some(ref mut cs) = self.codeblock_select {
                    cs.advance();
                }
                self.ensure_codeblock_cursor_visible();
            }
            Action::CodeblockCursorUp => {
                if let Some(ref mut cs) = self.codeblock_select {
                    if cs.cursor.row > 0 {
                        cs.cursor.row -= 1;
                    }
                }
                // Clamp col to the new line length (read after mutation).
                let max_col = self.codeblock_select.as_ref().and_then(|cs| {
                    self.codeblock_line_len(cs.selected, cs.cursor.row)
                }).unwrap_or(1).saturating_sub(1);
                if let Some(ref mut cs) = self.codeblock_select {
                    cs.cursor.col = cs.cursor.col.min(max_col);
                }
                self.ensure_codeblock_cursor_visible();
            }
            Action::CodeblockCursorDown => {
                let max_row = self.codeblock_select.as_ref().and_then(|cs| {
                    self.codeblock_line_count(cs.selected)
                }).unwrap_or(1).saturating_sub(1);
                if let Some(ref mut cs) = self.codeblock_select {
                    if cs.cursor.row < max_row {
                        cs.cursor.row += 1;
                    }
                }
                // Clamp col to the new line length (read after mutation).
                let max_col = self.codeblock_select.as_ref().and_then(|cs| {
                    self.codeblock_line_len(cs.selected, cs.cursor.row)
                }).unwrap_or(1).saturating_sub(1);
                if let Some(ref mut cs) = self.codeblock_select {
                    cs.cursor.col = cs.cursor.col.min(max_col);
                }
                self.ensure_codeblock_cursor_visible();
            }
            Action::CodeblockCursorLeft => {
                if let Some(ref mut cs) = self.codeblock_select {
                    if cs.cursor.col > 0 {
                        cs.cursor.col -= 1;
                    }
                }
            }
            Action::CodeblockCursorRight => {
                let max_col = self.codeblock_select.as_ref().and_then(|cs| {
                    self.codeblock_line_len(cs.selected, cs.cursor.row)
                }).unwrap_or(0).saturating_sub(1);
                if let Some(ref mut cs) = self.codeblock_select {
                    if cs.cursor.col < max_col {
                        cs.cursor.col += 1;
                    }
                }
            }
            Action::ScrollCodeblockDown => {
                let vp = self.message_pane_height();
                let half = (vp / 2).max(1);
                let total = self.message_list.total_lines();
                let max_scroll = total.saturating_sub(vp);
                self.message_list.scroll =
                    (self.message_list.scroll + half).min(max_scroll);
            }
            Action::ScrollCodeblockUp => {
                let vp = self.message_pane_height();
                let half = (vp / 2).max(1);
                self.message_list.scroll =
                    self.message_list.scroll.saturating_sub(half);
            }
            Action::YankCodeblock => {
                if let (Some(cs), Some(msg_idx)) =
                    (&self.codeblock_select, self.message_list.selected)
                {
                    if let Some(rendered) = self.message_list.rendered.get(msg_idx) {
                        if let Some(code) =
                            crate::ui::codeblock::get_codeblock_content(&rendered.blocks, cs.selected)
                        {
                            match clipboard::copy_to_clipboard(code) {
                                Ok(method) => {
                                    let msg = if method.is_internal() {
                                        "Yanked codeblock (internal register only)"
                                    } else {
                                        "Yanked codeblock"
                                    };
                                    self.status.status_message = Some(msg.to_string());
                                }
                                Err(e) => {
                                    self.status.status_message = Some(format!("Yank failed: {e}"));
                                }
                            }
                        }
                    }
                }
            }
            Action::EditCodeblock => {
                if let (Some(cs), Some(msg_idx)) =
                    (&self.codeblock_select, self.message_list.selected)
                {
                    if let Some(rendered) = self.message_list.rendered.get(msg_idx) {
                        if let Some(code) =
                            crate::ui::codeblock::get_codeblock_content(&rendered.blocks, cs.selected)
                        {
                            let code = code.to_string();
                            // Temporarily leave raw mode for the editor.
                            let _ = crossterm::terminal::disable_raw_mode();
                            let _ = crossterm::execute!(
                                io::stdout(),
                                crossterm::event::DisableBracketedPaste
                            );
                            match clipboard::edit_in_editor(&code) {
                                Ok(edited) => {
                                    // Copy the edited result to clipboard for the user to paste.
                                    match clipboard::copy_to_clipboard(&edited) {
                                        Ok(method) => {
                                            let msg = if method.is_internal() {
                                                "Edited code saved to internal register only"
                                            } else {
                                                "Edited code copied to clipboard"
                                            };
                                            self.status.status_message =
                                                Some(msg.to_string());
                                        }
                                        Err(e) => {
                                            self.status.status_message =
                                                Some(format!("Clipboard error: {e}"));
                                        }
                                    }
                                }
                                Err(e) => {
                                    self.status.status_message =
                                        Some(format!("Editor error: {e}"));
                                }
                            }
                            // Restore raw mode.
                            let _ = crossterm::terminal::enable_raw_mode();
                            let _ = crossterm::execute!(
                                io::stdout(),
                                crossterm::event::EnableBracketedPaste
                            );
                        }
                    }
                }
            }
            Action::EnterVisual => {
                if let Some(ref cs) = self.codeblock_select {
                    self.visual_selection =
                        Some(VisualSelection::new(cs.selected, cs.cursor.row, cs.cursor.col));
                    self.mode = Mode::Visual;
                    self.status.mode = self.mode;
                }
            }
            Action::EnterVisualLine => {
                if let Some(ref cs) = self.codeblock_select {
                    self.visual_selection =
                        Some(VisualSelection::new(cs.selected, cs.cursor.row, cs.cursor.col));
                    self.mode = Mode::VisualLine;
                    self.status.mode = self.mode;
                }
            }

            // -- Visual modes --
            Action::SelectLeft => {
                if let Some(vs) = &mut self.visual_selection {
                    if vs.cursor.col > 0 {
                        vs.cursor.col -= 1;
                    }
                }
            }
            Action::SelectRight => {
                // Read needed values before mutating.
                let max_col = self.visual_selection.as_ref().and_then(|vs| {
                    self.codeblock_line_len(vs.block_index, vs.cursor.row)
                }).unwrap_or(0).saturating_sub(1);
                if let Some(vs) = &mut self.visual_selection {
                    if vs.cursor.col < max_col {
                        vs.cursor.col += 1;
                    }
                }
            }
            Action::SelectUp => {
                if let Some(vs) = &mut self.visual_selection {
                    if vs.cursor.row > 0 {
                        vs.cursor.row -= 1;
                    }
                }
                // Clamp col to the new line length.
                let max_col = self.visual_selection.as_ref().and_then(|vs| {
                    self.codeblock_line_len(vs.block_index, vs.cursor.row)
                }).unwrap_or(1).saturating_sub(1);
                if let Some(vs) = &mut self.visual_selection {
                    vs.cursor.col = vs.cursor.col.min(max_col);
                }
            }
            Action::SelectDown => {
                // Get max_row before mutating.
                let max_row = self.visual_selection.as_ref().and_then(|vs| {
                    self.codeblock_line_count(vs.block_index)
                }).unwrap_or(1).saturating_sub(1);
                if let Some(vs) = &mut self.visual_selection {
                    if vs.cursor.row < max_row {
                        vs.cursor.row += 1;
                    }
                }
                // Clamp col to the new line length.
                let max_col = self.visual_selection.as_ref().and_then(|vs| {
                    self.codeblock_line_len(vs.block_index, vs.cursor.row)
                }).unwrap_or(1).saturating_sub(1);
                if let Some(vs) = &mut self.visual_selection {
                    vs.cursor.col = vs.cursor.col.min(max_col);
                }
            }
            Action::YankSelection => {
                if let Some(text) = self.extract_visual_selection() {
                    match clipboard::copy_to_clipboard(&text) {
                        Ok(method) => {
                            let msg = if method.is_internal() {
                                "Yanked selection (internal register only)"
                            } else {
                                "Yanked selection"
                            };
                            self.status.status_message = Some(msg.to_string());
                        }
                        Err(e) => {
                            self.status.status_message = Some(format!("Yank failed: {e}"));
                        }
                    }
                }
                // Return to CodeblockSelect mode.
                self.visual_selection = None;
                self.mode = Mode::CodeblockSelect;
                self.status.mode = self.mode;
            }
            Action::EditSelection => {
                if let Some(text) = self.extract_visual_selection() {
                    // Temporarily leave raw mode for the editor.
                    let _ = crossterm::terminal::disable_raw_mode();
                    let _ = crossterm::execute!(
                        io::stdout(),
                        crossterm::event::DisableBracketedPaste
                    );
                    match clipboard::edit_in_editor(&text) {
                        Ok(edited) => {
                            match clipboard::copy_to_clipboard(&edited) {
                                Ok(method) => {
                                    let msg = if method.is_internal() {
                                        "Edited selection saved to internal register only"
                                    } else {
                                        "Edited selection copied to clipboard"
                                    };
                                    self.status.status_message =
                                        Some(msg.to_string());
                                }
                                Err(e) => {
                                    self.status.status_message =
                                        Some(format!("Clipboard error: {e}"));
                                }
                            }
                        }
                        Err(e) => {
                            self.status.status_message =
                                Some(format!("Editor error: {e}"));
                        }
                    }
                    // Restore raw mode.
                    let _ = crossterm::terminal::enable_raw_mode();
                    let _ = crossterm::execute!(
                        io::stdout(),
                        crossterm::event::EnableBracketedPaste
                    );
                }
                // Return to CodeblockSelect mode.
                self.visual_selection = None;
                self.mode = Mode::CodeblockSelect;
                self.status.mode = self.mode;
            }
            Action::SwitchVisualMode => {
                self.mode = match self.mode {
                    Mode::Visual => Mode::VisualLine,
                    Mode::VisualLine => Mode::Visual,
                    other => other,
                };
                self.status.mode = self.mode;
            }
            Action::SwapVisualEnd => {
                if let Some(ref mut vs) = self.visual_selection {
                    vs.swap();
                }
            }

            // -- Phase 5: Favorites --
            Action::CycleFavorite => {
                let favs = self.config.favorites_ordered();
                if !favs.is_empty() {
                    let (_, fav) = &favs[self.favorite_cycle_index % favs.len()];
                    self.provider
                        .switch(&self.config, &fav.provider, &fav.model);
                    self.status.provider = fav.provider.clone();
                    self.status.model = fav.model.clone();
                    if let Some(ref err) = self.provider.build_error {
                        self.status.status_message = Some(format!("Provider error: {err}"));
                    } else {
                        self.status.status_message = Some(format!(
                            "Favorite {}: {}/{}",
                            self.favorite_cycle_index % favs.len() + 1,
                            fav.provider,
                            fav.model
                        ));
                    }
                    self.favorite_cycle_index += 1;
                    self.set_status_timer();
                } else {
                    self.status.status_message = Some("No favorites configured".to_string());
                    self.set_status_timer();
                }
            }
            Action::JumpFavorite(n) => {
                if let Some(fav) = self.config.favorite(n).cloned() {
                    self.provider
                        .switch(&self.config, &fav.provider, &fav.model);
                    self.status.provider = fav.provider.clone();
                    self.status.model = fav.model.clone();
                    if let Some(ref err) = self.provider.build_error {
                        self.status.status_message = Some(format!("Provider error: {err}"));
                    } else {
                        self.status.status_message =
                            Some(format!("Switched to {}/{}", fav.provider, fav.model));
                    }
                    self.set_status_timer();
                } else {
                    self.status.status_message = Some(format!("No favorite #{n}"));
                    self.set_status_timer();
                }
            }

            // -- Phase 5: System prompt --
            Action::ToggleSystemPrompt => {
                if self.system_prompt.is_some() {
                    self.show_system_prompt = !self.show_system_prompt;
                    self.status.status_message = Some(if self.show_system_prompt {
                        "System prompt visible".to_string()
                    } else {
                        "System prompt hidden".to_string()
                    });
                    self.set_status_timer();
                } else {
                    self.status.status_message = Some("No system prompt set (C-x e to edit)".to_string());
                    self.set_status_timer();
                }
            }
            Action::EditSystemPrompt => {
                let current = self.system_prompt.clone().unwrap_or_default();
                // Temporarily leave raw mode for the editor.
                let _ = crossterm::terminal::disable_raw_mode();
                let _ = crossterm::execute!(
                    io::stdout(),
                    crossterm::event::DisableBracketedPaste
                );
                match clipboard::edit_in_editor(&current) {
                    Ok(edited) => {
                        let trimmed = edited.trim().to_string();
                        if trimmed.is_empty() {
                            self.system_prompt = None;
                            self.show_system_prompt = false;
                            self.status.status_message = Some("System prompt cleared".to_string());
                        } else {
                            self.system_prompt = Some(trimmed);
                            self.show_system_prompt = true;
                            self.status.status_message = Some("System prompt updated".to_string());
                        }
                        // Rebuild the client with the new system prompt.
                        self.rebuild_client();
                    }
                    Err(e) => {
                        self.status.status_message = Some(format!("Editor error: {e}"));
                    }
                }
                // Restore raw mode.
                let _ = crossterm::terminal::enable_raw_mode();
                let _ = crossterm::execute!(
                    io::stdout(),
                    crossterm::event::EnableBracketedPaste
                );
                self.set_status_timer();
            }

            // -- Phase 5: Help overlay --
            Action::ShowHelp => {
                self.help_visible = !self.help_visible;
            }

            // -- Phase 5: Search messages --
            Action::SearchMessages => {
                self.open_search();
            }

            // -- Phase 5: History browser --
            Action::OpenHistory => {
                self.open_history();
            }

            Action::None => {}
        }
    }

    /// Send the current input as a user message and start streaming a response.
    fn send_message(&mut self) {
        let text = self.input.text();
        let msg = Message::user(&text);
        self.conversation.push(msg.clone());
        self.message_list
            .push(&msg, self.conceal, &self.highlighter);
        self.input.clear();

        // Spawn the streaming task BEFORE adding the placeholder assistant message,
        // so `to_chat_messages()` doesn't include a trailing empty assistant turn
        // (which can confuse providers into producing an empty response).
        self.spawn_stream();

        // Create a placeholder assistant message for the UI.
        let reply = Message::assistant("");
        self.conversation.push(reply.clone());
        self.message_list
            .push_streaming(&reply, self.conceal, &self.highlighter);

        // Defer scroll until after the viewport has been resized to fit new content.
        self.needs_scroll_bottom = true;
    }

    /// Cancel the active streaming response.
    fn cancel_stream(&mut self) {
        if let Some(cancel) = self.stream_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.status.is_streaming = false;
        // Finalize the partial message.
        if let Some(last) = self.conversation.messages.last() {
            self.message_list
                .finalize_last(last, self.conceal, &self.highlighter);
        }
        self.stream_rx = None;
        self.streaming_content.clear();
    }

    /// Open the model picker overlay.
    fn open_model_picker(&mut self) {
        let mut items: Vec<String> = provider::known_models()
            .into_iter()
            .map(|(p, m)| format!("{p}/{m}"))
            .collect();

        // Add favorites from config.
        for (_, fav) in self.config.favorites_ordered() {
            let entry = format!("{}/{}", fav.provider, fav.model);
            if !items.contains(&entry) {
                items.insert(0, entry);
            }
        }

        // Mark the current model.
        let current = format!("{}/{}", self.provider.provider_name, self.provider.model_name);
        // Move current to front if present.
        if let Some(pos) = items.iter().position(|x| x == &current) {
            items.remove(pos);
        }
        items.insert(0, current);

        self.picker.open(items);
    }

    /// Get the number of lines in the nth codeblock of the selected message.
    fn codeblock_line_count(&self, block_index: usize) -> Option<usize> {
        let msg_idx = self.message_list.selected?;
        let rendered = self.message_list.rendered.get(msg_idx)?;
        let code = crate::ui::codeblock::get_codeblock_content(&rendered.blocks, block_index)?;
        Some(code.lines().count().max(1))
    }

    /// Get the length of a specific line in the nth codeblock of the selected message.
    fn codeblock_line_len(&self, block_index: usize, row: usize) -> Option<usize> {
        let msg_idx = self.message_list.selected?;
        let rendered = self.message_list.rendered.get(msg_idx)?;
        let code = crate::ui::codeblock::get_codeblock_content(&rendered.blocks, block_index)?;
        code.lines().nth(row).map(|line| line.chars().count())
    }

    /// Ensure the current codeblock cursor row is visible in the viewport.
    fn ensure_codeblock_cursor_visible(&mut self) {
        if let (Some(cs), Some(msg_idx)) = (&self.codeblock_select, self.message_list.selected) {
            let vp = self.message_pane_height();
            self.message_list.scroll_codeblock_cursor_into_view(
                msg_idx,
                cs.selected,
                cs.cursor.row,
                vp,
            );
        }
    }

    /// Extract the text covered by the current visual selection.
    fn extract_visual_selection(&self) -> Option<String> {
        let vs = self.visual_selection.as_ref()?;
        let msg_idx = self.message_list.selected?;
        let rendered = self.message_list.rendered.get(msg_idx)?;
        let code =
            crate::ui::codeblock::get_codeblock_content(&rendered.blocks, vs.block_index)?;
        let lines: Vec<&str> = code.lines().collect();

        if lines.is_empty() {
            return None;
        }

        match self.mode {
            Mode::VisualLine => {
                let (start_row, end_row) = vs.row_range();
                let start_row = start_row.min(lines.len() - 1);
                let end_row = end_row.min(lines.len() - 1);
                let selected: Vec<&str> = lines[start_row..=end_row].to_vec();
                Some(selected.join("\n"))
            }
            Mode::Visual => {
                let (start, end) = if (vs.anchor.row, vs.anchor.col) <= (vs.cursor.row, vs.cursor.col) {
                    (vs.anchor, vs.cursor)
                } else {
                    (vs.cursor, vs.anchor)
                };

                // Helper: convert a char-column index to a byte offset in the line.
                let char_to_byte = |line: &str, char_idx: usize| -> usize {
                    line.char_indices()
                        .nth(char_idx)
                        .map(|(byte_idx, _)| byte_idx)
                        .unwrap_or(line.len())
                };

                if start.row == end.row {
                    // Single line selection.
                    let line = lines.get(start.row)?;
                    let char_count = line.chars().count();
                    let s = start.col.min(char_count);
                    let e = (end.col + 1).min(char_count);
                    let s_byte = char_to_byte(line, s);
                    let e_byte = char_to_byte(line, e);
                    Some(line[s_byte..e_byte].to_string())
                } else {
                    // Multi-line selection.
                    let mut result = String::new();
                    for row in start.row..=end.row.min(lines.len() - 1) {
                        let line = lines[row];
                        if row == start.row {
                            let char_count = line.chars().count();
                            let s = start.col.min(char_count);
                            let s_byte = char_to_byte(line, s);
                            result.push_str(&line[s_byte..]);
                        } else if row == end.row {
                            result.push('\n');
                            let char_count = line.chars().count();
                            let e = (end.col + 1).min(char_count);
                            let e_byte = char_to_byte(line, e);
                            result.push_str(&line[..e_byte]);
                        } else {
                            result.push('\n');
                            result.push_str(line);
                        }
                    }
                    Some(result)
                }
            }
            _ => None,
        }
    }

    fn re_render_messages(&mut self) {
        self.message_list
            .render_all(&self.conversation.messages, self.conceal, &self.highlighter);
        // If we're currently streaming, restore the incremental parser on the last message.
        if self.status.is_streaming {
            if let Some(last) = self.message_list.rendered.last_mut() {
                let mut inc = crate::ui::markdown::IncrementalParser::new();
                if let Some(msg) = self.conversation.messages.last() {
                    inc.update(&msg.content, self.conceal, &self.highlighter);
                }
                last.incremental = Some(inc);
            }
        }
    }

    /// Set the status message auto-dismiss timer.
    pub fn set_status_timer(&mut self) {
        self.status_message_at = Some(Instant::now());
    }

    /// Ensure the input cursor stays visible within the input viewport.
    /// The input viewport height matches `compute_input_height` in ui/mod.rs: lines.clamp(1, 10).
    fn ensure_input_visible(&mut self) {
        let viewport_h = (self.input.lines.len()).clamp(1, 10);
        self.input.ensure_visible(viewport_h);
    }

    /// Check and auto-dismiss the status message after ~3 seconds.
    fn check_status_dismiss(&mut self) {
        if let Some(at) = self.status_message_at {
            if at.elapsed() > Duration::from_secs(3) {
                self.status.status_message = None;
                self.status_message_at = None;
            }
        }
    }

    /// Rebuild the LLM client (e.g. after system prompt change).
    pub fn rebuild_client(&mut self) {
        let provider_name = self.provider.provider_name.clone();
        let model_name = self.provider.model_name.clone();
        let system = self.system_prompt.as_deref();
        let (client, build_error) = provider::build_client_with_system(
            &self.config,
            &provider_name,
            &model_name,
            system,
        );
        self.provider.client = client;
        self.provider.build_error = build_error;
    }

    /// Open the search overlay, populating it with message previews.
    fn open_search(&mut self) {
        let items: Vec<(usize, String)> = self
            .conversation
            .messages
            .iter()
            .enumerate()
            .map(|(i, msg)| {
                let role_prefix = match msg.role {
                    crate::conversation::Role::User => "user: ",
                    crate::conversation::Role::Assistant => "asst: ",
                    crate::conversation::Role::System => "sys: ",
                };
                let preview: String = msg
                    .content
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(80)
                    .collect();
                (i, format!("{role_prefix}{preview}"))
            })
            .collect();
        self.search.open(&items);
    }

    /// Open the history browser using the picker overlay.
    fn open_history(&mut self) {
        let Some(dir) = Config::conversations_dir() else {
            self.status.status_message = Some("Cannot determine data directory".to_string());
            self.set_status_timer();
            return;
        };
        match Conversation::list(&dir) {
            Ok(paths) => {
                if paths.is_empty() {
                    self.status.status_message = Some("No saved conversations".to_string());
                    self.set_status_timer();
                    return;
                }
                // Build picker items from conversation filenames/metadata.
                let mut items = Vec::new();
                for path in &paths {
                    match Conversation::load(path) {
                        Ok(conv) => {
                            let title = if conv.metadata.title.is_empty() {
                                "(untitled)"
                            } else {
                                &conv.metadata.title
                            };
                            let date = conv.metadata.updated.format("%Y-%m-%d %H:%M");
                            items.push(format!("{date} | {title}"));
                        }
                        Err(_) => {
                            // Include filename as fallback.
                            if let Some(name) = path.file_stem() {
                                items.push(name.to_string_lossy().to_string());
                            }
                        }
                    }
                }
                // Store paths for selection handling.
                self.history_paths = paths;
                self.picker.open(items);
            }
            Err(e) => {
                self.status.status_message = Some(format!("History error: {e}"));
                self.set_status_timer();
            }
        }
    }

    /// Load a conversation from history by index into history_paths.
    fn load_history_conversation(&mut self, index: usize) {
        if let Some(path) = self.history_paths.get(index) {
            match Conversation::load(path) {
                Ok(conv) => {
                    self.conversation = conv;
                    self.message_list = MessageListState::new();
                    self.message_list.render_all(
                        &self.conversation.messages,
                        self.conceal,
                        &self.highlighter,
                    );
                    self.input.clear();
                    self.mode = Mode::Insert;
                    self.status.mode = self.mode;
                    self.status.status_message = Some("Loaded conversation".to_string());
                    self.set_status_timer();
                }
                Err(e) => {
                    self.status.status_message = Some(format!("Load error: {e}"));
                    self.set_status_timer();
                }
            }
        }
    }
}

/// Poll for a terminal event with timeout, running on a blocking thread
/// so we don't block the tokio runtime.
async fn poll_terminal_event(timeout: Duration) -> Option<Result<Event, io::Error>> {
    tokio::task::spawn_blocking(move || {
        match event::poll(timeout) {
            Ok(true) => Some(event::read().map_err(|e| io::Error::new(io::ErrorKind::Other, e))),
            Ok(false) => None, // Timeout.
            Err(e) => Some(Err(io::Error::new(io::ErrorKind::Other, e))),
        }
    })
    .await
    .unwrap_or(None)
}

/// Receive from the stream channel if it exists.
async fn recv_stream_event(
    rx: &mut Option<mpsc::UnboundedReceiver<StreamEvent>>,
) -> Option<StreamEvent> {
    match rx {
        Some(receiver) => receiver.recv().await,
        None => {
            // No active stream; just pend forever (will be cancelled by select!).
            std::future::pending().await
        }
    }
}
