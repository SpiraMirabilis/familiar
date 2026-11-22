//! Plays Client.Media (MCMP) sounds: downloads them to the cache off the UI thread, then plays
//! them with GTK's media support.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use familiar_engine::media::{MediaCommand, fetch};

#[cfg(not(windows))]
/// A one-line explanation of a playback error. GStreamer's own messages are long and technical.
pub fn describe_error(name: &str, error: &glib::Error) -> String {
    let message = error.message();
    if message.contains("missing a plug-in") || message.contains("No suitable plugins") {
        let fix = if cfg!(windows) { "" } else { " Installing gst-plugins-good (WAV, MP3 and audio output) usually fixes this." };
        format!("Couldn't play {name}: GStreamer has no decoder or audio output for it.{fix}")
    } else {
        format!("Couldn't play {name}: {}", message.lines().next().unwrap_or(message))
    }
}

#[derive(Default)]
pub struct MediaPlayer {
    playing: RefCell<HashMap<String, Clip>>,
    /// The latest play request per name. A download that finishes after a newer request (or a
    /// stop) is dropped.
    requests: RefCell<HashMap<String, u64>>,
    next_request: Cell<u64>,
}

impl MediaPlayer {
    /// Runs a command. `notice` reports problems to the user.
    pub fn run(self: &Rc<Self>, command: MediaCommand, notice: Rc<dyn Fn(String)>) {
        match command {
            MediaCommand::Preload { url } => {
                let job = crate::runtime().spawn_blocking(move || fetch(&url));
                glib::spawn_future_local(async move {
                    if let Ok(Err(e)) = job.await {
                        notice(format!("Couldn't download a sound: {e}"));
                    }
                });
            }
            MediaCommand::Stop { name: Some(name) } => {
                self.requests.borrow_mut().remove(&name);
                if let Some(clip) = self.playing.borrow_mut().remove(&name) {
                    clip.stop();
                }
            }
            MediaCommand::Stop { name: None } => {
                self.requests.borrow_mut().clear();
                for (_, clip) in self.playing.borrow_mut().drain() {
                    clip.stop();
                }
            }
            MediaCommand::Play { name, url, volume, loops, restart } => {
                if !restart && self.playing.borrow().get(&name).is_some_and(|m| m.is_playing()) {
                    return;
                }
                let id = self.next_request.get() + 1;
                self.next_request.set(id);
                self.requests.borrow_mut().insert(name.clone(), id);
                let job = crate::runtime().spawn_blocking(move || fetch(&url));
                let this = Rc::downgrade(self);
                glib::spawn_future_local(async move {
                    let path = match job.await {
                        Ok(Ok(path)) => path,
                        Ok(Err(e)) => return notice(format!("Couldn't play a sound: {e}")),
                        Err(e) => return notice(format!("Couldn't play a sound: {e}")),
                    };
                    let Some(this) = this.upgrade() else { return };
                    if this.requests.borrow().get(&name) != Some(&id) {
                        return; // Stopped or replaced while downloading.
                    }
                    this.start(&name, &path, volume, loops, notice);
                });
            }
        }
    }

    fn start(&self, name: &str, path: &std::path::Path, volume: f64, loops: i32, notice: Rc<dyn Fn(String)>) {
        if let Some(old) = self.playing.borrow_mut().remove(name) {
            old.stop();
        }
        if let Some(clip) = Clip::play(name, path, volume, loops, notice) {
            self.playing.borrow_mut().insert(name.to_string(), clip);
        }
    }
}

/// One playing sound. GTK's media support (GStreamer) does the work on Linux; on Windows that
/// would mean shipping GStreamer and its plugins, so sounds go through the platform player.
#[cfg(not(windows))]
pub struct Clip(gtk::MediaFile);

#[cfg(not(windows))]
impl Clip {
    /// Starts playing. `loops` below zero repeats until stopped. `notice` reports problems.
    pub fn play(name: &str, path: &std::path::Path, volume: f64, loops: i32, notice: Rc<dyn Fn(String)>) -> Option<Self> {
        let media = gtk::MediaFile::for_filename(path);
        media.set_volume(volume);
        if loops < 0 {
            media.set_loop(true);
        } else if loops > 1 {
            let remaining = Cell::new(loops - 1);
            media.connect_ended_notify(move |media| {
                if media.is_ended() && remaining.get() > 0 {
                    remaining.set(remaining.get() - 1);
                    media.seek(0);
                    media.play();
                }
            });
        }
        let label = name.to_string();
        media.connect_error_notify(move |media| {
            if let Some(e) = media.error() {
                notice(describe_error(&label, &e));
            }
        });
        media.play();
        Some(Self(media))
    }

    pub fn stop(&self) {
        self.0.set_playing(false);
    }

    pub fn is_playing(&self) -> bool {
        self.0.is_playing()
    }

    pub fn is_finished(&self) -> bool {
        self.0.is_ended() || self.0.error().is_some()
    }
}

#[cfg(windows)]
pub struct Clip(String);

#[cfg(windows)]
fn player() -> &'static familiar_platform::SoundPlayer {
    static PLAYER: std::sync::OnceLock<familiar_platform::SoundPlayer> = std::sync::OnceLock::new();
    PLAYER.get_or_init(familiar_platform::SoundPlayer::new)
}

#[cfg(windows)]
impl Clip {
    /// Starts playing. `loops` below zero repeats until stopped. `notice` reports problems.
    pub fn play(name: &str, path: &std::path::Path, volume: f64, loops: i32, notice: Rc<dyn Fn(String)>) -> Option<Self> {
        // Each clip gets a key of its own, so stopping one never stops another of the same name.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let key = format!("{name}#{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        match player().play_loops(&key, path, volume, loops) {
            Ok(()) => Some(Self(key)),
            Err(e) => {
                notice(e);
                None
            }
        }
    }

    pub fn stop(&self) {
        player().stop(Some(&self.0));
    }

    pub fn is_playing(&self) -> bool {
        player().is_playing(&self.0)
    }

    pub fn is_finished(&self) -> bool {
        !self.is_playing()
    }
}
