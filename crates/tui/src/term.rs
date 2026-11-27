//! A program running on a pseudo-terminal with its screen kept by a terminal emulator, so an
//! editor can live in a tab instead of taking over the whole terminal.

use std::io::{Read, Write};

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use tokio::sync::mpsc::UnboundedSender;
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

pub enum PtyEvent {
    Output(Vec<u8>),
    /// The program ended; true if it reported success.
    Exited(bool),
}

/// Events from the programs, each with the id it was started with.
pub type PtySender = UnboundedSender<(u64, PtyEvent)>;

/// Answers to what programs ask the terminal, waiting to be sent back to them. Editors ask at
/// startup (which colours, what kind of terminal) and some wait a while for the answers.
#[derive(Default)]
struct Replies(Vec<u8>);

impl vt100::Callbacks for Replies {
    fn unhandled_csi(&mut self, screen: &mut vt100::Screen, i1: Option<u8>, _i2: Option<u8>, params: &[&[u16]], c: char) {
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        let reply = match (i1, c, first) {
            // Device status: fine, and where the cursor is.
            (None, 'n', 5) => "\x1b[0n".to_string(),
            (None, 'n', 6) => {
                let (row, col) = screen.cursor_position();
                format!("\x1b[{};{}R", row + 1, col + 1)
            }
            // What kind of terminal: a VT220 with colour, and a version for those who ask.
            (None, 'c', 0) => "\x1b[?62;22c".to_string(),
            (Some(b'>'), 'c', 0) => "\x1b[>1;10;0c".to_string(),
            _ => return,
        };
        self.0.extend_from_slice(reply.as_bytes());
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        // The default colours. The real ones aren't known here; the client's look is light on dark.
        let color = match params {
            [b"10", b"?"] => "c0c0/c0c0/c0c0",
            [b"11", b"?"] => "0000/0000/0000",
            _ => return,
        };
        self.0.extend_from_slice(format!("\x1b]{};rgb:{color}\x1b\\", String::from_utf8_lossy(params[0])).as_bytes());
    }
}

pub struct Term {
    parser: vt100::Parser<Replies>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// Columns and rows.
    size: (u16, u16),
}

fn pty_size((cols, rows): (u16, u16)) -> PtySize {
    PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }
}

fn nonzero(n: u16) -> std::num::NonZeroU16 {
    std::num::NonZeroU16::new(n).unwrap_or(std::num::NonZeroU16::MIN)
}

