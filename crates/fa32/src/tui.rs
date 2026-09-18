//! The full chrome: fa32 takes over the terminal for a live session.
//!
//! Layout (0-based rows):
//! ```text
//!   0            top bar — session, model, scheme
//!   1..rows-2    chronicle scroll region (the print loop, managed)
//!   rows-2       status bar — trail, tokens, state
//!   rows-1       input row — `you> …`
//! ```
//! The approval gate renders as a modal frame centered over the chronicle.
//!
//! Input runs on a dedicated blocking thread (raw mode gives us bytes, not
//! lines); the async UI task owns the view and re-renders on every key or
//! event. RPC notifications arrive as [`UiEvent`] from the client handler.

use std::io::{self, Write};

use tokio::sync::{mpsc, oneshot};

use super::screen;
use super::style::{self, Ink, Style};

/// Keys the input thread can produce.
#[derive(Debug, Clone)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Esc,
    CtrlC,
    CtrlD,
    Ignored,
}

/// The operator's decision on an approval modal.
#[derive(Debug)]
pub enum ApprovalDecision {
    Approve,
    Deny,
    Edit(serde_json::Value),
}

/// One chain item's human-readable form, for the modal.
#[derive(Debug, Clone)]
pub struct ChainItem {
    pub print: String,
}

/// One tool call, for the chronicle + trail.
#[derive(Debug, Clone)]
pub struct CallItem {
    pub print: String,
    pub room: Option<String>,
    pub root: Option<String>,
}

/// Events the RPC notification handler forwards to the UI task.
#[derive(Debug)]
pub enum UiEvent {
    AgentText(String),
    ToolCall(Vec<CallItem>),
    ToolResult {
        print: String,
        ok: bool,
        ms: u64,
        error: Option<String>,
    },
    Usage {
        prompt_tokens: u64,
        completion_tokens: u64,
    },
    Warning(String),
    Outcome(String),
    Approval {
        chain: Vec<ChainItem>,
        previews: Vec<String>,
        respond: oneshot::Sender<ApprovalDecision>,
    },
}

/// Build the gate's content rows from human-readable chain prints and
/// preview strings. Shared by the line-mode gate and the TUI modal so the
/// veto always speaks the same language.
pub fn approval_rows(chain: &[ChainItem], previews: &[String]) -> Vec<(Ink, String)> {
    let mut rows: Vec<(Ink, String)> = Vec::new();
    for item in chain {
        rows.push((Ink::Text, format!("⚙ {}", item.print)));
    }
    if !previews.is_empty() {
        rows.push((Ink::Faint, "─ detail ─".to_string()));
        for p in previews {
            rows.push((Ink::Dim, format!("  {p}")));
        }
    }
    if rows.is_empty() {
        rows.push((Ink::Dim, "(nothing to show)".to_string()));
    }
    rows
}

/// One logical (unwrapped) chronicle line: runs of ink + text.
#[derive(Debug, Clone, Default)]
struct Line {
    segs: Vec<(Ink, String)>,
}

struct GateModal {
    /// Logical content rows: chain prints, divider, previews.
    rows: Vec<(Ink, String)>,
    /// Cached (cols, rows) -> visible rows. `cap_gate_rows` spills overflow
    /// to a file as a side effect, so it must not run on every render —
    /// only when the modal opens or the terminal is resized.
    capped: Option<(usize, usize, Vec<(Ink, String)>)>,
    chain_len: usize,
    editing: bool,
    edit_error: Option<String>,
    stashed_input: Vec<char>,
    respond: Option<oneshot::Sender<ApprovalDecision>>,
}

/// The screen model. Everything the renderer needs, nothing it doesn't.
pub struct View {
    lines: Vec<Line>,
    wrapped: Vec<Vec<(Ink, String)>>,
    cache_width: usize,
    scroll: usize,
    input: Vec<char>,
    cursor: usize, // char index into input
    /// First visible char of the input row's horizontal viewport.
    /// Self-healing in render: clamped to the cursor and input length.
    input_off: usize,
    history: Vec<String>,
    hist_pos: Option<usize>,
    trail: Option<String>,
    tokens: Option<String>,
    working: bool,
    modal: Option<GateModal>,
    session_id: String,
    model: String,
    scheme: String,
    last_ch: usize, // chronicle height from the last render
}

impl View {
    pub fn new(session_id: String, model: String, scheme: String) -> Self {
        View {
            lines: Vec::new(),
            wrapped: Vec::new(),
            cache_width: 0,
            scroll: 0,
            input: Vec::new(),
            cursor: 0,
            input_off: 0,
            history: Vec::new(),
            hist_pos: None,
            trail: None,
            tokens: None,
            working: false,
            modal: None,
            session_id,
            model,
            scheme,
            last_ch: 20,
        }
    }

