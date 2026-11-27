//! macOS backend: the system's `say` command, one process per message, so nothing is linked.

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

pub const HINT: &str = "";

/// Messages wait their turn on a thread of their own, so queueing one never blocks and "stop"
/// can interrupt the one being spoken.
pub struct Backend {
    /// Each message with the number of stops there had been when it was queued.
    queue: mpsc::Sender<(u64, String)>,
    stops: Arc<AtomicU64>,
    speaking: Arc<Mutex<Option<Child>>>,
}

fn spawn_say() -> std::io::Result<(Child, Option<std::process::ChildStdin>)> {
    // The text goes in on stdin: as an argument, a line starting with `-` would be an option.
    let mut child = Command::new("say").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    let stdin = child.stdin.take();
    Ok((child, stdin))
}

fn run(queue: mpsc::Receiver<(u64, String)>, stops: Arc<AtomicU64>, speaking: Arc<Mutex<Option<Child>>>) {
    for (queued_at, text) in queue {
        let stdin = {
            // Checked with the lock held, so a stop either sees this message's process or
            // prevents it from starting.
            let mut slot = speaking.lock().unwrap_or_else(|e| e.into_inner());
            if stops.load(Ordering::SeqCst) != queued_at {
                continue;
            }
            match spawn_say() {
                Ok((child, stdin)) => {
                    *slot = Some(child);
                    stdin
                }
                Err(_) => continue,
            }
        };
        // Write stdin outside the lock so `stop()` can kill `say` even if text exceeds the pipe buffer.
        if let Some(mut stdin) = stdin {
            let _ = stdin.write_all(text.as_bytes());
        }
        // Polling rather than wait(): the lock has to be free for stop() to kill the process.
        loop {
            thread::sleep(Duration::from_millis(30));
            let mut slot = speaking.lock().unwrap_or_else(|e| e.into_inner());
            match slot.as_mut().map(Child::try_wait) {
                Some(Ok(None)) => {}
                _ => {
                    *slot = None;
                    break;
                }
            }
        }
    }
}

impl Backend {
    pub fn connect(_app: &str) -> Result<Self, String> {
        let (queue, messages) = mpsc::channel();
        let stops = Arc::new(AtomicU64::new(0));
        let speaking = Arc::new(Mutex::new(None));
        let (thread_stops, thread_speaking) = (stops.clone(), speaking.clone());
        thread::Builder::new()
            .name("mu-speech-say".into())
            .spawn(move || run(messages, thread_stops, thread_speaking))
            .map_err(|e| e.to_string())?;
        Ok(Self { queue, stops, speaking })
    }

    pub fn say(&mut self, text: &str) -> Result<(), String> {
        self.queue.send((self.stops.load(Ordering::SeqCst), text.to_string())).map_err(|_| "the speech thread has stopped".to_string())
    }

    pub fn stop(&mut self) -> Result<(), String> {
        let mut slot = self.speaking.lock().unwrap_or_else(|e| e.into_inner());
        self.stops.fetch_add(1, Ordering::SeqCst);
        if let Some(child) = slot.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        *slot = None;
        Ok(())
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