impl Term {
    /// Runs `command` on a new pseudo-terminal of `size` (columns, rows). Its output and exit
    /// arrive on `tx`, tagged with `id`.
    pub fn spawn(command: &std::process::Command, size: (u16, u16), id: u64, tx: PtySender) -> Result<Self, String> {
        let size = (size.0.max(2), size.1.max(2));
        let pair = native_pty_system().openpty(pty_size(size)).map_err(|e| e.to_string())?;
        let mut builder = CommandBuilder::new(command.get_program());
        builder.args(command.get_args());
        match command.get_current_dir() {
            Some(dir) => builder.cwd(dir),
            None => {
                if let Ok(dir) = std::env::current_dir() {
                    builder.cwd(dir);
                }
            }
        }
        for (name, value) in command.get_envs() {
            match value {
                Some(value) => builder.env(name, value),
                None => builder.env_remove(name),
            }
        }
        // What the emulator here understands, whatever the real terminal is.
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");
        for name in ["TERM_PROGRAM", "TERM_PROGRAM_VERSION", "KITTY_WINDOW_ID", "TMUX", "TMUX_PANE"] {
            builder.env_remove(name);
        }
        let mut child = pair.slave.spawn_command(builder).map_err(|e| e.to_string())?;
        drop(pair.slave);
        let mut killer = child.clone_killer();
        let mut reader = match pair.master.try_clone_reader() {
            Ok(r) => r,
            Err(e) => {
                let _ = killer.kill();
                return Err(e.to_string());
            }
        };
        let writer = match pair.master.take_writer() {
            Ok(w) => w,
            Err(e) => {
                let _ = killer.kill();
                return Err(e.to_string());
            }
        };

        let output = tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        if output.send((id, PtyEvent::Output(buf[..n].to_vec()))).is_err() {
                            return;
                        }
                    }
                    _ => return,
                }
            }
        });
        // Reading doesn't end when the program does on every system, so wait for it as well.
        std::thread::spawn(move || {
            let ok = child.wait().is_ok_and(|status| status.success());
            let _ = tx.send((id, PtyEvent::Exited(ok)));
        });
        Ok(Self { parser: vt100::Parser::new_with_callbacks(nonzero(size.1), nonzero(size.0), 0, Replies::default()), writer, master: pair.master, killer, size })
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        let replies = std::mem::take(&mut self.parser.callbacks_mut().0);
        if !replies.is_empty() {
            self.send(&replies);
        }
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let size = (cols.max(2), rows.max(2));
        if size != self.size {
            self.size = size;
            self.parser.screen_mut().set_size(nonzero(size.1), nonzero(size.0));
            let _ = self.master.resize(pty_size(size));
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn key(&mut self, key: KeyEvent) {
        let bytes = encode_key(key, self.screen().application_cursor());
        self.send(&bytes);
    }

    pub fn paste(&mut self, text: &str) {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if self.screen().bracketed_paste() {
            // Strip bracketed-paste delimiters so pasted text cannot break out of paste mode.
            let safe = text.replace("\x1b[200~", "").replace("\x1b[201~", "");
            self.send(format!("\x1b[200~{safe}\x1b[201~").as_bytes());
        } else {
            self.send(text.as_bytes());
        }
    }

    /// A mouse event at `col`, `row` of the program's screen.
    pub fn mouse(&mut self, event: MouseEvent, col: u16, row: u16) {
        let screen = self.screen();
        let (mode, encoding) = (screen.mouse_protocol_mode(), screen.mouse_protocol_encoding());
        let wheel = matches!(event.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown);
        if mode == MouseProtocolMode::None {
            // Like terminals do for full-screen programs that don't take the mouse: the wheel
            // moves the cursor.
            if wheel && screen.alternate_screen() {
                let code = if event.kind == MouseEventKind::ScrollUp { KeyCode::Up } else { KeyCode::Down };
                let bytes = encode_key(KeyEvent::new(code, KeyModifiers::NONE), screen.application_cursor()).repeat(3);
                self.send(&bytes);
            }
            return;
        }
        let button = |b: MouseButton| match b {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
        };
        let (mut code, release) = match event.kind {
            MouseEventKind::Down(b) => (button(b), false),
            MouseEventKind::Up(b) if mode != MouseProtocolMode::Press => (button(b), true),
            MouseEventKind::Drag(b) if matches!(mode, MouseProtocolMode::ButtonMotion | MouseProtocolMode::AnyMotion) => (button(b) + 32, false),
            MouseEventKind::Moved if mode == MouseProtocolMode::AnyMotion => (35, false),
            MouseEventKind::ScrollUp => (64, false),
            MouseEventKind::ScrollDown => (65, false),
            _ => return,
        };
        for (modifier, bit) in [(KeyModifiers::SHIFT, 4), (KeyModifiers::ALT, 8), (KeyModifiers::CONTROL, 16)] {
            if event.modifiers.contains(modifier) {
                code += bit;
            }
        }
        let bytes = match encoding {
            MouseProtocolEncoding::Sgr => format!("\x1b[<{code};{};{}{}", col + 1, row + 1, if release { 'm' } else { 'M' }).into_bytes(),
            // The older encodings: a byte each, which runs out at column 223.
            _ if col < 223 && row < 223 => {
                let code = if release { 3 | (code & !3) } else { code };
                vec![0x1b, b'[', b'M', code as u8 + 32, col as u8 + 33, row as u8 + 33]
            }
            _ => return,
        };
        self.send(&bytes);
    }

    pub fn kill(&mut self) {
        let _ = self.killer.kill();
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        self.kill();
    }
}

