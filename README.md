# beipmu-linux (working name)

A native Linux MU* (MUD/MUCK/MOO/MUSH) client in Rust, inspired by [BeipMU](https://github.com/BeipDev/BeipMU).
It is a new codebase, not a port. The BeipMU sources are used only as a reference for features and protocols.

## Status: M8 (BeipMU compatibility)

| Crate | What it does |
|---|---|
| `mu-telnet` | Telnet state machine. Handles BINARY, ECHO, SGA, EOR/GA prompts, NAWS, TTYPE/MTTS, CHARSET, GMCP, MSSP, and MCCP2 (including compression that starts partway through a read). Also the line splitter. |
| `mu-ansi` | ANSI parser. Supports SGR 16/256/truecolor in both `;` and `:` forms, OSC 8 hyperlinks, and swallows every other CSI/OSC/DCS sequence. Includes plain-URL detection and a palette resolver. |
| `mu-net` | Async TCP/TLS (rustls, verified against system certs, with an option to skip verification). Encodings: Auto (UTF-8 with a per-line Windows-1252 fallback, the default), UTF-8, Windows-1252, CP437. A session task turns raw bytes into line, prompt and GMCP events. |
| `mu-config` | Worlds, characters, settings and rules (triggers, aliases, macros, variables at global, world and character scope) in `~/.config/mu-client/config.toml`, plus a built-in directory of 40 public worlds. |
| `mu-engine` | The UI-independent client: per-connection `Engine` that turns network events and input into render outputs. Triggers, aliases, keyboard macros, variables, `/` commands, input history, daily-rotating logs, GMCP (`Beip.Stats`, `Client.Media`, `WebView`), and MCP 2.1 (simpleedit, awns-status, awns-ping, vmoo-client). Shared by the GUI and the planned TUI. |
| `mu-script` | JavaScript scripting with QuickJS: one isolated runtime per connection, a 2-second limit per call and a memory cap. Implements the engine's `ScriptHost` trait, so any frontend gets scripting. |
| `mu-map` | World maps: rooms, exits and areas, an automapper for GMCP `Room.Info` (IRE, Aardwolf and BeipMU styles), shortest-path walking, and tracking by room name. |
| `mu-import` | Reads BeipMU's `Config.txt` (worlds, characters, puppets, triggers, aliases, macros, variables) and `.atlas` maps (plain or zipped XML), reporting anything that can't be carried over. |
| `mu-speech` | Text to speech through speech-dispatcher, on a background thread. |
| `mu-code` | Editing support shared by frontends: block-aware auto-indentation for MOO, MUF and Lua. |
| `mu-gtk` | GTK4/libadwaita client: a tab per connection, ANSI and full Unicode output with clickable links, a side dock with spawn windows and GMCP stat panes, input with history, password mode, NAWS, logging, sounds (trigger and server-driven), notifications, an MCP status line and text editor, web views, and a rules editor. |
| `mu-cli` | Terminal client for testing against real servers. |

```sh
cargo run -p mu-gtk            # the GUI
cargo run -p mu-gtk -- "World" "Character"   # connect to a saved world on startup
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
| Ctrl+. | Stop speaking |
| Enter / Shift+Enter | Send / new line |
| Up, Down | History (Ctrl+Up/Down in multi-line input) |
| Page Up / Page Down | Scroll output |

Links work in OSC 8 output and in plain URLs. `send:` links send a command and `prompt:` links fill the input.

### Rules

- **Triggers** match each incoming line (plain text, whole words, or regex with lookaround). They can highlight the match or the whole line, hide it (and leave it out of the log), copy it to a named spawn window, send commands, play a sound, raise a desktop notification, or star the tab. Character triggers run first, then world, then global; "Stop here" ends the chain. A trigger that sends more than 20 times in 2 seconds is suspended so it can't flood the server.
- Triggers can also **replace** the matching text (to censor words, say), and a highlight can style only a regex's groups.
- **Aliases** rewrite what you type: plain ones match the first word (`$1`…`$9`, `$*`), regex ones use capture groups. An alias set to "replace just the matching text" edits every match in the line and lets later aliases run too, as BeipMU's do (`Mag` → `Magdalena` inside `page Mag=hi`). An alias can expand to client commands (`/script …`, `/delay …`).
- **Macros** bind keys (F5, Ctrl+K, number-pad keys…) to commands, or put text in the input box.
- **`/delay [id NAME] [every] 5s command`** runs a command later or repeatedly (`/delay list`, `/delay kill NAME`, `/delay killall`). Logins can use client commands too, such as `/delay 3s "connect bob pw"`.
- **Variables** (`%name%`) work in trigger sends and alias replacements.

### Scripting

JavaScript (QuickJS) runs per connection. `~/.config/mu-client/startup.js` runs for every session, and each world can name its own script file. `/@code` runs code right away (Shift+Enter for multiple lines), and `/script reload` restarts scripting after you edit a file. Scripts get `mu.send`, `mu.input`, `mu.echo`, `mu.notice`, `mu.spawn`, `mu.gmcp`, `mu.vars`, `mu.world`/`mu.character`/`mu.connected`, events through `mu.on` (`line` to hide lines, `receive`, `send` to rewrite or drop commands, `gmcp` and `gmcp:Package`, `connect`, `disconnect`), timers and `console.log`. Triggers can call a script function with the line and its regex groups. See [examples/startup.js](examples/startup.js).

BeipMU scripts work too: the `app` and `window` objects are there (member names ignore case, as in BeipMU), including `window.connection.SetOnReceive`/`SetOnSend`/`SetOnGMCP`/`SetOnConnect` hooks, `Display`/`Send`/`Receive`, `window.output.Write`, `window.Run`, variables, `app.CreateInterval`/`CreateTimeout`, `app.PlaySound`, and `app.NewWindow_Text()` (which becomes a spawn window). Hook functions get plain text without ANSI codes. A trigger whose script is `beipmu:Name` calls `Name(ranges, line, window)` the way BeipMU did. ActiveX, sockets and drawing windows aren't available and say so.

Client commands start with `/`: `/set`, `/unset`, `/echo`, `/log`, `/clear`, `/reconnect`, `/disconnect`, `/gmcp` (show GMCP traffic, or `/gmcp Package {json}` to send), `/mcp`, `/webview url`, `/images on|off`, `/puppet [name]`, `/speech on|off|stop`, `/speak text`, `/map …`, `/edit [language]`, `/delay …`, `/@code`, `/script code`, `/script load path`, `/script reload`, `/help`. Type `//` to send a line that starts with `/`.

### Program editor

When a MOO sends code with MCP simpleedit (`type: moo-code`), it opens in a small code editor (GtkSourceView): syntax highlighting, line numbers, bracket matching, search (Ctrl+F), and auto-indent that re-indents each line as you press Enter (Ctrl+Shift+I re-indents everything). Ctrl+S sends it back without closing; compile errors in the reply (`Line 3: …`, or FuzzBall's `Error in line 3: …`) are marked on their lines. `/edit [language]` opens a scratch editor whose text is sent line by line, for `@program`-style uploads. Plain text (descriptions) opens without highlighting.

Languages: **MOO (Stunt)** built from the Stunt lexer (keywords, `/* */` and string-statement comments, `$#colour` and `b"bytes"` literals, string escapes, maps and bitwise operators, built-in variables and all 209 built-in functions), **MUF**, and GtkSourceView's built-in languages such as Lua. Each world has a program language. Add your own GtkSourceView `.lang` files to `~/.local/share/mu-client/language-specs/`.

### Map

The map tab appears in the side dock once a world has rooms. If the game sends GMCP `Room.Info`, the map fills in as you move: rooms are placed by the direction you walked when the server sends no coordinates, and exits link up as both ends are seen. Without `Room.Info`, add rooms with `/map addroom "Name" north south` and the map follows you by recognising neighbouring room names. Double-click a room (or `/map go name`) to walk there by the shortest known path. Drag rooms to move them (Ctrl to place freely), drag the background to pan, scroll to zoom, and right-click for Walk Here, I'm Here, Rename, Delete and Add Room Here. Maps are saved per world in `~/.local/share/mu-client/maps/`. The folder button in the map (or `/map import file.atlas`) adds a BeipMU map: each of its maps becomes an area, with the room shapes, colours and exits. Background rectangles, labels and exit line shapes aren't drawn.

### Importing from BeipMU

Main menu → **Import from BeipMU…** and choose BeipMU's `Config.txt` (BeipMU keeps it in `%APPDATA%\BeipMU`, which under Wine is `~/.wine/drive_c/users/$USER/AppData/Roaming/BeipMU/`, or next to `BeipMU.exe`). Worlds, characters (logins with `%NAME%`/`%PASSWORD%` filled in), puppets, triggers, aliases, keyboard macros and character variables are added; worlds you already have are skipped, so importing again is safe. BeipMU's startup script is copied to `startup.js` if it's next to `Config.txt` and you don't have one. A list at the end says what couldn't be carried over: nested triggers become flat (and say so), "away" triggers come in switched off, Windows sound paths need changing, and hash colours, paragraph styling, stat updates, multi-line captures and per-puppet rules have no equivalent.

### Puppets

A puppet is a second character you control through your main connection, such as a MUCK zombie or a MUSH puppet, with its own tab (as in BeipMU). Add puppets to a character in the Connect dialog (edit the world). Lines starting with the puppet's receive prefix (plain text, or a regex whose group is the part to hide) go to its tab instead of the main window, and what you type there is sent with its send prefix, such as `z1 `. Tabs open when the puppet hears something, when the character connects, or with `/puppet name`, and they close with the character.

### Speech and spellcheck

`/speech on` speaks every incoming line through speech-dispatcher; `/speak text` says something; Ctrl+. or `/speech stop` stops. Triggers can speak (`$0` for the whole line, or a template like `Page from $1`), and scripts can call `mu.speak()`. Speech queues in order, and stopping cancels only this client's speech, not a screen reader's. The input underlines misspellings (libspelling with your Hunspell dictionaries); right-click a word for suggestions.

### Protocols

- **GMCP:** the client announces `Beip.Stats 1` and `Client.Media 1`. `Beip.Stats` panes appear as tabs in the side dock, using BeipMU's format (int, string, range and progress values, colours, partial updates, null deletes). `Client.Media` sounds are downloaded to `~/.cache/mu-client/media` and played.
- **Avatars** (`beip.line.image-url`, `beip.line.id`, `beip.ids`): a small picture before a line, with hover text and a click link. Unknown ids are requested once with `beip.id.request`, and pictures already shown update when the server describes them.
- **Inline images:** `/images on` shows a thumbnail under lines that link to images (off by default, since fetching an image tells its site your IP address).
- **Beip.Tilemap** (`beip.tilemap.info` / `.data`): tile grids drawn from a tile image, in all four encodings (`hex_4`, `hex_8`, `base64_8`, `zbase64_8`), each in its own side-dock tab.
- **MCP 2.1** (MOOs and MUCKs, on by default per world): the status line appears under the output, and simpleedit opens an editor window that sends your changes back.
- **Web views:** opened with `/webview url` or by the server (GMCP `webview.open`/`webview.close`, with `dock`, `width`/`height`, `source` and `http-request-headers`). The client asks before a server opens one (Block, Allow Once, Always Allow per world). Docked views go in the side dock; others get their own window. Pages reach the client as `window.mu.client`, or as `window.chrome.webview.hostObjects.client` for pages written for BeipMU, with the same case-insensitive methods: `Send`, `SendGMCP`, `Receive`, `Display`, `CloseWindow`, `SetOnSend`, `SetOnReceive`, `SetOnGMCP`, `SetOnDisplay`, `ClearOnDisplay`. Calls are asynchronous, and `SetOnDisplay` can't gag lines.

### Optional system packages

- `gst-plugins-good`: WAV/MP3 decoding and the standard audio output for sounds. Without it, playback reports a missing plugin.
- `speech-dispatcher` and a voice such as `espeak-ng`: speech.
- `hunspell-en_us` (or another Hunspell dictionary): spellcheck.
- `webkitgtk-6.0`: web views (on by default). Without it, build with `cargo run -p mu-gtk --no-default-features`.

## Roadmap

M9 a full terminal UI (`mu-tui`, ratatui) sharing the same engine.

`assets/Worlds.xml` and the test files in `crates/mu-import/tests/data/` (sample config and maps) come from BeipMU (MIT licence, © Bennet).
