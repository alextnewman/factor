//! The Textual Realist's palette: color schemes, true-color ANSI emission,
//! and honest degradation.
//!
//! Schemes (ember/frost/parchment) emit 24-bit color; ghost emits only the
//! 16 standard ANSI colors, so the operator's own terminal theme shows
//! through — the background-free option. Plain mode emits no escape codes
//! at all (NO_COLOR, dumb terminals, piped output): greppable lines.
//!
//! fa32 never paints a background: every scheme is foreground ink only.

use std::io::IsTerminal;

/// Semantic inks. The full palette is the scheme contract: not every ink
/// is used in every render, so unused variants are expected.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ink {
    Text,
    Dim,
    Faint,
    Amber,
    Green,
    Red,
    Cyan,
    Gold,
    Violet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Debug, Clone, Copy)]
pub struct Scheme {
    pub name: &'static str,
    /// None = the terminal's own default foreground (dark-terminal schemes).
    pub text: Option<Rgb>,
    pub dim: Rgb,
    pub faint: Rgb,
    pub amber: Rgb,
    pub green: Rgb,
    pub red: Rgb,
    pub cyan: Rgb,
    pub gold: Rgb,
    pub violet: Rgb,
}

pub const EMBER: Scheme = Scheme {
    name: "ember",
    text: None,
    dim: Rgb(0x8F, 0x7D, 0x64),
    faint: Rgb(0x5D, 0x50, 0x41),
    amber: Rgb(0xE8, 0xA3, 0x3D),
    green: Rgb(0x9D, 0xB8, 0x6A),
    red: Rgb(0xE0, 0x6C, 0x5B),
    cyan: Rgb(0x6F, 0xB3, 0xB8),
    gold: Rgb(0xD4, 0xAF, 0x6A),
    violet: Rgb(0xB4, 0x8E, 0xC7),
};

pub const FROST: Scheme = Scheme {
    name: "frost",
    text: None,
    dim: Rgb(0x64, 0x78, 0x8C),
    faint: Rgb(0x3D, 0x4A, 0x5C),
    amber: Rgb(0xE0, 0xB4, 0x5C),
    green: Rgb(0x7F, 0xC9, 0x7F),
    red: Rgb(0xE0, 0x6C, 0x75),
    cyan: Rgb(0x6F, 0xC3, 0xDF),
    gold: Rgb(0xA8, 0xC3, 0xE0),
    violet: Rgb(0x9D, 0x8F, 0xD0),
};

pub const PARCHMENT: Scheme = Scheme {
    name: "parchment",
    text: Some(Rgb(0x2B, 0x21, 0x14)),
    dim: Rgb(0x6E, 0x5F, 0x47),
    faint: Rgb(0xA4, 0x91, 0x6F),
    amber: Rgb(0x8A, 0x5A, 0x10),
    green: Rgb(0x3D, 0x63, 0x28),
    red: Rgb(0x9C, 0x2F, 0x24),
    cyan: Rgb(0x1E, 0x6E, 0x75),
    gold: Rgb(0x77, 0x60, 0x1F),
    violet: Rgb(0x6F, 0x4F, 0x86),
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    TrueColor,
    Native,
    Plain,
}

/// A resolved rendering style: how (and whether) to colorize.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    mode: Mode,
    scheme: Scheme,
}

impl Style {
    pub fn plain() -> Self {
        Self {
            mode: Mode::Plain,
            scheme: EMBER,
        }
    }

    pub fn is_plain(&self) -> bool {
        self.mode == Mode::Plain
    }

    /// Resolve the style from flags and environment.
    ///
    /// `color`: "auto" (default), "always", or "never". NO_COLOR (non-empty)
    /// always wins; a non-TTY stdout or TERM=dumb degrades to plain unless
    /// color is "always".
    pub fn detect(scheme_name: Option<&str>, color: &str) -> Result<Self, String> {
        // A misspelled scheme is an error even when the output degrades to
        // plain: the flag was still wrong.
        let name = scheme_name.unwrap_or("ember");
        let ghost = name == "ghost";
        let scheme = if ghost {
            EMBER // palette unused in native mode
        } else {
            match [EMBER, FROST, PARCHMENT].iter().find(|s| s.name == name) {
                Some(s) => *s,
                None => {
                    return Err(format!(
                        "unknown --scheme {name:?}; expected one of: ember, frost, parchment, ghost"
                    ))
                }
            }
        };
        if color == "never" || no_color_env() {
            return Ok(Self::plain());
        }
        if color != "always" && (!std::io::stdout().is_terminal() || dumb_term()) {
            return Ok(Self::plain());
        }
        Ok(if ghost {
            Self {
                mode: Mode::Native,
                scheme,
            }
        } else {
            Self::truecolor(scheme)
        })
    }