/// A key as the bytes an xterm would send for it. `application_cursor` is the mode programs set
/// to get `ESC O A` rather than `ESC [ A` for the arrow keys.
pub fn encode_key(key: KeyEvent, application_cursor: bool) -> Vec<u8> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let modifier = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    // ESC [ 1 ; modifier letter, or the short forms without modifiers.
    let letter = |c: char, ss3: bool| match (modifier, ss3) {
        (1, true) => format!("\x1bO{c}"),
        (1, false) => format!("\x1b[{c}"),
        _ => format!("\x1b[1;{modifier}{c}"),
    };
    let tilde = |n: u8| if modifier == 1 { format!("\x1b[{n}~") } else { format!("\x1b[{n};{modifier}~") };
    let escaped = |bytes: &[u8]| {
        let mut out = if alt { vec![0x1b] } else { Vec::new() };
        out.extend_from_slice(bytes);
        out
    };
    let text = match key.code {
        KeyCode::Char(c) => {
            let control = match c {
                _ if !ctrl => None,
                'a'..='z' | 'A'..='Z' => Some(c as u8 & 0x1f),
                '@' | ' ' | '2' => Some(0),
                '[' | '3' => Some(0x1b),
                '\\' | '4' => Some(0x1c),
                ']' | '5' => Some(0x1d),
                '^' | '6' => Some(0x1e),
                '_' | '-' | '/' | '7' => Some(0x1f),
                '?' | '8' => Some(0x7f),
                _ => None,
            };
            return match control {
                Some(byte) => escaped(&[byte]),
                None => escaped(c.encode_utf8(&mut [0; 4]).as_bytes()),
            };
        }
        KeyCode::Enter => return escaped(b"\r"),
        KeyCode::Tab => return escaped(b"\t"),
        KeyCode::Esc => return escaped(b"\x1b"),
        KeyCode::Backspace => return escaped(if ctrl { b"\x08" } else { b"\x7f" }),
        KeyCode::BackTab => "\x1b[Z".to_string(),
        KeyCode::Up => letter('A', application_cursor),
        KeyCode::Down => letter('B', application_cursor),
        KeyCode::Right => letter('C', application_cursor),
        KeyCode::Left => letter('D', application_cursor),
        KeyCode::Home => letter('H', application_cursor),
        KeyCode::End => letter('F', application_cursor),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => letter(['P', 'Q', 'R', 'S'][n as usize - 1], true),
        KeyCode::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][n as usize - 5]),
        _ => String::new(),
    };
    text.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(code: KeyCode, modifiers: KeyModifiers) -> String {
        String::from_utf8(encode_key(KeyEvent::new(code, modifiers), false)).unwrap()
    }

    #[test]
    fn keys_as_xterm_sends_them() {
        let none = KeyModifiers::NONE;
        assert_eq!(enc(KeyCode::Char('é'), none), "é");
        assert_eq!(enc(KeyCode::Char('c'), KeyModifiers::CONTROL), "\x03");
        assert_eq!(enc(KeyCode::Char('['), KeyModifiers::CONTROL), "\x1b");
        assert_eq!(enc(KeyCode::Char('x'), KeyModifiers::ALT), "\x1bx");
        assert_eq!(enc(KeyCode::Enter, none), "\r");
        assert_eq!(enc(KeyCode::Backspace, none), "\x7f");
        assert_eq!(enc(KeyCode::Up, none), "\x1b[A");
        assert_eq!(enc(KeyCode::Right, KeyModifiers::CONTROL), "\x1b[1;5C");
        assert_eq!(enc(KeyCode::Delete, none), "\x1b[3~");
        assert_eq!(enc(KeyCode::PageUp, KeyModifiers::SHIFT), "\x1b[5;2~");
        assert_eq!(enc(KeyCode::F(1), none), "\x1bOP");
        assert_eq!(enc(KeyCode::F(5), none), "\x1b[15~");
        assert_eq!(encode_key(KeyEvent::new(KeyCode::Up, none), true), b"\x1bOA", "application cursor keys");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_a_program_and_keeps_its_screen() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut command = std::process::Command::new("sh");
        // It asks the terminal's status, then reads a line: the answer plus what's typed.
        command.args(["-c", "printf 'hello \\033[1mworld\\033[0m\\033[5n'; read -r line; printf \"\\r\\ngot ${#line}\"; exit 3"]);
        let mut term = Term::spawn(&command, (40, 5), 7, tx).unwrap();
        let mut typed = false;
        let mut exit = None;
        while let Ok(Some((id, event))) = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await {
            assert_eq!(id, 7);
            match event {
                PtyEvent::Output(bytes) => term.feed(&bytes),
                PtyEvent::Exited(ok) => exit = Some(ok),
            }
            if !typed && term.screen().contents().contains("hello world") {
                typed = true;
                term.paste("abc\n");
            }
            if exit.is_some() && term.screen().contents().contains("got ") {
                break;
            }
        }
        assert_eq!(exit, Some(false), "a failing exit is reported");
        let screen = term.screen();
        assert!(screen.rows(0, 40).next().unwrap().starts_with("hello world"), "{:?}", screen.contents());
        assert!(screen.cell(0, 6).unwrap().bold() && !screen.cell(0, 0).unwrap().bold());
        // ESC [ 0 n (the answer to its question) and then "abc".
        assert!(screen.contents().contains("got 7"), "{:?}", screen.contents());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn passes_the_commands_environment_and_folder() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let dir = std::env::temp_dir();
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "printf \"$FAMILIAR_TEST_VAR $(pwd)\""]).env("FAMILIAR_TEST_VAR", "here").current_dir(&dir);
        let mut term = Term::spawn(&command, (200, 5), 1, tx).unwrap();
        let mut exited = false;
        while let Ok(Some((_, event))) = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await {
            match event {
                PtyEvent::Output(bytes) => term.feed(&bytes),
                PtyEvent::Exited(_) => exited = true,
            }
            if exited && term.screen().contents().contains("here") {
                break;
            }
        }
        let contents = term.screen().contents();
        let expected = dir.canonicalize().unwrap();
        let name = expected.file_name().unwrap().to_string_lossy();
        assert!(contents.starts_with("here /") && contents.trim_end().ends_with(&*name), "{contents:?}");
    }
}
