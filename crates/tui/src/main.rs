//! A terminal MU* client on the same engine as the GTK one.

mod app;
mod buffer;
mod form;
mod fuzzy;
mod input;
mod keys;
mod session;
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
    let input = InputThread::start(key_tx);
    let migrated = familiar_config::migrate_old_dirs();
    let mut app = App::new(net_tx);
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
            _ = tokio::time::sleep_until(wake.into()) => app.on_tick(),
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

/// Opens `$VISUAL` or `$EDITOR` (nano if neither is set; Notepad on Windows) on a file, at
/// `line` if the editor understands `+N`.
fn edit_file(path: &std::path::Path, line: Option<u32>) -> Result<(), String> {
    let (mut cmd, name) = familiar_platform::editor_command();
    if let Some(line) = line
        && ["vi", "vim", "nvim", "nano", "emacs", "micro", "kak", "joe", "mg"].contains(&name.as_str())
    {
        cmd.arg(format!("+{line}"));
    }
    let status = cmd.arg(path).status().map_err(|e| format!("couldn't run {name}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{name} exited with {status}")) }
}

/// Edits `content` in a temporary file named after `name`, returning the result.
fn edit_text(name: &str, extension: &str, content: &str, line: Option<u32>) -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("familiar-tui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let short: String = name.chars().take(40).collect();
    let path = dir.join(format!("{}.{extension}", familiar_config::paths::file_name(&short, "edit")));
    // Editors expect a final newline; it's taken off again afterwards.
    let added = !content.is_empty() && !content.ends_with('\n');
    std::fs::write(&path, if added { format!("{content}\n") } else { content.to_string() }).map_err(|e| e.to_string())?;
    let result = edit_file(&path, line).and_then(|_| std::fs::read_to_string(&path).map_err(|e| e.to_string()));
    let _ = std::fs::remove_file(&path);
    result.map(|text| if added { text.strip_suffix('\n').map(str::to_string).unwrap_or(text) } else { text })
}

fn run_external(app: &mut App, job: External) {
    match job {
        External::Edit { session, request, line } => {
            let extension = match request.language.as_deref() {
                Some(lang) if !lang.is_empty() => lang,
                _ => "txt",
            };
            let result = edit_text(&request.name, extension, &request.content, line);
            let Some(idx) = app.session_index(session) else { return };
            match result {
                Ok(text) if text == request.content && !request.is_local() => app.toast("Unchanged, so nothing was sent"),
                Ok(text) => {
                    // Remember what was sent, so Alt+E can reopen it if the server reports errors.
                    let sent = familiar_engine::EditRequest { content: text.clone(), ..request.clone() };
                    app.sessions[idx].last_edit = Some((sent, None));
                    let out = app.sessions[idx].engine.save_edit(&request, &text);
                    app.apply(idx, out);
                    app.toast(&format!("Sent {}", request.name));
                }
                Err(e) => app.toast(&e),
            }
        }
        External::EditInput => {
            let text = app.current_input().map(|i| i.text.clone()).unwrap_or_default();
            match edit_text("input", "txt", &text, None) {
                Ok(text) => {
                    if let Some(input) = app.current_input() {
                        input.set(text.trim_end_matches('\n'));
                    }
                }
                Err(e) => app.toast(&e),
            }
        }
        External::EditConfig => match edit_file(&familiar_config::default_config_path(), None) {
            Ok(()) => app.reload_config(),
            Err(e) => app.toast(&e),
        },
    }
}
