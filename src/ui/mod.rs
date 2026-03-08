pub mod codeblock;
pub mod input;
pub mod markdown;
pub mod messages;
pub mod picker;
pub mod search;
pub mod status;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use self::input::InputWidget;
use self::messages::MessageListWidget;
use self::picker::PickerWidget;
use self::search::SearchWidget;
use self::status::StatusBarWidget;
use crate::app::App;
use crate::mode::Mode;

/// Main render function. Draws the entire UI into the given area.
pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    if area.height < 3 {
        return;
    }

    // Layout: messages (flexible) | separator | input (min 1, grows) | status bar (1).
    let input_height = compute_input_height(app, area.width);
    let chunks = Layout::vertical([
        Constraint::Min(1),               // messages
        Constraint::Length(1),            // separator
        Constraint::Length(input_height), // input
        Constraint::Length(1),            // status bar
    ])
    .split(area);

    // Messages.
    let msg_widget = MessageListWidget {
        state: &app.message_list,
        mode: app.mode,
        codeblock_select: app.codeblock_select.as_ref(),
        visual_selection: app.visual_selection.as_ref(),
    };
    msg_widget.render(chunks[0], buf);

    // Separator line with key hints.
    render_separator(chunks[1], buf, app.mode, app.status.leader_pending);

    // Input area.
    let input_widget = InputWidget {
        state: &app.input,
        active: app.mode.is_insert(),
    };
    input_widget.render(chunks[2], buf);

    // Status bar.
    let status_widget = StatusBarWidget { state: &app.status };
    status_widget.render(chunks[3], buf);

    // Picker overlay (renders on top of everything if visible).
    if app.picker.visible {
        let picker_widget = PickerWidget { state: &app.picker };
        picker_widget.render(area, buf);
    }

    // Search overlay (renders on top of everything if visible).
    if app.search.visible {
        let search_widget = SearchWidget { state: &app.search };
        search_widget.render(area, buf);
    }

    // Help overlay (renders on top of everything if visible).
    if app.help_visible {
        render_help_overlay(area, buf);
    }
}

fn render_separator(area: Rect, buf: &mut Buffer, mode: Mode, leader_pending: bool) {
    let sep_style = Style::default().fg(Color::Rgb(60, 60, 70));

    // Fill with separator chars first.
    for x in area.x..area.x + area.width {
        buf.set_string(x, area.y, "\u{2500}", sep_style); // ─
    }

    // Choose hints based on state.
    let hints: &[(&str, &str)] = if leader_pending {
        &[
            ("c", "conceal"),
            ("m", "model"),
            ("h", "history"),
            ("s", "save"),
            ("n", "new"),
            ("q", "quit"),
            ("?", "help"),
        ]
    } else {
        match mode {
            Mode::Insert => &[
                ("Enter", "send"),
                ("S/A-Ret", "newline"),
                ("C-p/n", "browse"),
                ("C-x", "leader"),
            ],
            Mode::Browse => &[
                ("j/k", "scroll"),
                ("C-p/n", "navigate"),
                ("Tab", "codeblocks"),
                ("/", "search"),
                ("Esc", "back"),
            ],
            Mode::CodeblockSelect => &[
                ("Tab", "switch"),
                ("y", "yank"),
                ("v/V", "visual"),
                ("Esc", "back"),
                ("C-x", "leader"),
            ],
            Mode::Visual => &[
                ("hjkl", "select"),
                ("o", "swap end"),
                ("y", "yank"),
                ("v/V", "switch"),
                ("Esc", "back"),
            ],
            Mode::VisualLine => &[
                ("j/k", "select"),
                ("o", "swap end"),
                ("y", "yank"),
                ("e", "edit"),
                ("Esc", "back"),
            ],
        }
    };

    // Compute total width needed: each hint is " key desc " with double-space between.
    // Format: "  key desc  key desc  key desc  "
    let total_hint_width: usize = hints
        .iter()
        .map(|(k, d)| k.len() + 1 + d.len()) // "key desc"
        .sum::<usize>()
        + hints.len().saturating_sub(1) * 2 // "  " between hints
        + 2; // leading/trailing space

    let avail = area.width as usize;
    if total_hint_width > avail || avail < 10 {
        return; // Not enough space for hints, just show the separator line.
    }

    // Center the hints in the separator row.
    let start_x = area.x + ((avail - total_hint_width) / 2) as u16;

    let key_style = Style::default().fg(Color::Rgb(200, 180, 120));
    let desc_style = Style::default().fg(Color::Rgb(100, 100, 110));

    let mut x = start_x + 1; // leading space
    for (i, (key, desc)) in hints.iter().enumerate() {
        if i > 0 {
            // Double-space separator between hints.
            buf.set_string(x, area.y, "  ", sep_style);
            x += 2;
        }
        buf.set_string(x, area.y, *key, key_style);
        x += key.len() as u16;
        buf.set_string(x, area.y, " ", sep_style);
        x += 1;
        buf.set_string(x, area.y, *desc, desc_style);
        x += desc.len() as u16;
    }
}

