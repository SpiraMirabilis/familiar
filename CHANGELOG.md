# Changelog

## Unreleased

- Every keyboard shortcut can be changed, in every app: Preferences → Keyboard (F1; ⌘⌥/ on a
  Mac) in the desktop apps, and a key editor (F1 or `/keys`) in the terminal client. A command
  can have several keys; keys shared with another command or a keyboard macro are marked (the
  shortcut wins). Changes go in `[settings.keys]`, or `[settings.terminal_keys]` for the
  terminal client.
- Every app now has every command, each with a key: a command palette (Ctrl+K or Ctrl+Shift+P;
  ⇧⌘P) and input history search (Ctrl+Shift+R; ⇧⌘R) in the desktop apps, find in the output in
  the GTK app and the terminal client, Alt+1…9 for tabs, keys for the side dock and its tabs, the
  map, editing and adding worlds, restarting scripts, and reopening the last program sent from
  the editor at its first error (Alt+E). The terminal client gets `/import` for BeipMU settings.
- Notices about reconnecting name the app's actual Reconnect key.
- Autologging per character, as in BeipMU: a character can name a log file that it logs to
  whenever it connects, optionally with the date in its name (a new file each day). Set it in
  the Connect dialog, or with `log_file` and `log_date` in `config.toml`.
- Restore log, as in BeipMU: a character can keep the end of its output (with colours) and show
  it again when its tab opens (`restore_log`).
- Terminal client on macOS: Option+← and → switch tabs. Mac terminals send them as Esc b and
  Esc f, which the input used to take as word movement.
- Windows app: Alt+← and → switch tabs (they only beeped).
- Importing from BeipMU brings in each character's log file, date setting and Restore Log. On
  macOS and Linux a Windows log path becomes the same file name in the logs folder.

## 0.5.0 (2026-11-25)

- Terminal client:
  - `/agent` runs an LLM agent (Claude Code, Codex, Antigravity or Grok Build built in; any
    other through `[[settings.agent]]`) beside the world, where `$EDITOR` goes. Without a
    name it runs the first one installed, in the order `agent_order` gives. Familiar serves
    it the world's tools over MCP (Model Context Protocol), on 127.0.0.1 only.
  - The MOO defines the tools, with a new MCP 2.1 package, `dns-com-familiar-agent`
    (`docs/agent-protocol.md`): listing, searching and programming verbs, creating objects,
    and so on. They run with the character's own permissions.
  - Results come back as tagged, authenticated messages, so nothing is scraped from the
    output. Each call shows a one-line summary in the world's window.
  - `moo/familiar-agent` is a server side for ToastStunt/LambdaCore. It runs on the MCP
    feature (moved from `examples/moo` to `moo/mcp-feature`), which now passes other packages'
    messages to the objects registered with `:set_handler`.
- GTK: `/agent` too, with the agent in a terminal (VTE) in the side dock. VTE for GTK 4 is a
  new dependency (the default `agents` feature).
- macOS: `/agent` too, in a terminal (SwiftTerm) in the side dock. Agents' programs are looked
  for on the login shell's PATH, since an app opened from the Finder doesn't get it.
- Windows: `/agent` too. The engine runs the agent in a ConPTY (for apps that ask, with
  `"agents": "pty"`), and the side dock shows it with xterm.js in WebView2.
- Only clients that can run agents offer MOOs `dns-com-familiar-agent`. The agent server and
  launching live in a new crate, `familiar-agent`, for every frontend.
- The terminal client's embedded terminal passes on a command's environment and working
  folder.

## 0.4.0 (2026-11-24)

- Every client: the triggers, aliases and macros editor can add a set of examples to learn
  from ("Add Examples"; `x` in the terminal client). They aren't for any one game: each shows
  one thing, from highlighting a word to answering with what a regular expression matched,
  aliases with `$1` and `$*`, and a macro that types into the input box. Triggers and aliases
  arrive switched off, and nothing already there is replaced.
