//! Text to speech: speech-dispatcher (SSIP) on Linux, the built-in SAPI voice on Windows, `say`
//! on macOS. Speech runs on a background thread, so a slow or missing speech service never blocks the UI. Shared
//! by the GTK and terminal clients.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

#[cfg(all(unix, not(target_os = "macos")))]
#[path = "ssip.rs"]
mod backend;
#[cfg(target_os = "macos")]
#[path = "say.rs"]
mod backend;
#[cfg(windows)]
#[path = "sapi.rs"]
mod backend;
#[cfg(not(any(unix, windows)))]
#[path = "none.rs"]
mod backend;

use backend::Backend;

enum Command {
    Say(u64, String),
    Stop,
}

pub struct Speaker {
    commands: mpsc::Sender<Command>,
    errors: mpsc::Receiver<String>,
    stops: Arc<AtomicU64>,
}

fn run(app: String, commands: mpsc::Receiver<Command>, errors: mpsc::Sender<String>, stops: Arc<AtomicU64>) {
    let mut backend: Option<Backend> = None;
    // Report a failure once, not for every line.
    let mut reported = false;
    for command in commands {
        match &command {
            Command::Say(seq, _) if *seq != stops.load(Ordering::SeqCst) => continue,
            Command::Stop if backend.is_none() => continue,
            _ => {}
        }
        if backend.is_none() {
            match Backend::connect(&app) {
                Ok(b) => {
                    backend = Some(b);
                    reported = false;
                }
                Err(e) => {
                    if !reported {
                        let _ = errors.send(format!("Speech isn't available ({e}).{}", backend::HINT));
                        reported = true;
                    }
                    continue;
                }
            }
        }
        let b = backend.as_mut().unwrap();
        let result = match command {
            Command::Say(_, text) => b.say(&text),
            Command::Stop => b.stop(),
        };
        if let Err(e) = result {
            // Reconnect next time; the speech service may have restarted.
            backend = None;
            if !reported {
                let _ = errors.send(format!("Speech failed: {e}"));
                reported = true;
            }
        }
    }
}

impl Speaker {
    /// Starts the speech thread. Nothing connects until the first [`Speaker::say`].
    pub fn new(app: &str) -> Self {
        let (commands, commands_rx) = mpsc::channel();
        let (errors_tx, errors) = mpsc::channel();
        let stops = Arc::new(AtomicU64::new(0));
        let thread_stops = stops.clone();
        let app = app.to_string();
        thread::Builder::new()
            .name("mu-speech".into())
            .spawn(move || run(app, commands_rx, errors_tx, thread_stops))
            .expect("failed to start the speech thread");
        Self { commands, errors, stops }
    }

    pub fn say(&self, text: &str) {
        let text = text.trim();
        if !text.is_empty() {
            let seq = self.stops.load(Ordering::SeqCst);
            let _ = self.commands.send(Command::Say(seq, text.to_string()));
        }
    }

    /// Stops speaking and drops anything queued.
    pub fn stop(&self) {
        self.stops.fetch_add(1, Ordering::SeqCst);
        let _ = self.commands.send(Command::Stop);
    }

    /// A problem to show the user, if one happened since the last call.
    pub fn take_error(&self) -> Option<String> {
        self.errors.try_recv().ok()
    }
}