fn compute_input_height(app: &App, _width: u16) -> u16 {
    // Grow with input content, min 1, max 10.
    let lines = app.input.lines.len() as u16;
    lines.clamp(1, 10)
}

/// Render the help overlay showing all keybindings.
fn render_help_overlay(area: Rect, buf: &mut Buffer) {
    if area.height < 6 || area.width < 20 {
        return;
    }

    let bindings = [
        (
            "Global (C-x + key)",
            vec![
                ("C-x c", "Toggle conceal level"),
                ("C-x m", "Model picker"),
                ("C-x h", "History browser"),
                ("C-x s", "Save conversation"),
                ("C-x n", "New conversation"),
                ("C-x q", "Quit"),
                ("C-x f", "Cycle favorite models"),
                ("C-x 1-9", "Jump to favorite #N"),
                ("C-x t", "Toggle system prompt"),
                ("C-x e", "Edit system prompt"),
                ("C-x ?", "This help"),
            ],
        ),
        (
            "Insert mode",
            vec![
                ("Enter", "Send message"),
                ("Shift+Enter", "New line"),
                ("Alt+Enter", "New line (fallback)"),
                ("C-p / C-n", "Browse messages"),
                ("C-c", "Cancel stream / clear"),
                ("C-w", "Delete word backward"),
                ("C-u", "Delete to line start"),
                ("C-a / C-e", "Home / End"),
                ("Alt+j / Alt+k", "Scroll input view"),
            ],
        ),
        (
            "Browse mode",
            vec![
                ("j/k", "Scroll line up/down"),
                ("C-p / C-n", "Prev/next message"),
                ("g/G", "First/last message"),
                ("C-d/C-u", "Scroll half page down/up"),
                ("Tab", "Enter codeblock select"),
                ("/", "Search messages"),
                ("Esc/Enter/i", "Back to insert"),
            ],
        ),
        (
            "Codeblock select",
            vec![
                ("Tab/S-Tab", "Prev/next codeblock"),
                ("h/j/k/l", "Move cursor in codeblock"),
                ("C-d/C-u", "Scroll half page down/up"),
                ("y", "Yank codeblock"),
                ("e", "Edit in $EDITOR"),
                ("v/V", "Visual / visual line"),
                ("Esc", "Back to browse"),
            ],
        ),
        (
            "Visual modes",
            vec![
                ("h/j/k/l", "Move selection"),
                ("o", "Swap anchor/cursor"),
                ("y", "Yank selection"),
                ("e", "Edit selection (V-LINE)"),
                ("v/V", "Switch visual mode"),
                ("Esc", "Back to codeblock"),
            ],
        ),
    ];

    // Calculate dimensions.
    let mut total_lines = 0;
    for (_section, keys) in &bindings {
        total_lines += 1 + keys.len() + 1; // header + keys + blank
    }
    let max_key_width = bindings
        .iter()
        .flat_map(|(_, keys)| keys.iter().map(|(k, d)| k.len() + d.len() + 4))
        .max()
        .unwrap_or(30);

    let overlay_width = area.width.min((max_key_width as u16 + 4).max(40)).max(30);
    let overlay_height = area.height.min(total_lines as u16 + 2).max(6);
    let x = area.x + (area.width.saturating_sub(overlay_width)) / 2;
    let y = area.y + (area.height.saturating_sub(overlay_height)) / 2;

    let bg = Style::default()
        .bg(Color::Rgb(25, 25, 35))
        .fg(Color::Rgb(200, 200, 210));
    let border_style = Style::default()
        .fg(Color::Rgb(80, 80, 100))
        .bg(Color::Rgb(25, 25, 35));
    let section_style = Style::default()
        .fg(Color::Rgb(100, 180, 255))
        .bg(Color::Rgb(25, 25, 35))
        .add_modifier(Modifier::BOLD);
    let key_style = Style::default()
        .fg(Color::Rgb(255, 220, 100))
        .bg(Color::Rgb(25, 25, 35));
    let desc_style = Style::default()
        .fg(Color::Rgb(180, 180, 190))
        .bg(Color::Rgb(25, 25, 35));

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
    let title = " Help (press any key to close) ";
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

    // Content.
    let mut row = y + 1;
    let max_row = y + overlay_height - 1;

    for (section_name, keys) in &bindings {
        if row >= max_row {
            break;
        }
        // Section header.
        buf.set_string(x + 1, row, section_name, section_style);
        row += 1;

        for (key, desc) in keys {
            if row >= max_row {
                break;
            }
            let key_w = key.len() as u16;
            buf.set_string(x + 2, row, key, key_style);
            buf.set_string(x + 2 + key_w + 1, row, desc, desc_style);
            row += 1;
        }

        // Blank line between sections.
        row += 1;
    }

    // Bottom border.
    if max_row < area.y + area.height {
        let bottom_border: String = std::iter::repeat('\u{2500}').take(border_len).collect();
        buf.set_string(x, max_row, &bottom_border, border_style);
    }
}
