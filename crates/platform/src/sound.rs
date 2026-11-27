//! Sound playback: the system's command-line player on Linux (PipeWire, PulseAudio or ALSA) and
//! macOS (`afplay`), so no audio libraries are linked, and rodio on Windows.

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
        self.play_loops(name, path, volume, 1)
    }

    /// Like [`SoundPlayer::play`], repeating `loops` times, or until stopped if that's negative.
    /// (On Linux and macOS the command-line players can't repeat, so the sound plays once.)
    pub fn play_loops(&self, name: &str, path: &Path, volume: f64, loops: i32) -> Result<(), String> {
        if !path.is_file() {
            return Err(format!("Sound file not found: {}", path.display()));
        }
        let volume = if volume.is_finite() { volume.clamp(0.0, 1.0) } else { 1.0 };
        self.backend.lock().unwrap_or_else(|e| e.into_inner()).play(name, path, volume, loops)
    }

    /// Whether a sound with this name is still playing.
    pub fn is_playing(&self, name: &str) -> bool {
        self.backend.lock().unwrap_or_else(|e| e.into_inner()).is_playing(name)
    }

    /// Stops the sounds with this name, or every sound.
    pub fn stop(&self, name: Option<&str>) {
        self.stops.fetch_add(1, Ordering::Relaxed);
        self.backend.lock().unwrap_or_else(|e| e.into_inner()).stop(name);
    }

    /// Forgets sounds that have finished. Worth calling now and then; playing does it too.
    pub fn tidy(&self) {
        self.backend.lock().unwrap_or_else(|e| e.into_inner()).tidy();
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

    const MAX_CONCURRENT_SOUNDS: usize = 16;

    impl Backend {
        pub fn play(&mut self, name: &str, path: &Path, volume: f64, _loops: i32) -> Result<(), String> {
            self.tidy();
            if self.sounds.len() >= MAX_CONCURRENT_SOUNDS {
                return Ok(());
            }
            let players: &[&'static str] = if cfg!(target_os = "macos") { &["afplay"] } else { &["pw-play", "paplay", "aplay"] };
            let player = *self.player.get_or_insert_with(|| players.iter().copied().find(|p| crate::which(p).is_some()));
            let Some(player) = player else {
                return Err(if cfg!(target_os = "macos") {
                    "No sound player found (afplay is missing)".into()
                } else {
                    "No sound player found (install pipewire, pulseaudio-utils or alsa-utils)".into()
                });
            };
            let mut cmd = Command::new(player);
            match player {
                "pw-play" => {
                    cmd.arg(format!("--volume={volume:.2}"));
                }
                "afplay" => {
                    cmd.args(["-v", &format!("{volume:.2}")]);
                }
                "paplay" => {
                    cmd.arg(format!("--volume={}", (volume * 65536.0) as u32));
                }
                _ => {}
            }
            // Prefix `-`-leading relative paths with `./` so players don't parse them as flags.
            let safe_path = if path.is_relative() && path.to_string_lossy().starts_with('-') {
                Path::new(".").join(path)
            } else {
                path.to_path_buf()
            };
            cmd.arg(&safe_path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            let child = cmd.spawn().map_err(|e| format!("Couldn't play {}: {e}", path.display()))?;
            self.sounds.push((name.to_string(), child));
            Ok(())
        }

        pub fn is_playing(&mut self, name: &str) -> bool {
            self.tidy();
            self.sounds.iter().any(|(n, _)| n == name)
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
        Play { name: String, path: PathBuf, volume: f64, loops: i32, reply: mpsc::Sender<Result<(), String>> },
        Stop(Option<String>),
        IsPlaying { name: String, reply: mpsc::Sender<bool> },
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
                Command::Play { name, path, volume, loops, reply } => {
                    let result = (|| {
                        if device.is_none() {
                            device = Some(rodio::DeviceSinkBuilder::open_default_sink().map_err(|e| format!("No sound device: {e}"))?);
                        }
                        let open = || std::fs::File::open(&path).map(std::io::BufReader::new).map_err(|e| format!("{}: {e}", path.display()));
                        let bad = |e: rodio::decoder::DecoderError| format!("Couldn't play {}: {e}", path.display());
                        let player = rodio::Player::connect_new(device.as_ref().unwrap().mixer());
                        player.set_volume(volume as _);
                        if loops < 0 {
                            player.append(rodio::Decoder::new_looped(open()?).map_err(bad)?);
                        } else {
                            for _ in 0..loops.clamp(1, 100) {
                                player.append(rodio::Decoder::new(open()?).map_err(bad)?);
                            }
                        }
                        sounds.push((name, player));
                        Ok(())
                    })();
                    let _ = reply.send(result);
                }
                Command::IsPlaying { name, reply } => {
                    let _ = reply.send(sounds.iter().any(|(n, _)| *n == name));
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

        pub fn is_playing(&mut self, name: &str) -> bool {
            let Some(commands) = &self.commands else { return false };
            let (reply, result) = mpsc::channel();
            commands.send(Command::IsPlaying { name: name.into(), reply }).is_ok() && result.recv().unwrap_or(false)
        }

        pub fn play(&mut self, name: &str, path: &Path, volume: f64, loops: i32) -> Result<(), String> {
            let (reply, result) = mpsc::channel();
            let command = Command::Play { name: name.into(), path: path.into(), volume, loops, reply };
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
