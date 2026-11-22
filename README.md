# beipmu-linux (working name)

A native Linux MU* (MUD/MUCK/MOO/MUSH) client in Rust, inspired by [BeipMU](https://github.com/BeipDev/BeipMU).
It is a new codebase, not a port. The BeipMU sources are used only as a reference for features and protocols.

## Status: M2 (triggers, aliases, macros)

| Crate | What it does |
|---|---|
| `mu-telnet` | Telnet state machine. Handles BINARY, ECHO, SGA, EOR/GA prompts, NAWS, TTYPE/MTTS, CHARSET, GMCP, MSSP, and MCCP2 (including compression that starts partway through a read). Also the line splitter. |
| `mu-ansi` | ANSI parser. Supports SGR 16/256/truecolor in both `;` and `:` forms, OSC 8 hyperlinks, and swallows every other CSI/OSC/DCS sequence. Includes plain-URL detection and a palette resolver. |
| `mu-net` | Async TCP/TLS (rustls, verified against system certs, with an option to skip verification). Encodings: Auto (UTF-8 with a per-line Windows-1252 fallback, the default), UTF-8, Windows-1252, CP437. A session task turns raw bytes into line, prompt and GMCP events. |
| `mu-config` | Worlds, characters, settings and rules (triggers, aliases, macros, variables at global, world and character scope) in `~/.config/mu-client/config.toml`, plus a built-in directory of 40 public worlds. |
| `mu-engine` | The UI-independent client: per-connection `Engine` that turns network events and input into render outputs. Triggers, aliases, keyboard macros, variables, `/` commands, input history and daily-rotating logs. Shared by the GUI and the planned TUI. |
| `mu-gtk` | GTK4/libadwaita client: a tab per connection, ANSI and full Unicode output with clickable links, spawn-window panes, input with history, password mode, NAWS, logging, sounds and notifications, and a rules editor. |
| `mu-cli` | Terminal client for testing against real servers. |

```sh
cargo run -p mu-gtk            # the GUI
cargo test --workspace
cargo run -p mu-cli -- lambda.moo.mud.org 8888
cargo run -p mu-cli -- --gmcp aardmud.org 4000
```

### GUI keys

| Key | Action |
|---|---|
| Ctrl+O | Connect (saved worlds, quick connect, directory) |
| Ctrl+R / Ctrl+Shift+D | Reconnect / disconnect |
| Ctrl+W | Close tab |
| Ctrl+Shift+G | Toggle logging |
| Ctrl+Shift+L | Clear output |
| Ctrl+T | Triggers, aliases, macros and variables |
| Enter / Shift+Enter | Send / new line |
| Up, Down | History (Ctrl+Up/Down in multi-line input) |
| Page Up / Page Down | Scroll output |

Links work in OSC 8 output and in plain URLs. `send:` links send a command and `prompt:` links fill the input.

### Rules

- **Triggers** match each incoming line (plain text, whole words, or regex with lookaround). They can highlight the match or the whole line, hide it (and leave it out of the log), copy it to a named spawn window, send commands, play a sound, raise a desktop notification, or star the tab. Character triggers run first, then world, then global; "Stop here" ends the chain. A trigger that sends more than 20 times in 2 seconds is suspended so it can't flood the server.
- **Aliases** rewrite what you type: plain ones match the first word (`$1`…`$9`, `$*`), regex ones use capture groups.
- **Macros** bind keys (F5, Ctrl+K, number-pad keys…) to commands.
- **Variables** (`%name%`) work in trigger sends and alias replacements.

Client commands start with `/`: `/set`, `/unset`, `/echo`, `/log`, `/clear`, `/reconnect`, `/disconnect`, `/help`. Type `//` to send a line that starts with `/`.

## Roadmap

M3 GMCP panes, webviews, sound, MCP → M4 QuickJS scripting →
M5 maps, speech, spellcheck → M6 BeipMU Config.txt, `.atlas` and script compatibility →
M7 a full terminal UI (`mu-tui`, ratatui) sharing the same engine.

`assets/Worlds.xml` comes from BeipMU (MIT licence, © Bennet).
