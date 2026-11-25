//! An agent's program in a pseudo-terminal (ConPTY on Windows), for apps that draw the
//! terminal themselves and leave running the program to us (the Windows app, with xterm.js).
//! What it prints is collected for `session.poll`, which hands it to the app.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use familiar_agent::Launch;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// What the program has printed since the last poll, and whether it has exited.
#[derive(Default)]
pub struct TtyOutput {
    pub bytes: Vec<u8>,
    pub exited: bool,
}

pub struct Tty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

impl Tty {
    /// Starts the program. Its output goes to `output`, and `wake` is called whenever there is
    /// some, or when it exits.
    pub fn spawn(launch: &Launch, cols: u16, rows: u16, output: Arc<Mutex<TtyOutput>>, wake: impl Fn() + Send + Clone + 'static) -> Result<Self, String> {
        let pair = native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string())?;
        let mut command = CommandBuilder::new(&launch.program);
        command.args(&launch.args);
        command.cwd(&launch.dir);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        for (key, value) in &launch.env {
            command.env(key, value);
        }
        let mut child = pair.slave.spawn_command(command).map_err(|e| e.to_string())?;
        // The program has its end; ours closing it lets the reader see the end on Unix.
        drop(pair.slave);
        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

        let (out, woken) = (output.clone(), wake.clone());
        std::thread::Builder::new()
            .name("familiar-agent-out".into())
            .spawn(move || {
                let mut buf = [0u8; 16 * 1024];
                // Ends when the terminal is closed (on Windows ConPTY only closes when we drop it).
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    out.lock().unwrap().bytes.extend_from_slice(&buf[..n]);
                    woken();
                }
            })
            .map_err(|e| e.to_string())?;
        std::thread::Builder::new()
            .name("familiar-agent-wait".into())
            .spawn(move || {
                let _ = child.wait();
                output.lock().unwrap().exited = true;
                wake();
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { master: pair.master, writer, killer })
    }

    /// Types into the program.
    pub fn write(&mut self, data: &[u8]) {
        let _ = self.writer.write_all(data);
        let _ = self.writer.flush();
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
    }
}

impl Drop for Tty {
    /// Stops the program, if it's still running, and closes its terminal.
    fn drop(&mut self) {
        let _ = self.killer.kill();
    }
}

/// Bytes as base64, for JSON.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_as_the_browser_reads_it() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe, 0x00]), "//4A");
    }

    #[cfg(unix)]
    #[test]
    fn a_program_runs_in_a_terminal() {
        let launch = Launch {
            name: "test".into(),
            token: String::new(),
            url: String::new(),
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "read line; echo \"got $line in $FAMILIAR_WORLD\"; stty size".into()],
            dir: std::env::temp_dir(),
            env: vec![("FAMILIAR_WORLD".into(), "Vale".into())],
        };
        let output = Arc::new(Mutex::new(TtyOutput::default()));
        let mut tty = Tty::spawn(&launch, 100, 30, output.clone(), || {}).unwrap();
        tty.write(b"hello\r");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let text = String::from_utf8_lossy(&output.lock().unwrap().bytes).into_owned();
            if text.contains("30 100") && output.lock().unwrap().exited {
                assert!(text.contains("got hello in Vale"), "{text}");
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{text}");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
