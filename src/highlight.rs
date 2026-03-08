use ratatui::style::{Color, Modifier, Style};
use syntect::easy::HighlightLines;
use syntect::highlighting::{self, ThemeSet};
use syntect::parsing::SyntaxSet;

/// Wrapper around syntect for syntax highlighting of code blocks.
pub struct Highlighter {
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
    theme_name: String,
}

/// A styled text fragment: the text content and its ratatui style.
#[derive(Debug, Clone)]
pub struct StyledFragment {
    pub text: String,
    pub style: Style,
}

impl Highlighter {
    pub fn new(theme_name: &str) -> Self {
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
            theme_name: theme_name.to_string(),
        }
    }

    /// Highlight a block of code, returning styled fragments per line.
    /// Each inner Vec is one line of styled fragments.
    pub fn highlight(&self, code: &str, language: &str) -> Vec<Vec<StyledFragment>> {
        let syntax = self
            .syntax_set
            .find_syntax_by_token(language)
            .or_else(|| self.syntax_set.find_syntax_by_extension(language))
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        let theme = self
            .theme_set
            .themes
            .get(&self.theme_name)
            .or_else(|| self.theme_set.themes.values().next())
            .expect("no themes available");

        let mut h = HighlightLines::new(syntax, theme);
        let mut result = Vec::new();

        for line in code.lines() {
            let ranges = h.highlight_line(line, &self.syntax_set).unwrap_or_default();
            let fragments: Vec<StyledFragment> = ranges
                .into_iter()
                .map(
                    |(style, text): (highlighting::Style, &str)| StyledFragment {
                        text: text.to_string(),
                        style: convert_style(&style),
                    },
                )
                .collect();
            result.push(fragments);
        }

        result
    }
}

/// Convert a syntect style to a ratatui style.
fn convert_style(style: &highlighting::Style) -> Style {
    let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);

    let mut ratatui_style = Style::default().fg(fg);

    if style.font_style.contains(highlighting::FontStyle::BOLD) {
        ratatui_style = ratatui_style.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(highlighting::FontStyle::ITALIC) {
        ratatui_style = ratatui_style.add_modifier(Modifier::ITALIC);
    }
    if style
        .font_style
        .contains(highlighting::FontStyle::UNDERLINE)
    {
        ratatui_style = ratatui_style.add_modifier(Modifier::UNDERLINED);
    }

    ratatui_style
}