    fn truecolor(scheme: Scheme) -> Self {
        Self {
            mode: Mode::TrueColor,
            scheme,
        }
    }

    /// Paint `text` in a semantic ink. Returns `text` unchanged in plain mode.
    pub fn paint(&self, ink: Ink, text: &str) -> String {
        match self.mode {
            Mode::Plain => text.to_string(),
            Mode::TrueColor => {
                let rgb = match ink {
                    Ink::Text => match self.scheme.text {
                        Some(c) => c,
                        None => return text.to_string(),
                    },
                    Ink::Dim => self.scheme.dim,
                    Ink::Faint => self.scheme.faint,
                    Ink::Amber => self.scheme.amber,
                    Ink::Green => self.scheme.green,
                    Ink::Red => self.scheme.red,
                    Ink::Cyan => self.scheme.cyan,
                    Ink::Gold => self.scheme.gold,
                    Ink::Violet => self.scheme.violet,
                };
                format!("\x1b[38;2;{};{};{}m{text}\x1b[0m", rgb.0, rgb.1, rgb.2)
            }
            Mode::Native => {
                let code: &str = match ink {
                    Ink::Text => return text.to_string(),
                    Ink::Dim | Ink::Faint => "90",
                    Ink::Amber => "33",
                    Ink::Gold => "93",
                    Ink::Green => "32",
                    Ink::Red => "31",
                    Ink::Cyan => "36",
                    Ink::Violet => "35",
                };
                format!("\x1b[{code}m{text}\x1b[0m")
            }
        }
    }
}

fn no_color_env() -> bool {
    // NO_COLOR convention: present and non-empty disables color.
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

fn dumb_term() -> bool {
    std::env::var("TERM").is_ok_and(|t| t == "dumb")
}

/// Terminal width in columns for width-aware rendering (the gate frame).
/// Falls back to 80 when undetectable; clamped to a sane range.
/// Terminal size as (columns, rows). Columns clamp to 40–160, rows to
/// 10–60; the probe fails (piped, redirected, dumb terminals) → (80, 24).
pub fn term_size() -> (usize, usize) {
    match term_size_os() {
        Some((w, h)) => (w.clamp(40, 160), h.clamp(10, 60)),
        None => (80, 24),
    }
}

#[cfg(unix)]
fn term_size_os() -> Option<(usize, usize)> {
    // SAFETY: trivial ioctl with a stack winsize; no invariants to uphold.
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
            let h = if ws.ws_row > 0 {
                ws.ws_row as usize
            } else {
                24
            };
            return Some((ws.ws_col as usize, h));
        }
    }
    None
}

