//! A terminal MU* client on the same engine as the GTK one.

mod app;
mod buffer;
mod clipboard;
mod editor;
mod form;
mod fuzzy;
mod input;
mod keys;
mod rules;
mod session;
mod term;
mod theme;
mod ui;

use std::io::{Stdout, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::Parser;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste, EnableFocusChange, EnableMouseCapture, Event,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::crossterm::{execute, queue};
use tokio::sync::mpsc;

use crate::app::{App, External};

#[derive(Parser)]
#[command(name = "familiar-tui", version, about = "Familiar: a terminal client for MUDs, MUSHes, MUCKs and MOOs")]
struct Args {
    /// A saved world to connect to (or host:port).
    world: Option<String>,
    /// Which of the world's characters to log in as.
    character: Option<String>,
}

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Reads terminal events on a thread, so it can be paused while an external editor owns the
/// terminal (crossterm would otherwise read the editor's keystrokes).
struct InputThread {
    paused: Arc<AtomicBool>,
    idle: Arc<AtomicBool>,
}

impl InputThread {
    fn start(tx: mpsc::UnboundedSender<Event>) -> Self {
        let paused = Arc::new(AtomicBool::new(false));
        let idle = Arc::new(AtomicBool::new(false));
        let (p, i) = (paused.clone(), idle.clone());
        std::thread::spawn(move || {
            loop {
                if p.load(Ordering::SeqCst) {
                    i.store(true, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                i.store(false, Ordering::SeqCst);
                match event::poll(Duration::from_millis(50)) {
                    Ok(true) => {
                        let Ok(first) = event::read() else { return };
                        let mut events = vec![first];
                        if cfg!(windows) {
                            // Read everything that's waiting, to tell a paste from typing.
                            while events.len() < 100_000 && matches!(event::poll(Duration::ZERO), Ok(true)) {
                                let Ok(ev) = event::read() else { return };
                                events.push(ev);
                            }
                            events = keys::coalesce_paste(events);
                        }
                        for ev in events {
                            if tx.send(ev).is_err() {
                                return;
                            }
                        }
                    }
                    Ok(false) => {}
                    Err(_) => return,
                }
            }
        });
        Self { paused, idle }
    }

    fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        while !self.idle.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }
}

fn enter(stdout: &mut Stdout) -> std::io::Result<()> {
    terminal::enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, EnableFocusChange)?;
    // Older Windows consoles can't do this one; pastes are recognised another way there.
    let _ = execute!(stdout, EnableBracketedPaste);
    // Lets terminals that support it report Shift+Enter and keypad keys distinctly.
    if terminal::supports_keyboard_enhancement().unwrap_or(false) {
        queue!(stdout, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
    }
    stdout.flush()
}

fn leave(stdout: &mut Stdout) {
    if terminal::supports_keyboard_enhancement().unwrap_or(false) {
        let _ = queue!(stdout, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(stdout, DisableBracketedPaste);
    let _ = execute!(stdout, DisableFocusChange, DisableMouseCapture, LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();
    // Restore the terminal if anything panics, so the shell stays usable.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        leave(&mut std::io::stdout());
        default_hook(info);
    }));

    enter(&mut std::io::stdout())?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let result = run(&mut terminal, args).await;
    leave(&mut std::io::stdout());
    result
}

async fn run(terminal: &mut Term, args: Args) -> std::io::Result<()> {
    let (net_tx, mut net_rx) = mpsc::unbounded_channel();
    let (key_tx, mut key_rx) = mpsc::unbounded_channel();
    let (pty_tx, mut pty_rx) = mpsc::unbounded_channel();
    let input = InputThread::start(key_tx);
    let migrated = familiar_config::migrate_old_dirs();
    let mut app = App::new(net_tx, pty_tx);
    for message in migrated {
        app.toast(&message);
    }
    if let Some(world) = &args.world {
        app.overlay = None;
        app.connect_command(&format!("{world} {}", args.character.as_deref().unwrap_or("")));
        if app.tabs.is_empty() {
            app.open_connect_picker();
        }
    }

    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;
        let wake = app.next_wake();
        tokio::select! {
            Some(ev) = key_rx.recv() => {
                app.on_event(ev);
                // Take everything already typed (e.g. a paste without bracketed paste) at once.
                while let Ok(ev) = key_rx.try_recv() {
                    app.on_event(ev);
                }
            }
            Some(ev) = net_rx.recv() => {
                app.on_net(ev);
                // Drain a burst of lines before redrawing.
                for _ in 0..500 {
                    match net_rx.try_recv() {
                        Ok(ev) => app.on_net(ev),
                        Err(_) => break,
                    }
                }
            }
            Some(ev) = pty_rx.recv() => {
                app.on_pty(ev);
                while let Ok(ev) = pty_rx.try_recv() {
                    app.on_pty(ev);
                }
            }
            _ = tokio::time::sleep_until(wake.into()) => app.on_tick(),
        }
        if let Some(text) = app.clipboard.take() {
            clipboard::copy(&text);
        }
        if let Some(job) = app.external.take() {
            input.pause();
            leave(&mut std::io::stdout());
            run_external(&mut app, job);
            enter(&mut std::io::stdout())?;
            input.resume();
            terminal.clear()?;
        }
        if app.quit {
            break;
        }
    }
    app.shutdown();
    Ok(())
}

/// Runs an edit with the terminal handed over to the editor.
fn run_external(app: &mut App, job: External) {
    let result = app.document(&job).and_then(|(document, _, line)| {
        let (mut command, name) = editor::command(&document.path, line);
        let ran = match command.status() {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(format!("{name} exited with {status}")),
            Err(e) => Err(format!("couldn't run {name}: {e}")),
        };
        match ran {
            Ok(()) => document.finish(),
            Err(e) => {
                document.discard();
                Err(e)
            }
        }
    });
    app.finish_edit(job, result);
}