    /// Append a logical line; embedded newlines become separate lines.
    /// New content pins the scroll to the bottom.
    fn push_line(&mut self, segs: &[(Ink, &str)]) {
        let mut lines: Vec<Vec<(Ink, String)>> = Vec::new();
        let mut cur: Vec<(Ink, String)> = Vec::new();
        for (ink, text) in segs {
            for chunk in text.split_inclusive('\n') {
                match chunk.strip_suffix('\n') {
                    Some(part) => {
                        push_run(&mut cur, *ink, part);
                        lines.push(std::mem::take(&mut cur));
                    }
                    None => push_run(&mut cur, *ink, chunk),
                }
            }
        }
        lines.push(cur);
        for segs in lines {
            self.lines.push(Line { segs });
        }
        // Bounded memory: the chronicle is a viewport, not an archive.
        if self.lines.len() > 5000 {
            self.lines.drain(0..self.lines.len() - 5000);
        }
        self.scroll = 0;
        self.cache_width = 0; // invalidate the wrap cache
    }

    fn rewrap(&mut self, cols: usize) {
        self.wrapped.clear();
        for line in &self.lines {
            let refs: Vec<(Ink, &str)> = line.segs.iter().map(|(k, s)| (*k, s.as_str())).collect();
            self.wrapped.extend(style::wrap_spans(&refs, cols));
        }
        self.cache_width = cols;
    }

    /// The trail moves with the call, before the call line — same rule as
    /// the line renderer's `trail_line`, minus the paint.
    fn update_trail(&mut self, room: Option<&str>, root: Option<&str>) {
        let room = match room {
            Some(r) => r,
            None => return,
        };
        let root = root.unwrap_or("workspace");
        let chain = if room == "." {
            root.to_string()
        } else {
            format!("{root} › {}", room.replace('/', " › "))
        };
        if self.trail.as_deref() != Some(chain.as_str()) {
            self.trail = Some(chain.clone());
            self.push_line(&[(Ink::Amber, "● "), (Ink::Dim, &chain)]);
        }
    }

    pub fn on_event(&mut self, ev: UiEvent) {
        match ev {
            UiEvent::AgentText(t) => {
                if !t.trim().is_empty() {
                    self.push_line(&[(Ink::Gold, "agent> "), (Ink::Text, &t)]);
                }
            }
            UiEvent::ToolCall(calls) => {
                for c in calls {
                    self.update_trail(c.room.as_deref(), c.root.as_deref());
                    self.push_line(&[(Ink::Amber, "⚙ "), (Ink::Text, &c.print)]);
                }
            }
            UiEvent::ToolResult {
                print,
                ok,
                ms,
                error,
            } => {
                if ok {
                    self.push_line(&[
                        (Ink::Green, "✓ "),
                        (Ink::Text, &print),
                        (Ink::Dim, &format!(" ({ms}ms)")),
                    ]);
                } else {
                    let mut segs = vec![(Ink::Red, "✗ "), (Ink::Red, print.as_str())];
                    if let Some(e) = error.as_deref() {
                        segs.push((Ink::Red, ": "));
                        segs.push((Ink::Red, e));
                    }
                    self.push_line(&segs);
                }
            }
            UiEvent::Usage {
                prompt_tokens,
                completion_tokens,
            } => {
                self.tokens = Some(format!("{prompt_tokens}+{completion_tokens}"));
            }
            UiEvent::Warning(msg) => {
                self.push_line(&[(Ink::Amber, "! "), (Ink::Text, &msg)]);
            }
            UiEvent::Outcome(o) => {
                self.working = false;
                if !o.trim().is_empty() {
                    self.push_line(&[(Ink::Dim, &o)]);
                }
            }
            UiEvent::Approval {
                chain,
                previews,
                respond,
            } => {
                let rows = approval_rows(&chain, &previews);
                self.modal = Some(GateModal {
                    chain_len: chain.len(),
                    rows,
                    capped: None, // built on first draw_modal, then cached
                    editing: false,
                    edit_error: None,
                    stashed_input: std::mem::take(&mut self.input),
                    respond: Some(respond),
                });
                self.cursor = 0;
            }
        }
    }

    /// Take the input line for submission. Returns None when there is
    /// nothing to send (empty, or a turn already in flight).
    pub fn submit_input(&mut self) -> Option<String> {
        if self.input.is_empty() || self.working {
            return None;
        }
        let text: String = self.input.drain(..).collect();
        self.cursor = 0;
        self.hist_pos = None;
        if self.history.last().map(String::as_str) != Some(text.as_str()) {
            self.history.push(text.clone());
        }
        self.push_line(&[(Ink::Amber, "you> "), (Ink::Text, &text)]);
        self.working = true;
        Some(text)
    }

    #[cfg(test)]
    pub fn has_modal(&self) -> bool {
        self.modal.is_some()
    }
}