- Windows: a native app, written for Windows 11 (C# and WinUI 3) on the same engine, in place
  of the GTK client. It follows the system's light and dark modes, keeps worlds in title-bar
  tabs, and has what the GTK client lacked there: web views (through WebView2) and spell
  checking in every language Windows has. Ctrl+F finds text in the output, a bar says why a
  connection ended and offers to reconnect, and the window opens where it was last time. The
  installer upgrades an older install in place.
- Terminal client: `/editor syntax` installs MOO highlighting for `$EDITOR`: a syntax file for
  micro, a `moo-mode` for Emacs (which also indents blocks, and needs one line in your init
  file, which it shows), or for Vim and Neovim, a fuller one than they ship (Stunt's
  try/except, maps, `$#colours`, `b"bytes"`, built-in functions and verb calls). It leaves a
  file that's there already alone unless you add `force`. The first time a MOO edit closes in
  one of those editors without it, a hint says so, once.

## 0.3.1 (2026-11-23)

- Bouncer support: worlds can connect through an mbnc bouncer (a `[bouncer]` section in
  `config.toml`, and a per-world switch), which keeps the character online while you're away
  and replays what arrived, with timestamps, when you return. `/disconnect` detaches; `/bnc`
  lists sessions, `/bnc reconnect` reconnects the bouncer to the world and `/bnc close` logs off.
- Fixed: in the terminal client, selecting text with the mouse stopped at the top of the output.
  Holding the selection at the top or bottom edge now scrolls, so it can go on.
- Fixed: in the terminal client, selecting downwards while scrolled back jumped over the
  "scrolled back" line into the live lines under it, selecting everything in between.

## 0.3.0 (2026-11-23)

- Terminal client: `/editor sidebar` opens `$EDITOR` beside the world, where the sidebar goes,
  when that world isn't using its sidebar. F6 switches between the editor and the world, and
  F7 moves an open editor between the sidebar and a tab.
- macOS: a native app, with what the GTK client has, web views included. Worlds open as
  window tabs; speech, spell checking and notifications are the system's.
- macOS: the terminal client, as one file for Apple silicon and Intel. Speech uses the system
  voice, sounds play through `afplay`, and notifications come from Notification Center.

## 0.2.1 (2026-11-22)

- Terminal client: dragging over the output selects text and copies it on release. Wrapped
  lines are copied whole, and the selection stays with its text as the output scrolls.
- Terminal client: `$EDITOR` opens in a tab of its own instead of taking over the terminal, so
  the world stays in view and several edits can be open at once. `/editor fullscreen` brings
  the old behaviour back (the `editor_in_tab` setting).
- Terminal client: a window for triggers, aliases, keyboard macros and variables (F2 or
  `/rules`), like the GUI's.

## 0.2.0 (2026-11-22)

- Windows: the GTK client as an installer and a zip with everything included, and the terminal
  client as a single file. Speech uses the Windows voice; web views aren't available there.
- Preferences dialog (Ctrl+,), with font selection. Worlds can have their own font.
- Terminal client: the 16 basic colours are drawn exactly rather than with the terminal theme's
  versions (`/colors terminal` brings those back).
- Fixed: the first lines of a session (the "Connecting" notices, and whatever a fast server sent
  straight away) could be hidden above the top of the output for good.
- Fixed: memory used by links in the output was never freed as they scrolled away.
- Fixed: in the terminal client, sounds from a URL couldn't be stopped.
- Media downloads time out instead of waiting on a silent server for ever.
- Addresses typed into a connect box can be IPv6 (`[::1]:4000`) and can end in `tls`.
- Maps and logs moved from `%APPDATA%` to `%LOCALAPPDATA%` on Windows (no change on Linux).

## 0.1.0 (2026-11-22)

First release.

- GTK 4 client (`familiar`) and terminal client (`familiar-tui`) on one shared engine.
- Telnet with GMCP, MSSP, MCCP2, NAWS, MTTS and CHARSET; TLS; UTF-8 with a Windows-1252 fallback.
- Triggers, aliases, keyboard macros, variables, logging and JavaScript (QuickJS) scripting.
- GMCP Beip.Stats panes, Client.Media sounds, web views, Room.Info automapping and walking,
  tilemaps, avatars and inline images.
- MCP 2.1: simpleedit with a MOO/MUF program editor, awns-status and awns-ping.
- Puppets, text to speech and spell checking.
- Imports BeipMU's Config.txt, .atlas maps and scripts.
