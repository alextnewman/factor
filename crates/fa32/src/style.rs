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

/// A text face: semantic ink plus emphasis. Width measurement ignores
/// emphasis — SGR bytes are zero-width — so wrapping and clipping treat
/// a bold run exactly like its plain twin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Face {
    pub ink: Ink,
    pub bold: bool,
}

impl Face {
    pub const fn plain(ink: Ink) -> Self {
        Face { ink, bold: false }
    }

    pub const fn bold(ink: Ink) -> Self {
        Face { ink, bold: true }
    }
}

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
        self.paint_face(Face::plain(ink), text)
    }

    /// Paint `text` in a face (ink + optional bold). Bold is emphasis only:
    /// it never changes display width. Returns `text` unchanged in plain.
    pub fn paint_face(&self, face: Face, text: &str) -> String {
        match self.mode {
            Mode::Plain => text.to_string(),
            Mode::TrueColor => {
                let rgb = match face.ink {
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
                if face.bold {
                    format!("\x1b[1;38;2;{};{};{}m{text}\x1b[0m", rgb.0, rgb.1, rgb.2)
                } else {
                    format!("\x1b[38;2;{};{};{}m{text}\x1b[0m", rgb.0, rgb.1, rgb.2)
                }
            }
            Mode::Native => {
                let code: &str = match face.ink {
                    Ink::Text => return text.to_string(),
                    Ink::Dim | Ink::Faint => "90",
                    Ink::Amber => "33",
                    Ink::Gold => "93",
                    Ink::Green => "32",
                    Ink::Red => "31",
                    Ink::Cyan => "36",
                    Ink::Violet => "35",
                };
                if face.bold {
                    format!("\x1b[1;{code}m{text}\x1b[0m")
                } else {
                    format!("\x1b[{code}m{text}\x1b[0m")
                }
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

/// Terminal size as (columns, rows). Columns clamp to 40–160, rows to
/// 10–60; the probe fails (piped, redirected, dumb terminals) → (80, 24).
/// The clamp keeps the line-mode gate frame sane on absurdly wide windows.
pub fn term_size() -> (usize, usize) {
    match term_size_os() {
        Some((w, h)) => (w.clamp(40, 160), h.clamp(10, 60)),
        None => (80, 24),
    }
}

/// Terminal size without the gate's conservative clamp: the full-screen
/// TUI chrome spans the real window. Floored so tiny terminals stay usable.
pub fn term_size_raw() -> (usize, usize) {
    match term_size_os() {
        Some((w, h)) => (w.max(20), h.max(10)),
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

/// Crude display width for box alignment: most chars are 1 cell; CJK,
/// emoji-presentation ranges, and the ⚙/⚡ symbols are 2. The remaining
/// sigils (✓✗●◆◈▲⬡) render 1 cell in Windows Terminal.
pub fn disp_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// Display width of one char: 2 for wide (CJK) glyphs, 1 otherwise.
pub fn char_width(c: char) -> usize {
    if is_wide(c) {
        2
    } else {
        1
    }
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
        // Emoji-presentation symbols we actually emit: ⚙ (U+2699) and
        // ⚡ (U+26A1) render as 2-cell emoji in Windows Terminal despite
        // being outside the 1F000 block. A 1-cell count here makes any
        // row containing them exactly 1 cell too wide, pushing the
        // gate frame's right border off-screen.
        | '\u{2699}'
        | '\u{26A1}'
    )
}

/// Word-wrap `text` to `max` display columns (greedy, word-based; overlong
/// words are hard-broken). Leading indentation is preserved — the gate shows
/// literal executable content, and indentation is content. Used by the
/// approval gate so complete forms wrap instead of truncating.
pub fn wrap_text(text: &str, max: usize) -> Vec<String> {
    let max = max.max(10);
    // Preserve leading indentation: wrap the body in the remaining width,
    // then re-attach the indent to every produced line.
    let indent_len = text.len() - text.trim_start_matches(' ').len();
    let (pad, body) = text.split_at(indent_len);
    let body_max = max.saturating_sub(disp_width(pad)).max(10);
    let mut lines = wrap_core(body, body_max);
    if pad.is_empty() {
        return lines;
    }
    for line in lines.iter_mut() {
        let mut p = String::with_capacity(pad.len() + line.len());
        p.push_str(pad);
        p.push_str(line);
        *line = p;
    }
    lines
}

/// The word-wrapping core: no leading-whitespace handling. `wrap_text` is
/// the entry point.
fn wrap_core(text: &str, max: usize) -> Vec<String> {
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

/// Word-wrap runs of `(face, text)` to `max` display columns, keeping each
/// character's face across wraps. Returns lines of `(face, text)` runs with
/// adjacent same-face runs merged.
///
/// The full-screen renderer needs this (rather than wrapping painted text)
/// because ANSI escape bytes would corrupt naive width measurement.
/// Embedded newlines are hard breaks; overlong words are hard-split.
pub fn wrap_spans(spans: &[(Face, &str)], max: usize) -> Vec<Vec<(Face, String)>> {
    let max = max.max(1);
    // Split into paragraphs on newlines, keeping per-char face.
    let mut paras: Vec<Vec<(Face, char)>> = vec![Vec::new()];
    for (face, text) in spans {
        for ch in text.chars() {
            if ch == '\n' {
                paras.push(Vec::new());
            } else {
                paras.last_mut().expect("paragraph").push((*face, ch));
            }
        }
    }
    let mut out: Vec<Vec<(Face, String)>> = Vec::new();
    for para in paras {
        // Tokenize into words on spaces.
        let mut words: Vec<Vec<(Face, char)>> = Vec::new();
        let mut cur: Vec<(Face, char)> = Vec::new();
        for (face, ch) in para {
            if ch == ' ' {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
            } else {
                cur.push((face, ch));
            }
        }
        if !cur.is_empty() {
            words.push(cur);
        }
        // Greedy pack; each line is a list of words.
        let mut lines: Vec<Vec<Vec<(Face, char)>>> = Vec::new();
        let mut line: Vec<Vec<(Face, char)>> = Vec::new();
        let mut width = 0usize;
        for word in words {
            if word_width(&word) > max {
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                    width = 0;
                }
                let mut chunks = split_word(&word, max);
                if let Some(last) = chunks.pop() {
                    for c in chunks {
                        lines.push(vec![c]);
                    }
                    width = word_width(&last);
                    line.push(last);
                }
                continue;
            }
            let ww = word_width(&word);
            let need = if line.is_empty() { ww } else { width + 1 + ww };
            if need > max {
                lines.push(std::mem::take(&mut line));
                width = 0;
            }
            if !line.is_empty() {
                width += 1; // the separating space
            }
            width += ww;
            line.push(word);
        }
        if !line.is_empty() || lines.is_empty() {
            lines.push(line);
        }
        // Flatten each line back to merged face runs.
        for line_words in lines {
            let mut chars: Vec<(Face, char)> = Vec::new();
            for (i, word) in line_words.into_iter().enumerate() {
                if i > 0 {
                    let face = chars.last().map(|(k, _)| *k).unwrap_or_else(|| {
                        word.first()
                            .map(|(k, _)| *k)
                            .unwrap_or(Face::plain(Ink::Text))
                    });
                    chars.push((face, ' '));
                }
                chars.extend(word);
            }
            let mut runs: Vec<(Face, String)> = Vec::new();
            for (face, ch) in chars {
                match runs.last_mut() {
                    Some((k, s)) if *k == face => s.push(ch),
                    _ => runs.push((face, ch.to_string())),
                }
            }
            if runs.is_empty() {
                runs.push((Face::plain(Ink::Text), String::new()));
            }
            out.push(runs);
        }
    }
    out
}

fn word_width(word: &[(Face, char)]) -> usize {
    word.iter().map(|(_, c)| char_width(*c)).sum()
}

/// Split a word into chunks each at most `max` display columns.
fn split_word(word: &[(Face, char)], max: usize) -> Vec<Vec<(Face, char)>> {
    let mut chunks = Vec::new();
    let mut cur = Vec::new();
    let mut w = 0usize;
    for &(face, ch) in word {
        let cw = char_width(ch);
        if w + cw > max && !cur.is_empty() {
            chunks.push(std::mem::take(&mut cur));
            w = 0;
        }
        cur.push((face, ch));
        w += cw;
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    chunks
}

/// Split `s` into (head, tail) with head at most `max` display columns.
fn split_at_width(s: &str, max: usize) -> (String, &str) {
    let mut w = 0usize;
    let mut idx = 0usize;
    for (i, c) in s.char_indices() {
        let cw = char_width(c);
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
        assert_eq!(disp_width("⚙ x"), 4); // ⚙ is 2 cells (emoji presentation)
        assert_eq!(disp_width("✓ x"), 3); // ✓ stays 1 cell
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

    #[test]
    fn wrap_text_preserves_leading_indentation() {
        assert_eq!(wrap_text("    indented", 20), vec!["    indented"]);
        // Indented lines still wrap within the width, indent included.
        let lines = wrap_text("    aa bb cc dd ee ff", 14);
        for l in &lines {
            assert!(l.starts_with("    "), "{l:?}");
            assert!(disp_width(l) <= 14, "{l:?}");
        }
        assert!(lines.len() > 1);
    }

    #[test]
    fn wrap_spans_keeps_face_across_wraps() {
        let spans = vec![
            (Face::plain(Ink::Amber), "⚙ "),
            (Face::plain(Ink::Text), "hello world foo"),
        ];
        let lines = wrap_spans(&spans, 10);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            vec![
                (Face::plain(Ink::Amber), "⚙ ".into()),
                (Face::plain(Ink::Text), "hello".into())
            ]
        );
        assert_eq!(lines[1], vec![(Face::plain(Ink::Text), "world foo".into())]);
        // Every rendered line fits the width.
        for line in &lines {
            let painted: String = line.iter().map(|(_, s)| s.as_str()).collect();
            assert!(disp_width(&painted) <= 10, "{painted:?}");
        }
    }

    #[test]
    fn wrap_spans_splits_long_words_and_hard_breaks() {
        let spans = vec![(Face::plain(Ink::Text), "abcdefghij\nklmnopqr")];
        let lines = wrap_spans(&spans, 4);
        let texts: Vec<String> = lines
            .iter()
            .map(|runs| runs.iter().map(|(_, s)| s.as_str()).collect())
            .collect();
        assert_eq!(texts, vec!["abcd", "efgh", "ij", "klmn", "opqr"]);
    }

    #[test]
    fn wrap_spans_empty_is_one_empty_line() {
        let lines = wrap_spans(&[], 20);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], vec![(Face::plain(Ink::Text), String::new())]);
    }

    #[test]
    fn paint_face_bold_emits_sgr_and_ignores_width() {
        let st = Style::truecolor(EMBER);
        let bold = st.paint_face(Face::bold(Ink::Amber), "you> ");
        assert!(bold.starts_with("\x1b[1;38;2;"), "no bold SGR: {bold:?}");
        assert!(bold.ends_with("\x1b[0m"));
        // Plain mode: no bytes at all, bold or not.
        let pl = Style::plain();
        assert_eq!(pl.paint_face(Face::bold(Ink::Amber), "you> "), "you> ");
        // Ghost (native) mode: bold flag, 16-color code.
        let native = Style {
            mode: Mode::Native,
            scheme: EMBER,
        };
        assert_eq!(
            native.paint_face(Face::bold(Ink::Amber), "x"),
            "\x1b[1;33mx\x1b[0m"
        );
        assert_eq!(
            native.paint_face(Face::plain(Ink::Amber), "x"),
            "\x1b[33mx\x1b[0m"
        );
    }
}