fn push_run(cur: &mut Vec<(Ink, String)>, ink: Ink, text: &str) {
    if text.is_empty() {
        return;
    }
    match cur.last_mut() {
        Some((k, s)) if *k == ink => s.push_str(text),
        _ => cur.push((ink, text.to_string())),
    }
}

/// What the UI task should do after a key.
#[derive(Debug, PartialEq, Eq)]
pub enum KeyAction {
    None,
    Redraw,
    Submit(String),
    Quit,
}

/// Shared line-editing keys, used by the prompt row and the modal's
/// edit-args row.
fn edit_key(view: &mut View, key: Key) -> KeyAction {
    match key {
        Key::Char(c) => {
            view.input.insert(view.cursor, c);
            view.cursor += 1;
            KeyAction::Redraw
        }
        Key::Backspace => {
            if view.cursor > 0 {
                view.cursor -= 1;
                view.input.remove(view.cursor);
            }
            KeyAction::Redraw
        }
        Key::Delete => {
            if view.cursor < view.input.len() {
                view.input.remove(view.cursor);
            }
            KeyAction::Redraw
        }
        Key::Left => {
            view.cursor = view.cursor.saturating_sub(1);
            KeyAction::Redraw
        }
        Key::Right => {
            view.cursor = (view.cursor + 1).min(view.input.len());
            KeyAction::Redraw
        }
        Key::Home => {
            view.cursor = 0;
            KeyAction::Redraw
        }
        Key::End => {
            view.cursor = view.input.len();
            KeyAction::Redraw
        }
        _ => KeyAction::None,
    }
}

fn history_back(view: &mut View) {
    if view.history.is_empty() {
        return;
    }
    let next = match view.hist_pos {
        None => view.history.len() - 1,
        Some(0) => 0,
        Some(i) => i - 1,
    };
    view.hist_pos = Some(next);
    view.input = view.history[next].chars().collect();
    view.cursor = view.input.len();
}

fn history_fwd(view: &mut View) {
    match view.hist_pos {
        None => {}
        Some(i) if i + 1 >= view.history.len() => {
            view.hist_pos = None;
            view.input.clear();
            view.cursor = 0;
        }
        Some(i) => {
            view.hist_pos = Some(i + 1);
            view.input = view.history[i + 1].chars().collect();
            view.cursor = view.input.len();
        }
    }
}

fn resolve_modal(view: &mut View, decision: ApprovalDecision) -> KeyAction {
    if let Some(m) = view.modal.take() {
        view.input = m.stashed_input;
        view.cursor = view.input.len();
        if let Some(tx) = m.respond {
            let _ = tx.send(decision);
        }
    }
    KeyAction::Redraw
}

fn handle_modal_key(view: &mut View, key: Key) -> KeyAction {
    let editing = view.modal.as_ref().is_some_and(|m| m.editing);
    if editing {
        match key {
            Key::Enter => {
                let text: String = view.input.drain(..).collect();
                view.cursor = 0;
                match serde_json::from_str::<serde_json::Value>(&text) {
                    Ok(v) if v.is_object() => resolve_modal(view, ApprovalDecision::Edit(v)),
                    _ => {
                        if let Some(m) = view.modal.as_mut() {
                            m.edit_error =
                                Some("not a JSON object — fix it or press Esc".to_string());
                        }
                        KeyAction::Redraw
                    }
                }
            }
            Key::Esc => {
                if let Some(m) = view.modal.as_mut() {
                    m.editing = false;
                    m.edit_error = None;
                }
                view.input.clear();
                view.cursor = 0;
                KeyAction::Redraw
            }
            _ => edit_key(view, key),
        }
    } else {
        match key {
            Key::Char('a') | Key::Char('A') => resolve_modal(view, ApprovalDecision::Approve),
            Key::Char('d') | Key::Char('D') => resolve_modal(view, ApprovalDecision::Deny),
            Key::Esc => resolve_modal(view, ApprovalDecision::Deny),
            Key::Char('e') | Key::Char('E') => {
                let single = view.modal.as_ref().is_some_and(|m| m.chain_len == 1);
                if single {
                    if let Some(m) = view.modal.as_mut() {
                        m.editing = true;
                        m.edit_error = None;
                    }
                    view.input.clear();
                    view.cursor = 0;
                }
                KeyAction::Redraw
            }
            _ => KeyAction::None,
        }
    }
}

