//! Terminal takeover: alternate screen, raw mode, cursor control.
//!
//! When fa32 runs a live session on a real terminal it owns the whole
//! canvas: the alternate screen keeps the operator's scrollback untouched,
//! raw mode gives us a managed input row, and `Drop` restores everything
//! (console modes, cursor, main screen) even on panic.

use std::io::{self, Write};

/// Guard: entering takes over the terminal; dropping gives it back.
pub struct Screen {
    #[cfg(unix)]
    orig_termios: libc::termios,
    /// The fd the termios belongs to (stdin in production; a pty slave in tests).
    #[cfg(unix)]
    term_fd: libc::c_int,
    #[cfg(windows)]
    orig_in_mode: u32,
    #[cfg(windows)]
    orig_out_mode: u32,
}

impl Screen {
    /// Take over the terminal. Fails if stdin/stdout are not usable as a
    /// terminal; callers should fall back to line mode.
    ///
    /// Transactional: raw/console modes are set up *first*, before anything
    /// is drawn. If mode setup fails the terminal is untouched. If the
    /// canvas takeover then fails, the mode guard is dropped explicitly so
    /// the terminal is restored — we never strand the operator in the
    /// alternate screen with a hidden cursor.
    pub fn enter() -> io::Result<Self> {
        Self::enter_with(Self::enable_raw, Self::take_canvas)
    }

    /// The transactional orchestration, with its two steps injectable so
    /// tests can force the canvas step to fail on a real pty.
    fn enter_with(
        enable: impl FnOnce() -> io::Result<Self>,
        take_canvas: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<Self> {
        let screen = enable()?;
        match take_canvas() {
            Ok(()) => Ok(screen),
            Err(e) => {
                drop(screen); // rollback: restores console modes + main screen
                Err(e)
            }
        }
    }

    /// Alternate screen, hidden cursor, cleared canvas.
    fn take_canvas() -> io::Result<()> {
        let mut out = io::stdout();
        write!(out, "\x1b[?1049h\x1b[?25l")?;
        out.flush()?;
        // Clear the fresh canvas so no stale cells show through.
        write!(out, "\x1b[2J")?;
        out.flush()?;
        Ok(())
    }

    #[cfg(unix)]
    fn enable_raw() -> io::Result<Self> {
        // SAFETY: fd 0 is our own stdin.
        Self::enable_raw_fd(unsafe { libc::STDIN_FILENO })
    }

    /// Raw mode on an explicit fd. Production passes stdin; tests pass a
    /// pty slave so the rollback path is exercisable deterministically.
    #[cfg(unix)]
    fn enable_raw_fd(fd: libc::c_int) -> io::Result<Self> {
        // SAFETY: tcgetattr/tcsetattr on our own fd with a valid pointer.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut t) != 0 {
                return Err(io::Error::last_os_error());
            }
            let orig = t;
            // cbreak, no echo, no signal chars (we read Ctrl+C as a key),
            // no XON/XOFF, no CR translation, no output post-processing.
            t.c_iflag &= !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
            t.c_oflag &= !(libc::OPOST);
            t.c_cflag |= libc::CS8;
            t.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
            t.c_cc[libc::VMIN as usize] = 1;
            t.c_cc[libc::VTIME as usize] = 0;
            if libc::tcsetattr(fd, libc::TCSAFLUSH, &t) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Screen {
                orig_termios: orig,
                term_fd: fd,
            })
        }
    }

    #[cfg(windows)]
    fn enable_raw() -> io::Result<Self> {
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::Console::*;
        // SAFETY: console mode flags on our own std handles.
        unsafe {
            let hin = GetStdHandle(STD_INPUT_HANDLE);
            let hout = GetStdHandle(STD_OUTPUT_HANDLE);
            if hin == INVALID_HANDLE_VALUE || hout == INVALID_HANDLE_VALUE {
                return Err(io::Error::other("no console handles"));
            }
            let mut imode = 0u32;
            let mut omode = 0u32;
            if GetConsoleMode(hin, &mut imode) == 0 {
                return Err(io::Error::last_os_error());
            }
            if GetConsoleMode(hout, &mut omode) == 0 {
                return Err(io::Error::last_os_error());
            }
            // Input: no line buffering, no echo, Ctrl+C arrives as a key
            // event instead of killing us, VT input sequences on.
            // Output: VT processing so our ANSI renders on conhost too.
            let new_in = (imode | ENABLE_VIRTUAL_TERMINAL_INPUT)
                & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT);
            let new_out = omode | ENABLE_VIRTUAL_TERMINAL_PROCESSING;
            if SetConsoleMode(hin, new_in) == 0 {
                return Err(io::Error::last_os_error());
            }
            if SetConsoleMode(hout, new_out) == 0 {
                // Roll back the input mode we already changed.
                SetConsoleMode(hin, imode);
                return Err(io::Error::last_os_error());
            }
            Ok(Screen {
                orig_in_mode: imode,
                orig_out_mode: omode,
            })
        }
    }

    #[cfg(not(any(unix, windows)))]
    fn enable_raw() -> io::Result<Self> {
        Err(io::Error::other(
            "terminal takeover not supported on this platform",
        ))
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        #[cfg(unix)]
        // SAFETY: restoring our own saved termios on its own fd.
        unsafe {
            libc::tcsetattr(self.term_fd, libc::TCSANOW, &self.orig_termios);
        }
        #[cfg(windows)]
        // SAFETY: restoring our own saved console modes.
        unsafe {
            use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
            use windows_sys::Win32::System::Console::*;
            let hin = GetStdHandle(STD_INPUT_HANDLE);
            let hout = GetStdHandle(STD_OUTPUT_HANDLE);
            if hin != INVALID_HANDLE_VALUE {
                SetConsoleMode(hin, self.orig_in_mode);
            }
            if hout != INVALID_HANDLE_VALUE {
                SetConsoleMode(hout, self.orig_out_mode);
            }
        }
        let mut out = io::stdout();
        // Show cursor, leave the alternate screen, back to the operator's world.
        let _ = write!(out, "\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
    }
}

