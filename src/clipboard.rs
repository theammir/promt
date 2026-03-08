/// Clipboard and $EDITOR integration.
///
/// Clipboard strategy depends on the session type:
/// - On Wayland: shell tools (wl-copy) first, then arboard, then internal register.
///   (arboard silently "succeeds" on Wayland but often doesn't actually populate
///   the clipboard, so wl-copy is preferred.)
/// - Otherwise: arboard first, then shell tools, then internal register.
use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// Internal register for when no system clipboard is available.
static INTERNAL_REGISTER: Mutex<Option<String>> = Mutex::new(None);

/// Indicates which method was used to copy text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyMethod {
    /// Native clipboard via arboard.
    Arboard,
    /// Shell clipboard tool (wl-copy, xclip, etc.).
    ShellTool,
    /// Internal register (session-only, no system clipboard).
    InternalRegister,
}

impl CopyMethod {
    /// Whether the copy only went to the internal register (session-only).
    pub fn is_internal(&self) -> bool {
        matches!(self, CopyMethod::InternalRegister)
    }
}

/// Detect if we're running in a Wayland session.
fn is_wayland() -> bool {
    std::env::var("WAYLAND_DISPLAY").is_ok()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland"))
}

/// Copy text to the system clipboard, with fallbacks.
/// Returns the method used on success.
pub fn copy_to_clipboard(text: &str) -> Result<CopyMethod, ClipboardError> {
    if is_wayland() {
        // On Wayland, prefer shell tools (wl-copy) over arboard.
        if try_shell_copy(text) {
            return Ok(CopyMethod::ShellTool);
        }
        if try_arboard_copy(text) {
            return Ok(CopyMethod::Arboard);
        }
    } else {
        // Non-Wayland: arboard first, then shell tools.
        if try_arboard_copy(text) {
            return Ok(CopyMethod::Arboard);
        }
        if try_shell_copy(text) {
            return Ok(CopyMethod::ShellTool);
        }
    }

    // Fall back to internal register.
    if let Ok(mut reg) = INTERNAL_REGISTER.lock() {
        *reg = Some(text.to_string());
        return Ok(CopyMethod::InternalRegister);
    }

    Err(ClipboardError::Copy(
        "all clipboard methods failed".to_string(),
    ))
}

/// Try copying via arboard (native platform clipboard). Returns true on success.
fn try_arboard_copy(text: &str) -> bool {
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        return clipboard.set_text(text).is_ok();
    }
    false
}

/// Try copying via shell clipboard tools. Returns true on success.
fn try_shell_copy(text: &str) -> bool {
    // Ordered by preference: Wayland → X11 → macOS → WSL
    let tools: &[&[&str]] = &[
        &["wl-copy"],
        &["xclip", "-selection", "clipboard"],
        &["xsel", "--clipboard", "--input"],
        &["pbcopy"],
        &["clip.exe"],
    ];

    for tool in tools {
        let cmd = tool[0];
        let args = &tool[1..];
        if let Ok(mut child) = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            if let Some(ref mut stdin) = child.stdin {
                if stdin.write_all(text.as_bytes()).is_ok() {
                    drop(child.stdin.take());
                    if let Ok(status) = child.wait() {
                        if status.success() {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

/// Open text in $EDITOR (or $VISUAL, fallback to vi).
/// Returns the edited text content after the editor exits.
///
/// The caller is responsible for restoring terminal raw mode after this returns,
/// since this function needs cooked mode for the editor to work.
pub fn edit_in_editor(content: &str) -> Result<String, EditorError> {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());

    // Write content to a temp file.
    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join(format!("promt-edit-{}.tmp", std::process::id()));
    std::fs::write(&tmp_path, content).map_err(EditorError::Io)?;

    // Run the editor.
    let status = Command::new(&editor)
        .arg(&tmp_path)
        .status()
        .map_err(|e| EditorError::Launch(editor.clone(), e))?;

    if !status.success() {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(EditorError::ExitStatus(editor, status.code().unwrap_or(-1)));
    }

    // Read back the edited content.
    let result = std::fs::read_to_string(&tmp_path).map_err(EditorError::Io)?;
    let _ = std::fs::remove_file(&tmp_path);
    Ok(result)
}

#[derive(Debug)]
pub enum ClipboardError {
    Copy(String),
}

impl std::fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClipboardError::Copy(e) => write!(f, "clipboard copy failed: {e}"),
        }
    }
}

#[derive(Debug)]
pub enum EditorError {
    Io(io::Error),
    Launch(String, io::Error),
    ExitStatus(String, i32),
}

impl std::fmt::Display for EditorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditorError::Io(e) => write!(f, "editor I/O error: {e}"),
            EditorError::Launch(editor, e) => write!(f, "failed to launch {editor}: {e}"),
            EditorError::ExitStatus(editor, code) => {
                write!(f, "{editor} exited with status {code}")
            }
        }
    }
}