pub fn handle_key(view: &mut View, key: Key) -> KeyAction {
    if view.modal.is_some() {
        return handle_modal_key(view, key);
    }
    match key {
        Key::Enter => match view.submit_input() {
            Some(text) => KeyAction::Submit(text),
            None => KeyAction::None,
        },
        Key::Up => {
            history_back(view);
            KeyAction::Redraw
        }
        Key::Down => {
            history_fwd(view);
            KeyAction::Redraw
        }
        Key::PageUp => {
            view.scroll = view.scroll.saturating_add(view.last_ch.max(1) / 2);
            KeyAction::Redraw
        }
        Key::PageDown => {
            view.scroll = view.scroll.saturating_sub(view.last_ch.max(1) / 2);
            KeyAction::Redraw
        }
        Key::Esc => {
            if view.input.is_empty() {
                KeyAction::None
            } else {
                view.input.clear();
                view.cursor = 0;
                KeyAction::Redraw
            }
        }
        Key::CtrlC => {
            if view.input.is_empty() {
                KeyAction::Quit
            } else {
                view.input.clear();
                view.cursor = 0;
                KeyAction::Redraw
            }
        }
        Key::CtrlD => KeyAction::Quit,
        Key::Ignored => KeyAction::None,
        _ => edit_key(view, key),
    }
}

/// Draw the whole screen: top bar, chronicle, status bar, input row, and the
/// approval modal when one is open.
pub fn render(view: &mut View, style: &Style, out: &mut impl Write) -> io::Result<()> {
    let (cols, rows) = style::term_size();
    if view.cache_width != cols {
        view.rewrap(cols);
    }
    let ch = rows.saturating_sub(3); // top bar + status bar + input row
    view.last_ch = ch;

    // Top bar: session on the left, model/scheme on the right, exactly one
    // row. The session side truncates first (from its left, keeping the
    // session id's tail); the whole bar is hard-clipped as a guarantee.
    screen::goto(0, 0, out)?;
    let left = format!("◈ factoragent · {}", view.session_id);
    let right = format!("{} · {}", view.model, view.scheme);
    let right_w = style::disp_width(&right);
    let budget = cols.saturating_sub(right_w + 2);
    let left_vis = {
        let chars: Vec<char> = left.chars().collect();
        let total: usize = chars.iter().map(|c| style::char_width(*c)).sum();
        if total <= budget {
            left
        } else {
            let mut w = 1; // room for the … mark
            let mut n = 0;
            for c in chars.iter().rev() {
                let cw = style::char_width(*c);
                if w + cw > budget {
                    break;
                }
                w += cw;
                n += 1;
            }
            format!("…{}", chars[chars.len() - n..].iter().collect::<String>())
        }
    };
    let gap = cols.saturating_sub(style::disp_width(&left_vis) + right_w);
    let bar = vec![
        (Ink::Dim, left_vis),
        (Ink::Dim, " ".repeat(gap)),
        (Ink::Dim, right),
    ];
    for (ink, text) in clip_spans(&bar, cols) {
        write!(out, "{}", style.paint(ink, &text))?;
    }
    screen::clear_eol(out)?;

    // Chronicle.
    let total = view.wrapped.len();
    let end = total.saturating_sub(view.scroll);
    let start = end.saturating_sub(ch);
    for (i, line) in view.wrapped.iter().enumerate().take(end).skip(start) {
        screen::goto(0, 1 + i - start, out)?;
        for (ink, text) in line {
            write!(out, "{}", style.paint(*ink, text))?;
        }
        screen::clear_eol(out)?;
    }
    // Blank rows the chronicle no longer covers (shrink / scroll artifacts).
    for i in (end - start)..ch {
        screen::goto(0, 1 + i, out)?;
        screen::clear_eol(out)?;
    }

    // Status bar: one row, clipped — a long trail never wraps into input.
    screen::goto(0, rows.saturating_sub(2), out)?;
    let status = clip_spans(&status_line(view), cols);
    for (ink, text) in &status {
        write!(out, "{}", style.paint(*ink, text))?;
    }
    screen::clear_eol(out)?;

    // Input row: exactly one terminal row, always. A cursor-following
    // horizontal viewport slides the visible window so the cursor stays on
    // screen no matter how long the line gets.
    screen::goto(0, rows.saturating_sub(1), out)?;
    let prompt = if view.modal.as_ref().is_some_and(|m| m.editing) {
        "args JSON> "
    } else {
        "you> "
    };
    write!(out, "{}", style.paint(Ink::Amber, prompt))?;
    let prompt_w = style::disp_width(prompt);
    let avail = cols.saturating_sub(prompt_w).max(1);
    let cursor = view.cursor.min(view.input.len());
    let (off, end) = input_viewport(&view.input, cursor, view.input_off, avail);
    view.input_off = off;
    let visible_input: String = view.input[off..end].iter().collect();
    write!(out, "{}", style.paint(Ink::Text, &visible_input))?;
    screen::clear_eol(out)?;
    let cursor_col = prompt_w
        + view.input[off..cursor]
            .iter()
            .map(|c| style::char_width(*c))
            .sum::<usize>();

    // The approval modal, centered over the chronicle.
    if let Some(mut modal) = view.modal.take() {
        draw_modal(&mut modal, style, cols, rows, out)?;
        view.modal = Some(modal);
        screen::hide_cursor(out)?;
    } else {
        screen::show_cursor(out)?;
        screen::goto(cursor_col, rows.saturating_sub(1), out)?;
    }
    out.flush()
}

