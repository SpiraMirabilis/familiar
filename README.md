# beipmu-linux (working name)

A native Linux MU* (MUD/MUCK/MOO/MUSH) client in Rust, inspired by [BeipMU](https://github.com/BeipDev/BeipMU).
It is a new codebase, not a port. The BeipMU sources are used only as a reference for features and protocols.

## Status: M1 (GTK4 client)

| Crate | What it does |
|---|---|
| `mu-telnet` | Telnet state machine. Handles BINARY, ECHO, SGA, EOR/GA prompts, NAWS, TTYPE/MTTS, CHARSET, GMCP, MSSP, and MCCP2 (including compression that starts partway through a read). Also the line splitter. |
| `mu-ansi` | ANSI parser. Supports SGR 16/256/truecolor in both `;` and `:` forms, OSC 8 hyperlinks, and swallows every other CSI/OSC/DCS sequence. Includes plain-URL detection and a palette resolver. |
| `mu-net` | Async TCP/TLS (rustls, verified against system certs, with an option to skip verification). Encodings: UTF-8, Windows-1252, CP437. A session task turns raw bytes into line, prompt and GMCP events. |
| `mu-config` | Worlds, characters and settings in `~/.config/mu-client/config.toml`, plus a built-in directory of 40 public worlds. |
| `mu-engine` | Input history and daily-rotating text logs (`~/.local/share/mu-client/logs`). Triggers and aliases arrive in M2. |
| `mu-gtk` | GTK4/libadwaita client: a tab per connection, ANSI output with clickable links, input with history, password mode, NAWS and logging. |
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
| Enter / Shift+Enter | Send / new line |
| Up, Down | History (Ctrl+Up/Down in multi-line input) |
| Page Up / Page Down | Scroll output |

Links work in OSC 8 output and in plain URLs. `send:` links send a command and `prompt:` links fill the input.

## Roadmap

M2 triggers/aliases/macros → M3 GMCP panes, webviews, sound, MCP → M4 QuickJS scripting →
M5 maps, speech, spellcheck → M6 BeipMU Config.txt, `.atlas` and script compatibility →
M7 a full terminal UI (`mu-tui`, ratatui) sharing the same engine.

`assets/Worlds.xml` comes from BeipMU (MIT licence, © Bennet).
