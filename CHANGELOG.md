# Changelog

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