/// Cursor-following horizontal viewport for the input row.
/// Returns (offset, end) char indices: the visible slice keeps the cursor
/// on screen and never exceeds `avail` display columns. Pure and tested.
fn input_viewport(input: &[char], cursor: usize, off: usize, avail: usize) -> (usize, usize) {
    let avail = avail.max(1);
    let cursor = cursor.min(input.len());
    // Self-healing offset: never past the cursor or the input end.
    let mut off = off.min(cursor);
    while off < cursor {
        let w: usize = input[off..cursor]
            .iter()
            .map(|c| style::char_width(*c))
            .sum();
        if w < avail {
            break;
        }
        off += 1;
    }
    let mut end = off;
    let mut w = 0;
    while end < input.len() {
        let cw = style::char_width(input[end]);
        if w + cw > avail {
            break;
        }
        w += cw;
        end += 1;
    }
    (off, end)
}
/// Clip inked spans to `cols` display columns without cutting UTF-8.
/// The top and status bars are single terminal rows; they must never wrap
/// into their neighbors no matter how long the trail or token readout gets.
fn clip_spans(spans: &[(Ink, String)], cols: usize) -> Vec<(Ink, String)> {
    let mut out = Vec::new();
    let mut w = 0;
    for (ink, text) in spans {
        if w >= cols {
            break;
        }
        let mut kept = String::new();
        let mut cut = false;
        for c in text.chars() {
            let cw = style::char_width(c);
            if w + cw > cols {
                cut = true;
                break;
            }
            w += cw;
            kept.push(c);
        }
        if !kept.is_empty() {
            out.push((*ink, kept));
        }
        if cut || w >= cols {
            break;
        }
    }
    out
}

fn status_line(view: &View) -> Vec<(Ink, String)> {
    let mut segs: Vec<(Ink, String)> = vec![(Ink::Amber, "● ".to_string())];
    segs.push((
        Ink::Dim,
        view.trail.clone().unwrap_or_else(|| "—".to_string()),
    ));
    if let Some(tok) = &view.tokens {
        segs.push((Ink::Dim, format!("   tok {tok}")));
    }
    segs.push((
        if view.working { Ink::Amber } else { Ink::Dim },
        if view.working {
            "   working…".to_string()
        } else {
            "   idle".to_string()
        },
    ));
    if view.scroll > 0 {
        segs.push((Ink::Dim, "   ▲ scrolled".to_string()));
    }
    segs
}

/// The gate as a modal: same content preparation as the line-mode gate
/// (wrap, height cap, spill), framed and centered on the canvas.
fn draw_modal(
    modal: &mut GateModal,
    style: &Style,
    cols: usize,
    rows: usize,
    out: &mut impl Write,
) -> io::Result<()> {
    // Capped rows (and the overflow spill file) are prepared once per
    // terminal size — never once per render, or an overflowing modal would
    // mint a new spill file on every keystroke.
    let stale = modal
        .capped
        .as_ref()
        .map_or(true, |(c, r, _)| *c != cols || *r != rows);
    if stale {
        modal.capped = Some((cols, rows, super::cap_gate_rows(&modal.rows, cols, rows)));
    }
    let mut visible = modal
        .capped
        .as_ref()
        .map(|(_, _, v)| v.clone())
        .unwrap_or_default();
    // Tiny terminals: the modal must never cover the status/input rows.
    let max_box_h = rows.saturating_sub(2).max(6);
    if visible.len() + 3 > max_box_h {
        visible.truncate(max_box_h.saturating_sub(3));
    }
    // Footer: decision keys, or the edit-args state.
    let footer: (Ink, String) = if modal.editing {
        match &modal.edit_error {
            Some(e) => (Ink::Red, format!("  {e}")),
            None => (Ink::Dim, "  Enter to submit · Esc to cancel".to_string()),
        }
    } else if modal.chain_len == 1 {
        (Ink::Dim, "  [a]pprove · [d]eny · [e]dit args".to_string())
    } else {
        (Ink::Dim, "  [a]pprove · [d]eny".to_string())
    };
    let box_h = visible.len() + 3; // top + footer + bottom
    let y0 = rows.saturating_sub(box_h) / 2;
    screen::goto(0, y0, out)?;
    write!(out, "{}", super::gate_top(style, cols))?;
    screen::clear_eol(out)?;
    for (i, (ink, line)) in visible.iter().enumerate() {
        screen::goto(0, y0 + 1 + i, out)?;
        write!(out, "{}", super::gate_row(style, *ink, line, cols))?;
        screen::clear_eol(out)?;
    }
    screen::goto(0, y0 + 1 + visible.len(), out)?;
    write!(out, "{}", super::gate_row(style, footer.0, &footer.1, cols))?;
    screen::clear_eol(out)?;
    screen::goto(0, y0 + 2 + visible.len(), out)?;
    write!(out, "{}", super::gate_bottom(style, cols))?;
    screen::clear_eol(out)?;
    Ok(())
}