/// Move to a 0-based (col, row); rows/cols are 1-based on the wire.
pub fn goto(col: usize, row: usize, out: &mut impl Write) -> io::Result<()> {
    write!(out, "\x1b[{};{}H", row + 1, col + 1)
}

pub fn hide_cursor(out: &mut impl Write) -> io::Result<()> {
    write!(out, "\x1b[?25l")
}

pub fn show_cursor(out: &mut impl Write) -> io::Result<()> {
    write!(out, "\x1b[?25h")
}

/// Clear from the cursor to the end of the current line.
pub fn clear_eol(out: &mut impl Write) -> io::Result<()> {
    write!(out, "\x1b[K")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A failing canvas takeover must roll back the raw mode that was
    /// already applied: the pty's termios must be byte-identical to before.
    #[test]
    fn failed_canvas_takeover_restores_termios() {
        unsafe {
            let mut master: libc::c_int = -1;
            let mut slave: libc::c_int = -1;
            assert_eq!(
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut()
                ),
                0
            );
            let mut before: libc::termios = std::mem::zeroed();
            assert_eq!(libc::tcgetattr(slave, &mut before), 0);

            let res = Screen::enter_with(
                || Screen::enable_raw_fd(slave),
                || Err(io::Error::new(io::ErrorKind::Other, "canvas boom")),
            );
            match res {
                Err(e) => assert_eq!(e.kind(), io::ErrorKind::Other),
                Ok(_) => panic!("canvas failure must propagate"),
            }

            // Rollback happened: every termios field matches the snapshot.
            let mut after: libc::termios = std::mem::zeroed();
            assert_eq!(libc::tcgetattr(slave, &mut after), 0);
            let b = termios_bytes(&before);
            let a = termios_bytes(&after);
            assert_eq!(b, a, "termios not restored after failed takeover");

            libc::close(master);
            libc::close(slave);
        }
    }

    unsafe fn termios_bytes(t: &libc::termios) -> Vec<u8> {
        let ptr = t as *const libc::termios as *const u8;
        std::slice::from_raw_parts(ptr, std::mem::size_of::<libc::termios>()).to_vec()
    }
}
