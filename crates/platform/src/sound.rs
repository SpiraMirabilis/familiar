//! Sound playback: the system's command-line player on Linux (PipeWire, PulseAudio or ALSA, so
//! no audio libraries are linked), and rodio on Windows.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Plays sound files. Cheap to clone; clones share the same sounds, so one can be handed to a
/// background download and still be stopped from the UI.
#[derive(Clone)]
pub struct SoundPlayer {
    backend: Arc<Mutex<imp::Backend>>,
    stops: Arc<AtomicU64>,
}

impl Default for SoundPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl SoundPlayer {
    /// Nothing is opened until the first sound plays.
    pub fn new() -> Self {
        Self { backend: Arc::new(Mutex::new(imp::Backend::default())), stops: Arc::new(AtomicU64::new(0)) }
    }

    /// Starts a sound. `name` is what [`SoundPlayer::stop`] knows it by; `volume` is 0 to 1.
    pub fn play(&self, name: &str, path: &Path, volume: f64) -> Result<(), String> {
        if !path.is_file() {
            return Err(format!("Sound file not found: {}", path.display()));
        }
        self.backend.lock().unwrap().play(name, path, volume.clamp(0.0, 1.0))
    }

    /// Stops the sounds with this name, or every sound.
    pub fn stop(&self, name: Option<&str>) {
        self.stops.fetch_add(1, Ordering::Relaxed);
        self.backend.lock().unwrap().stop(name);
    }

    /// Forgets sounds that have finished. Worth calling now and then; playing does it too.
    pub fn tidy(&self) {
        self.backend.lock().unwrap().tidy();
    }

    /// Changes whenever something is stopped. A sound that had to be downloaded first compares
    /// this from before and after, and doesn't start if a stop arrived in between.
    pub fn stop_count(&self) -> u64 {
        self.stops.load(Ordering::Relaxed)
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;
    use std::process::{Child, Command, Stdio};

    #[derive(Default)]
    pub struct Backend {
        /// Looked up on first use; None inside means there isn't one.
        player: Option<Option<&'static str>>,
        sounds: Vec<(String, Child)>,
    }

    impl Backend {
        pub fn play(&mut self, name: &str, path: &Path, volume: f64) -> Result<(), String> {
            self.tidy();
            let player = *self.player.get_or_insert_with(|| ["pw-play", "paplay", "aplay"].into_iter().find(|p| crate::which(p).is_some()));
            let Some(player) = player else {
                return Err("No sound player found (install pipewire, pulseaudio-utils or alsa-utils)".into());
            };
            let mut cmd = Command::new(player);
            match player {
                "pw-play" => {
                    cmd.arg(format!("--volume={volume:.2}"));
                }
                "paplay" => {
                    cmd.arg(format!("--volume={}", (volume * 65536.0) as u32));
                }
                _ => {}
            }
            cmd.arg(path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            let child = cmd.spawn().map_err(|e| format!("Couldn't play {}: {e}", path.display()))?;
            self.sounds.push((name.to_string(), child));
            Ok(())
        }

        /// Forgets the ones that have finished (which also reaps the processes).
        pub fn tidy(&mut self) {
            self.sounds.retain_mut(|(_, child)| matches!(child.try_wait(), Ok(None)));
        }

        pub fn stop(&mut self, name: Option<&str>) {
            self.sounds.retain_mut(|(n, child)| {
                let stop = name.is_none_or(|wanted| wanted == n);
                if stop {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                !stop
            });
        }
    }

    impl Drop for Backend {
        fn drop(&mut self) {
            self.stop(None);
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;

    enum Command {
        Play { name: String, path: PathBuf, volume: f64, reply: mpsc::Sender<Result<(), String>> },
        Stop(Option<String>),
    }

    /// The audio device handle can't move between threads, so it lives on its own thread and
    /// this just sends it commands.
    #[derive(Default)]
    pub struct Backend {
        commands: Option<mpsc::Sender<Command>>,
    }

    fn run(commands: mpsc::Receiver<Command>) {
        let mut device: Option<rodio::MixerDeviceSink> = None;
        let mut sounds: Vec<(String, rodio::Player)> = Vec::new();
        for command in commands {
            sounds.retain(|(_, player)| !player.empty());
            match command {
                Command::Play { name, path, volume, reply } => {
                    let result = (|| {
                        if device.is_none() {
                            device = Some(rodio::DeviceSinkBuilder::open_default_sink().map_err(|e| format!("No sound device: {e}"))?);
                        }
                        let file = std::fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                        let player = rodio::play(device.as_ref().unwrap().mixer(), std::io::BufReader::new(file))
                            .map_err(|e| format!("Couldn't play {}: {e}", path.display()))?;
                        player.set_volume(volume as _);
                        sounds.push((name, player));
                        Ok(())
                    })();
                    if result.is_err() {
                        // The device may have gone away (headphones unplugged); reopen next time.
                        device = None;
                    }
                    let _ = reply.send(result);
                }
                Command::Stop(name) => sounds.retain(|(n, player)| {
                    let stop = name.as_ref().is_none_or(|wanted| wanted == n);
                    if stop {
                        player.stop();
                    }
                    !stop
                }),
            }
        }
    }

    impl Backend {
        fn sender(&mut self) -> &mpsc::Sender<Command> {
            self.commands.get_or_insert_with(|| {
                let (tx, rx) = mpsc::channel();
                let _ = std::thread::Builder::new().name("familiar-sound".into()).spawn(move || run(rx));
                tx
            })
        }

        pub fn play(&mut self, name: &str, path: &Path, volume: f64) -> Result<(), String> {
            let (reply, result) = mpsc::channel();
            let command = Command::Play { name: name.into(), path: path.into(), volume, reply };
            self.sender().send(command).map_err(|_| "The sound thread has stopped".to_string())?;
            result.recv().map_err(|_| "The sound thread has stopped".to_string())?
        }

        pub fn stop(&mut self, name: Option<&str>) {
            // Nothing has played yet if there's no thread, so there's nothing to stop.
            if let Some(commands) = &self.commands {
                let _ = commands.send(Command::Stop(name.map(str::to_string)));
            }
        }

        /// The sound thread drops finished sounds itself.
        pub fn tidy(&mut self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::SoundPlayer;

    #[test]
    fn missing_file_is_reported() {
        let player = SoundPlayer::new();
        let e = player.play("x", std::path::Path::new("no-such-sound.wav"), 1.0).unwrap_err();
        assert!(e.contains("not found"), "{e}");
    }

    #[test]
    fn stopping_is_counted() {
        let player = SoundPlayer::new();
        let before = player.stop_count();
        player.clone().stop(None);
        assert_ne!(player.stop_count(), before);
    }
}