/// The input thread: blocking byte reads from stdin, parsed into keys.
/// Ends when the UI task drops the channel.
pub fn spawn_input_thread(tx: mpsc::Sender<Key>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(key) = read_key() {
            if tx.blocking_send(key).is_err() {
                break;
            }
        }
    })
}

#[cfg(unix)]
fn read_byte() -> io::Result<u8> {
    let mut b = [0u8; 1];
    loop {
        // SAFETY: reading one byte from our own stdin into a valid buffer.
        let r = unsafe { libc::read(libc::STDIN_FILENO, b.as_mut_ptr() as *mut libc::c_void, 1) };
        if r == 1 {
            return Ok(b[0]);
        }
        if r < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        return Err(io::Error::other("stdin closed"));
    }
}

#[cfg(unix)]
fn stdin_ready(ms: i32) -> io::Result<bool> {
    // SAFETY: polling our own stdin with a valid pollfd.
    unsafe {
        let mut pfd = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let r = libc::poll(&mut pfd, 1, ms);
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(r > 0)
    }
}

#[cfg(unix)]
fn read_key() -> io::Result<Key> {
    let b = read_byte()?;
    match b {
        b'\r' | b'\n' => Ok(Key::Enter),
        0x7f | 0x08 => Ok(Key::Backspace),
        0x03 => Ok(Key::CtrlC),
        0x04 => Ok(Key::CtrlD),
        0x1b => {
            // Lone Esc vs. an escape sequence: wait briefly for more bytes.
            if !stdin_ready(50)? {
                return Ok(Key::Esc);
            }
            match read_byte()? {
                b'[' => match read_byte()? {
                    b'A' => Ok(Key::Up),
                    b'B' => Ok(Key::Down),
                    b'C' => Ok(Key::Right),
                    b'D' => Ok(Key::Left),
                    b'H' => Ok(Key::Home),
                    b'F' => Ok(Key::End),
                    d @ b'1'..=b'8' => {
                        // CSI n ~ : 3 delete, 5 pgup, 6 pgdn, 7 home, 8 end.
                        let mut seq = vec![d];
                        loop {
                            let c = read_byte()?;
                            seq.push(c);
                            if c == b'~' || !c.is_ascii_digit() {
                                break;
                            }
                        }
                        match seq.as_slice() {
                            [b'3', b'~'] => Ok(Key::Delete),
                            [b'5', b'~'] => Ok(Key::PageUp),
                            [b'6', b'~'] => Ok(Key::PageDown),
                            [b'7', b'~'] => Ok(Key::Home),
                            [b'8', b'~'] => Ok(Key::End),
                            _ => Ok(Key::Ignored),
                        }
                    }
                    _ => Ok(Key::Ignored),
                },
                b'O' => match read_byte()? {
                    b'H' => Ok(Key::Home),
                    b'F' => Ok(Key::End),
                    _ => Ok(Key::Ignored),
                },
                _ => Ok(Key::Esc), // Alt+key: Esc for v1
            }
        }
        c if c < 0x20 => Ok(Key::Ignored),
        _ => {
            // UTF-8 lead byte tells us how many bytes follow.
            let len = if b < 0x80 {
                1
            } else if b < 0xE0 {
                2
            } else if b < 0xF0 {
                3
            } else {
                4
            };
            let mut buf = vec![b];
            for _ in 1..len {
                buf.push(read_byte()?);
            }
            match std::str::from_utf8(&buf) {
                Ok(s) => Ok(s.chars().next().map(Key::Char).unwrap_or(Key::Ignored)),
                Err(_) => Ok(Key::Ignored),
            }
        }
    }
}

