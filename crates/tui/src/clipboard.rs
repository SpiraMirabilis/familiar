//! Putting text on the system clipboard.

use std::io::Write;
use std::process::{Command, Stdio};

/// Copies `text`: by asking the terminal (OSC 52, which also works over ssh), and through the
/// desktop's own tool where there is one, since not every terminal (or tmux setup) honours OSC 52.
pub fn copy(text: &str) {
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let _ = out.flush();

    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let x11 = std::env::var_os("DISPLAY").is_some();
    let tools: &[(&str, &[&str], bool)] = &[
        ("pbcopy", &[], cfg!(target_os = "macos")),
        ("wl-copy", &[], cfg!(unix) && wayland),
        ("xclip", &["-selection", "clipboard"], cfg!(unix) && x11),
        ("xsel", &["--clipboard", "--input"], cfg!(unix) && x11),
    ];
    for (tool, args, _) in tools.iter().filter(|t| t.2) {
        let child = Command::new(tool).args(*args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        if let Ok(mut child) = child {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            // Some of these stay around to serve the clipboard; don't wait for them here.
            std::thread::spawn(move || child.wait());
            return;
        }
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, b| n << 8 | *b as u32) << (8 * (3 - chunk.len()));
        for i in 0..4 {
            out.push(if i <= chunk.len() { ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_pads() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64("é\n".as_bytes()), "w6kK");
    }
}