#[cfg(windows)]
fn term_size_os() -> Option<(usize, usize)> {
    use windows_sys::Win32::System::Console::{
        GetConsoleScreenBufferInfo, GetStdHandle, CONSOLE_SCREEN_BUFFER_INFO, STD_OUTPUT_HANDLE,
    };
    // SAFETY: handle checked for null; struct written only by the API call.
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if handle.is_null() {
            return None;
        }
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
        if GetConsoleScreenBufferInfo(handle, &mut info) == 0 {
            return None;
        }
        let w = info.srWindow.Right - info.srWindow.Left + 1;
        let h = info.srWindow.Bottom - info.srWindow.Top + 1;
        if w > 0 {
            let h = if h > 0 { h as usize } else { 24 };
            Some((w as usize, h))
        } else {
            None
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn term_size_os() -> Option<(usize, usize)> {
    None
}

/// Crude display width for box alignment: most chars are 1 cell; CJK and
/// emoji-presentation ranges are 2. The sigils we use (⚙✓✗●◆◈▲⬡⚡) render
/// 1 cell in Cascadia/Windows Terminal; terminals that disagree may misalign
/// the gate frame by a cell — accepted for v1.
pub fn disp_width(s: &str) -> usize {
    s.chars().map(|c| if is_wide(c) { 2 } else { 1 }).sum()
}

fn is_wide(c: char) -> bool {
    matches!(c,
        '\u{1100}'..='\u{115F}'
        | '\u{2E80}'..='\u{303E}'
        | '\u{3041}'..='\u{33FF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{AC00}'..='\u{D7A3}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{FE30}'..='\u{FE4F}'
        | '\u{FF00}'..='\u{FF60}'
        | '\u{FFE0}'..='\u{FFE6}'
        | '\u{1F000}'..='\u{1FAFF}'
    )
}

/// Word-wrap `text` to `max` display columns (greedy, word-based; overlong
/// words are hard-broken). Used by the approval gate so complete forms wrap
/// instead of truncating.
pub fn wrap_text(text: &str, max: usize) -> Vec<String> {
    let max = max.max(10);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for word in text.split(' ') {
        // Hard-break a word that exceeds the width on its own.
        let mut rest = word;
        while disp_width(rest) > max {
            let (head, tail) = split_at_width(rest, max);
            if cur_w > 0 {
                lines.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            lines.push(head);
            rest = tail;
        }
        let ww = disp_width(rest);
        if cur_w > 0 && cur_w + 1 + ww > max {
            lines.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        if cur_w > 0 {
            cur.push(' ');
            cur_w += 1;
        }
        cur.push_str(rest);
        cur_w += ww;
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Split `s` into (head, tail) with head at most `max` display columns.
fn split_at_width(s: &str, max: usize) -> (String, &str) {
    let mut w = 0usize;
    let mut idx = 0usize;
    for (i, c) in s.char_indices() {
        let cw = if is_wide(c) { 2 } else { 1 };
        if w + cw > max {
            break;
        }
        w += cw;
        idx = i + c.len_utf8();
    }
    if idx == 0 {
        // A single wide char wider than max: take it whole rather than
        // returning an empty head.
        let c = s.chars().next().unwrap();
        idx = c.len_utf8();
    }
    (s[..idx].to_string(), &s[idx..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_emits_no_escapes() {
        let st = Style::plain();
        for ink in [
            Ink::Text,
            Ink::Dim,
            Ink::Faint,
            Ink::Amber,
            Ink::Green,
            Ink::Red,
            Ink::Cyan,
            Ink::Gold,
            Ink::Violet,
        ] {
            assert_eq!(st.paint(ink, "hi"), "hi");
        }
    }

    #[test]
    fn truecolor_emits_24bit_codes() {
        let st = Style {
            mode: Mode::TrueColor,
            scheme: EMBER,
        };
        assert_eq!(st.paint(Ink::Amber, "x"), "\x1b[38;2;232;163;61mx\x1b[0m");
        // Dark-terminal schemes leave Text unpainted (terminal default).
        assert_eq!(st.paint(Ink::Text, "x"), "x");
    }

    #[test]
    fn parchment_paints_text_dark() {
        let st = Style {
            mode: Mode::TrueColor,
            scheme: PARCHMENT,
        };
        assert_eq!(st.paint(Ink::Text, "x"), "\x1b[38;2;43;33;20mx\x1b[0m");
    }

    #[test]
    fn native_uses_16color_codes() {
        let st = Style {
            mode: Mode::Native,
            scheme: EMBER,
        };
        assert_eq!(st.paint(Ink::Amber, "x"), "\x1b[33mx\x1b[0m");
        assert_eq!(st.paint(Ink::Text, "x"), "x");
    }

    #[test]
    fn unknown_scheme_is_an_error() {
        assert!(Style::detect(Some("neon"), "always").is_err());
    }

    #[test]
    fn ghost_selects_native_mode() {
        let st = Style::detect(Some("ghost"), "always").unwrap();
        assert_eq!(st.mode, Mode::Native);
    }

    #[test]
    fn disp_width_counts_cjk_double() {
        assert_eq!(disp_width("ab"), 2);
        assert_eq!(disp_width("⚙ x"), 3); // sigils count 1 here
        assert_eq!(disp_width("中文"), 4);
    }

    #[test]
    fn wrap_text_wraps_words_and_breaks_long_ones() {
        assert_eq!(wrap_text("a b c", 10), vec!["a b c"]);
        assert_eq!(wrap_text("aa bb cc dd ee", 10), vec!["aa bb cc", "dd ee"]);
        assert_eq!(wrap_text("abcdefghijklm", 10), vec!["abcdefghij", "klm"]);
        assert_eq!(wrap_text("", 10), vec![""]);
        // Widths below the 10-column floor are treated as 10.
        assert_eq!(wrap_text("aa bb", 4), vec!["aa bb"]);
    }
}