#[cfg(windows)]
fn read_key() -> io::Result<Key> {
    use windows_sys::Win32::System::Console::*;
    // SAFETY: reading our own console input; union fields accessed in unsafe.
    unsafe {
        let hin = GetStdHandle(STD_INPUT_HANDLE);
        // Virtual-key codes (stable Win32 values, kept local to avoid
        // another windows-sys feature).
        const VK_PRIOR: u16 = 0x21;
        const VK_NEXT: u16 = 0x22;
        const VK_END: u16 = 0x23;
        const VK_HOME: u16 = 0x24;
        const VK_LEFT: u16 = 0x25;
        const VK_UP: u16 = 0x26;
        const VK_RIGHT: u16 = 0x27;
        const VK_DOWN: u16 = 0x28;
        const VK_DELETE: u16 = 0x2E;
        const VK_RETURN: u16 = 0x0D;
        const VK_BACK: u16 = 0x08;
        const VK_ESCAPE: u16 = 0x1B;
        const LEFT_ALT_PRESSED: u32 = 0x0002;
        const RIGHT_ALT_PRESSED: u32 = 0x0001;
        loop {
            let mut rec: INPUT_RECORD = std::mem::zeroed();
            let mut n = 0u32;
            if ReadConsoleInputW(hin, &mut rec, 1, &mut n) == 0 {
                return Err(io::Error::last_os_error());
            }
            if rec.EventType as u32 != KEY_EVENT {
                continue;
            }
            let ke = rec.Event.KeyEvent;
            if ke.bKeyDown == 0 {
                continue;
            }
            match ke.wVirtualKeyCode {
                VK_LEFT => return Ok(Key::Left),
                VK_RIGHT => return Ok(Key::Right),
                VK_UP => return Ok(Key::Up),
                VK_DOWN => return Ok(Key::Down),
                VK_PRIOR => return Ok(Key::PageUp),
                VK_NEXT => return Ok(Key::PageDown),
                VK_HOME => return Ok(Key::Home),
                VK_END => return Ok(Key::End),
                VK_DELETE => return Ok(Key::Delete),
                VK_RETURN => return Ok(Key::Enter),
                VK_BACK => return Ok(Key::Backspace),
                VK_ESCAPE => return Ok(Key::Esc),
                _ => {}
            }
            // Alt held: skip (v1).
            if ke.dwControlKeyState & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED) != 0 {
                continue;
            }
            let ch = ke.uChar.UnicodeChar;
            if ch == 0 {
                continue;
            }
            let ch = char::from_u32(ch as u32).unwrap_or('\u{FFFD}');
            return Ok(match ch {
                '\x03' => Key::CtrlC,
                '\x04' => Key::CtrlD,
                '\r' | '\n' => Key::Enter,
                '\x08' | '\x7f' => Key::Backspace,
                '\x1b' => Key::Esc,
                c if (c as u32) < 0x20 => Key::Ignored,
                c => Key::Char(c),
            });
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn read_key() -> io::Result<Key> {
    Err(io::Error::other("key input not supported on this platform"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View::new("sess-1".into(), "model".into(), "ember".into())
    }

    #[test]
    fn submit_takes_input_and_marks_working() {
        let mut v = view();
        for c in "hi".chars() {
            assert_eq!(handle_key(&mut v, Key::Char(c)), KeyAction::Redraw);
        }
        assert_eq!(
            handle_key(&mut v, Key::Enter),
            KeyAction::Submit("hi".into())
        );
        // Empty submit is a no-op.
        assert_eq!(handle_key(&mut v, Key::Enter), KeyAction::None);
    }

    #[test]
    fn editing_keys_move_and_delete() {
        let mut v = view();
        for c in "abc".chars() {
            handle_key(&mut v, Key::Char(c));
        }
        handle_key(&mut v, Key::Left);
        handle_key(&mut v, Key::Backspace);
        assert_eq!(
            handle_key(&mut v, Key::Enter),
            KeyAction::Submit("ac".into())
        );
    }

    #[test]
    fn history_cycles() {
        let mut v = view();
        for c in "one".chars() {
            handle_key(&mut v, Key::Char(c));
        }
        handle_key(&mut v, Key::Enter);
        v.on_event(UiEvent::Outcome("done".into()));
        for c in "two".chars() {
            handle_key(&mut v, Key::Char(c));
        }
        handle_key(&mut v, Key::Enter);
        v.on_event(UiEvent::Outcome("done".into()));
        handle_key(&mut v, Key::Up);
        assert_eq!(
            handle_key(&mut v, Key::Enter),
            KeyAction::Submit("two".into())
        );
    }

    #[test]
    fn ctrl_c_clears_then_quits() {
        let mut v = view();
        handle_key(&mut v, Key::Char('x'));
        assert_eq!(handle_key(&mut v, Key::CtrlC), KeyAction::Redraw);
        assert_eq!(handle_key(&mut v, Key::CtrlC), KeyAction::Quit);
    }

    #[test]
    fn modal_stash_restores_unfinished_input() {
        let mut v = view();
        for c in "partial".chars() {
            handle_key(&mut v, Key::Char(c));
        }
        let (tx, _rx) = oneshot::channel();
        v.on_event(UiEvent::Approval {
            chain: vec![ChainItem {
                print: "Run x".into(),
            }],
            previews: vec![],
            respond: tx,
        });
        assert!(v.has_modal());
        assert!(
            v.input.is_empty(),
            "input must be stashed while modal is up"
        );
        assert_eq!(handle_key(&mut v, Key::Char('a')), KeyAction::Redraw);
        assert!(!v.has_modal());
        let typed: String = v.input.iter().collect();
        assert_eq!(typed, "partial", "unfinished prompt must survive approval");
    }

    #[test]
    fn modal_keys_decide() {
        let mut v = view();
        let (tx, mut rx) = oneshot::channel();
        v.on_event(UiEvent::Approval {
            chain: vec![ChainItem {
                print: "Run x".into(),
            }],
            previews: vec![],
            respond: tx,
        });
        assert!(v.has_modal());
        // Typing is swallowed by the modal.
        assert_eq!(handle_key(&mut v, Key::Char('q')), KeyAction::None);
        assert_eq!(handle_key(&mut v, Key::Char('a')), KeyAction::Redraw);
        assert!(!v.has_modal());
        assert!(matches!(rx.try_recv(), Ok(ApprovalDecision::Approve)));
    }

    #[test]
    fn modal_edit_parses_json_object() {
        let mut v = view();
        let (tx, mut rx) = oneshot::channel();
        v.on_event(UiEvent::Approval {
            chain: vec![ChainItem {
                print: "Run x".into(),
            }],
            previews: vec![],
            respond: tx,
        });
        assert_eq!(handle_key(&mut v, Key::Char('e')), KeyAction::Redraw);
        for c in "{\"a\":1}".chars() {
            handle_key(&mut v, Key::Char(c));
        }
        assert_eq!(handle_key(&mut v, Key::Enter), KeyAction::Redraw);
        assert!(!v.has_modal());
        match rx.try_recv() {
            Ok(ApprovalDecision::Edit(val)) => assert_eq!(val["a"], 1),
            other => panic!("expected Edit, got {other:?}"),
        }
    }

    #[test]
    fn trail_updates_on_room_change() {
        let mut v = view();
        v.on_event(UiEvent::ToolCall(vec![CallItem {
            print: "Run x".into(),
            room: Some("crates".into()),
            root: Some("winagent32".into()),
        }]));
        assert_eq!(v.trail.as_deref(), Some("winagent32 › crates"));
        let lines_before = v.lines.len();
        // Same room: no new trail line.
        v.on_event(UiEvent::ToolCall(vec![CallItem {
            print: "Run y".into(),
            room: Some("crates".into()),
            root: Some("winagent32".into()),
        }]));
        assert_eq!(v.lines.len(), lines_before + 1); // only the call line
    }

    #[test]
    fn input_viewport_short_input_shows_all() {
        let input: Vec<char> = "hello".chars().collect();
        let (off, end) = input_viewport(&input, 5, 0, 20);
        assert_eq!((off, end), (0, 5));
    }

    #[test]
    fn input_viewport_long_input_keeps_cursor_visible() {
        let input: Vec<char> = "x".repeat(100).chars().collect();
        // Cursor at the end: window slides left, cursor visible at right edge.
        let (off, end) = input_viewport(&input, 100, 0, 20);
        assert!(off > 0);
        assert!(off <= 100 && 100 <= end);
        let w: usize = input[off..end].iter().map(|c| style::char_width(*c)).sum();
        assert!(w <= 20);
        // Cursor in the middle with a stale offset snaps back into view.
        let (off2, end2) = input_viewport(&input, 10, 90, 20);
        assert!(off2 <= 10 && 10 <= end2);
    }

    #[test]
    fn input_viewport_counts_wide_chars() {
        let input: Vec<char> = "あ".repeat(10).chars().collect(); // 2 cols each
        let (off, end) = input_viewport(&input, 10, 0, 10);
        let w: usize = input[off..end].iter().map(|c| style::char_width(*c)).sum();
        assert!(w <= 10);
        assert!(end - off <= 5); // at most five wide chars fit
    }

    #[test]
    fn clip_spans_never_exceeds_cols_or_cuts_utf8() {
        let spans = vec![
            (Ink::Dim, "● winagent32 › ".to_string()),
            (Ink::Text, "some very long trail …".to_string()),
        ];
        let clipped = clip_spans(&spans, 20);
        let w: usize = clipped
            .iter()
            .flat_map(|(_, t)| t.chars())
            .map(|c| style::char_width(c))
            .sum();
        assert!(w <= 20);
        // Re-encode: no cut UTF-8 (chars() round-trips by construction, but
        // the ellipsis must survive whole or not at all).
        let text: String = clipped.iter().map(|(_, t)| t.as_str()).collect();
        assert!(text.is_char_boundary(text.len()));
    }

    #[test]
    fn clip_spans_empty_and_tiny() {
        let spans = vec![(Ink::Dim, "hello".to_string())];
        assert!(clip_spans(&spans, 0).is_empty());
        let one = clip_spans(&spans, 1);
        let text: String = one.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(text, "h");
    }
}
