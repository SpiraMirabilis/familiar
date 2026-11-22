//! What arrives from the server: session events, GMCP, and each line through triggers and highlights.

use super::*;

impl Engine {
    pub fn handle_event(&mut self, event: SessionEvent) -> Vec<Output> {
        let mut out = Vec::new();
        match event {
            SessionEvent::Connected { addr } => {
                out.push(Output::Status(Status::Connected));
                out.push(Output::Notice(format!("Connected to {addr}")));
                let login = self.character.as_ref().map(|c| c.login.clone()).unwrap_or_default();
                for line in login.lines().filter(|l| !l.is_empty()) {
                    // Client commands work in logins (e.g. /delay 2s connect bob pw); "//"
                    // sends a line that starts with "/".
                    match line.strip_prefix('/') {
                        Some(rest) if !rest.starts_with('/') => self.client_command(rest, &mut out),
                        _ => {
                            self.send_raw(line.strip_prefix('/').unwrap_or(line));
                        }
                    }
                }
                let on_connect: Vec<String> = self.puppets().filter(|p| p.open_on_connect).map(|p| p.name.clone()).collect();
                for name in on_connect {
                    self.open_puppet(&name, false, &mut out);
                }
                self.script_event("connect", &mut out);
            }
            SessionEvent::Line(text) => self.incoming(&text, None, &mut out),
            SessionEvent::Prompt(text) => self.incoming(&text, Some(true), &mut out),
            // MCP lines are never prompts; the whole line is handled when it arrives.
            SessionEvent::PartialLine(text) if self.world.mcp && familiar_ansi::AnsiParser::new().parse_line(&text).text.starts_with("#$") => {}
            SessionEvent::PartialLine(text) => {
                // Parse with a copy of the ANSI state: the full line will be parsed again later.
                let line = self.ansi.clone().parse_line(&text);
                out.push(Output::Prompt { line, terminated: false });
                out.push(Output::Activity { important: false });
            }
            SessionEvent::ServerEcho(on) => {
                self.server_echo = on;
                out.push(Output::PasswordMode(on));
            }
            SessionEvent::EncodingChanged(e) => out.push(Output::Notice(format!("Server switched encoding to {}", e.name()))),
            SessionEvent::Notice(text) => out.push(Output::Notice(text)),
            SessionEvent::Gmcp { package, data } => self.gmcp(package, data, &mut out),
            SessionEvent::Mssp(_) | SessionEvent::Compression(_) => {}
            SessionEvent::Disconnected { error } => {
                let reason = match &error {
                    Some(e) => format!("Disconnected: {e}"),
                    None => "Disconnected".to_string(),
                };
                out.push(Output::Notice(format!("{reason}. Press Ctrl+R or type /reconnect to reconnect.")));
                self.log(&format!("--- {reason} ---"), &mut out);
                out.push(Output::Status(Status::Disconnected(error)));
                if self.server_echo {
                    self.server_echo = false;
                    out.push(Output::PasswordMode(false));
                }
                self.handle = None;
                if let Err(e) = self.save_map() {
                    out.push(Output::Notice(format!("Couldn't save the map: {e}")));
                }
                self.script_event("disconnect", &mut out);
            }
        }
        out
    }

    pub(super) fn gmcp(&mut self, package: String, data: String, out: &mut Vec<Output>) {
        if self.gmcp_debug {
            out.push(Output::Notice(format!("GMCP ← {package} {data}")));
        }
        let lower = package.to_ascii_lowercase();
        match lower.as_str() {
            "beip.stats" => match self.stats.apply(&data) {
                Ok(panes) => {
                    for pane in panes {
                        let data = self.stats.pane(&pane).cloned();
                        out.push(Output::Stats { pane, data });
                    }
                }
                Err(e) => out.push(Output::Notice(e)),
            },
            "beip.line.image-url" => {
                let url: Option<String> = serde_json::from_str(&data).ok();
                self.next_avatar = url.map(|url| Avatar { url: Some(url), ..Avatar::default() });
            }
            "beip.line.id" => {
                if let Ok(id) = serde_json::from_str::<String>(&data) {
                    let known = self.avatars.get(&id).cloned();
                    if known.is_none() && self.avatars_requested.insert(id.clone()) {
                        let request = serde_json::to_string(&id).unwrap_or_default();
                        if let Some(handle) = &self.handle {
                            handle.send_gmcp("beip.id.request", Some(request));
                        }
                    }
                    self.next_avatar = Some(known.unwrap_or(Avatar { id: Some(id), ..Avatar::default() }));
                }
            }
            "beip.ids" => {
                if let Ok(serde_json::Value::Object(ids)) = serde_json::from_str::<serde_json::Value>(&data) {
                    for (id, fields) in ids {
                        let avatar = self.avatars.entry(id.clone()).or_insert_with(|| Avatar { id: Some(id.clone()), ..Avatar::default() });
                        let s = |k: &str| fields.get(k).and_then(|v| v.as_str()).map(str::to_string);
                        // Only the fields sent change.
                        if let Some(url) = s("url") {
                            avatar.url = Some(url);
                        }
                        if let Some(text) = s("hover-text") {
                            avatar.hover = Some(text);
                        }
                        if let Some(url) = s("click-url") {
                            avatar.click_url = Some(url);
                        }
                        out.push(Output::AvatarUpdated(avatar.clone()));
                    }
                }
            }
            "beip.tilemap.info" | "beip.tilemap.data" => {
                let result = if lower.ends_with("info") { self.tilemaps.info(&data) } else { self.tilemaps.data(&data) };
                match result {
                    Ok(names) => {
                        for name in names {
                            if let Some(map) = self.tilemaps.get(&name) {
                                out.push(Output::Tilemap { name, map: map.clone() });
                            }
                        }
                    }
                    Err(e) => out.push(Output::Notice(e)),
                }
            }
            "room.info" => match RoomInfo::parse(&data) {
                Some(info) => {
                    self.room_info_seen = true;
                    let last = self.last_move.take();
                    if let Some(room) = self.map.apply_room_info(&info, self.current_room, last.as_deref()) {
                        self.current_room = Some(room);
                    }
                    out.push(Output::MapChanged);
                }
                None => out.push(Output::Notice("Room.Info: couldn't read it".into())),
            },
            "webview.open" => match parse_webview_open(&data) {
                Ok(open) => out.push(Output::WebView(WebViewCommand::Open(open))),
                Err(e) => out.push(Output::Notice(e)),
            },
            "webview.close" => {
                let id = serde_json::from_str::<serde_json::Value>(&data)
                    .ok()
                    .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_string));
                if let Some(id) = id {
                    out.push(Output::WebView(WebViewCommand::Close { id }));
                }
            }
            _ => {
                if let Some(message) = lower.strip_prefix("client.media.") {
                    match self.media.handle(message, &data) {
                        Ok(commands) => out.extend(commands.into_iter().map(Output::Media)),
                        Err(e) => out.push(Output::Notice(e)),
                    }
                }
            }
        }
        if let Some(script) = self.script.as_mut() {
            script.on_gmcp(&package, &data);
            self.drain_script(out);
        }
        out.push(Output::Gmcp { package, data });
    }

    /// Handles text as if the server had sent it, e.g. from a web view.
    pub fn inject_line(&mut self, text: &str) -> Vec<Output> {
        let mut out = Vec::new();
        self.incoming(text, None, &mut out);
        out
    }

    /// Looks for "Line 3:  syntax error" (MOO) or "Error in line 3: ..." (FuzzBall MUF) soon
    /// after an upload.
    pub(super) fn check_compile_error(&mut self, text: &str, out: &mut Vec<Output>) {
        let Some((reference, until)) = &self.compile_watch else { return };
        if Instant::now() > *until {
            self.compile_watch = None;
            return;
        }
        // Cores that colour every line would otherwise hide "Line 3:" behind escape codes.
        let plain;
        let text = if text.contains('\x1b') {
            plain = familiar_ansi::AnsiParser::new().parse_line(text).text;
            plain.as_str()
        } else {
            text
        };
        let rest = text
            .trim_start()
            .strip_prefix("Line ")
            .or_else(|| text.trim_start().strip_prefix("Error in line "));
        let Some(rest) = rest else { return };
        let Some((number, message)) = rest.split_once(':') else { return };
        if let Ok(line) = number.trim().parse::<u32>() {
            out.push(Output::CompileError { reference: reference.clone(), line, message: message.trim().to_string() });
        }
    }

    /// Handles a line (prompt = None) or prompt (prompt = Some(terminated)) from the server.
    pub(super) fn incoming(&mut self, text: &str, prompt: Option<bool>, out: &mut Vec<Output>) {
        let quoted;
        // Some cores' :tell wraps every line in colour codes, which would hide the #$# that
        // marks an MCP line; look past them.
        // Such cores often add a space before the reset code too, so trailing spaces go as well.
        let uncoloured;
        let mcp_text = if self.world.mcp && text.contains('\x1b') {
            uncoloured = familiar_ansi::AnsiParser::new().parse_line(text).text;
            if uncoloured.starts_with("#$") { uncoloured.trim_end() } else { text }
        } else {
            text
        };
        if self.world.mcp && self.mcp_debug && mcp_text.starts_with("#$") {
            // The raw line, with escape characters made visible.
            out.push(Output::Notice(format!("MCP ← {}", text.replace('\x1b', "␛"))));
        }
        let text = if self.world.mcp {
            let text = mcp_text;
            let mut events = Vec::new();
            let (disposition, sends) = self.mcp.on_line(text, &mut events);
            for line in sends {
                if self.mcp_debug {
                    out.push(Output::Notice(format!("MCP → {line}")));
                }
                self.send_raw(&line);
            }
            for event in events {
                out.push(match event {
                    McpEvent::Status(text) => Output::StatusText(text),
                    McpEvent::Edit { reference, name, kind, content } => {
                        let language = (kind == "moo-code").then(|| self.program_language());
                        Output::Edit(EditRequest { reference, name, kind, content, language })
                    }
                    McpEvent::Notice(text) => Output::Notice(text),
                });
            }
            match disposition {
                Disposition::Hide => return,
                Disposition::Quoted(text) => {
                    quoted = text;
                    quoted.as_str()
                }
                Disposition::Display => text,
            }
        } else {
            text
        };
        out.push(Output::Received(text.to_string()));
        self.check_compile_error(text, out);
        if self.map_tracking && !self.room_info_seen
            && let Some(current) = self.current_room
            && let Some(next) = self.map.track(current, text, self.last_move.as_deref())
        {
            self.current_room = Some(next);
            self.last_move = None;
            out.push(Output::MapChanged);
        }
        if let Some(script) = self.script.as_mut() {
            script.on_receive(text);
            self.drain_script(out);
        }
        let mut line = self.ansi.parse_line(text);
        line.linkify();
        let mut plain = line.text.clone();

        // A puppet's line goes to its tab (and its own rules, not the main window's triggers).
        if prompt.is_none()
            && let Some((i, start, end)) = self.match_puppet(&plain)
        {
            let puppet = self.puppets[i].0.clone();
            let open = self.open_puppets.contains(&puppet.name);
            if open || puppet.auto_open {
                if !open {
                    self.open_puppet(&puppet.name, false, out);
                }
                if puppet.hide_prefix {
                    line.remove_range(start, end);
                }
                self.log(&plain, out);
                out.push(Output::PuppetLine { name: puppet.name, line });
                return;
            }
        }

        let mut gag = false;
        let mut gag_log = false;
        let mut important = false;
        let mut spawns: Vec<String> = Vec::new();
        let mut sends: Vec<String> = Vec::new();
        let mut calls: Vec<(String, Vec<String>)> = Vec::new();
        let mut speech: Vec<String> = Vec::new();

        for i in 0..self.rules.triggers.len() {
            let t = &self.rules.triggers[i];
            if t.suspended {
                continue;
            }
            let Ok(Some(caps)) = t.regex.captures(plain.as_str()) else { continue };
            let trigger = &t.trigger;
            if let Some(h) = &trigger.highlight {
                if h.whole_line {
                    line.restyle(0, plain.len(), |s| apply_highlight(s, h));
                } else if h.groups && t.regex.captures_len() > 1 {
                    for c in t.regex.captures_iter(plain.as_str()).flatten() {
                        for m in (1..c.len()).filter_map(|g| c.get(g)) {
                            line.restyle(m.start(), m.end(), |s| apply_highlight(s, h));
                        }
                    }
                } else {
                    for m in t.regex.find_iter(plain.as_str()).flatten() {
                        line.restyle(m.start(), m.end(), |s| apply_highlight(s, h));
                    }
                }
            }
            gag |= trigger.gag;
            gag_log |= trigger.gag_log;
            important |= trigger.important;
            if !trigger.spawn.is_empty() && !spawns.contains(&trigger.spawn) {
                spawns.push(trigger.spawn.clone());
            }
            if !trigger.sound.is_empty() {
                out.push(Output::PlaySound(trigger.sound.clone()));
            }
            if trigger.notify {
                out.push(Output::Notify { title: self.title(), body: plain.clone() });
            }
            let stop = trigger.stop;
            if !trigger.speak.is_empty() {
                let caps = Captures::from_regex(&t.regex, &caps);
                speech.push(expand(&trigger.speak, &caps, |name| self.variable(name)));
            }
            if !trigger.script.is_empty() {
                let mut args = vec![plain.clone()];
                if trigger.script.starts_with("beipmu:") {
                    // BeipMU passes match ranges (in UTF-16 units, as JavaScript counts).
                    let utf16 = |byte: usize| plain[..byte].encode_utf16().count().to_string();
                    for g in 0..caps.len() {
                        match caps.get(g) {
                            Some(m) => args.extend([utf16(m.start()), utf16(m.end())]),
                            None => args.extend(["-1".to_string(), "-1".to_string()]),
                        }
                    }
                } else {
                    args.extend((1..caps.len()).map(|i| caps.get(i).map_or(String::new(), |m| m.as_str().to_string())));
                }
                calls.push((trigger.script.clone(), args));
            }
            let mut edits: Vec<(usize, usize, String)> = Vec::new();
            if let Some(replacement) = &trigger.replace {
                edits = t
                    .regex
                    .captures_iter(plain.as_str())
                    .flatten()
                    .filter_map(|c| {
                        let m = c.get(0).filter(|m| m.start() < m.end())?;
                        Some((m.start(), m.end(), expand(replacement, &Captures::from_regex(&t.regex, &c), |name| self.variable(name))))
                    })
                    .collect();
            }
            if !trigger.send.is_empty() {
                let caps = Captures::from_regex(&t.regex, &caps);
                let template = trigger.send.clone();
                if self.note_trigger_send(i, out) {
                    let text = expand(&template, &caps, |name| self.variable(name));
                    sends.extend(text.lines().map(str::to_string));
                }
            }
            if !edits.is_empty() {
                for (start, end, text) in edits.into_iter().rev() {
                    line.replace_range(start, end, &text);
                }
                // Later triggers see the changed line.
                plain = line.text.clone();
            }
            if stop {
                break;
            }
        }

        if let Some(script) = self.script.as_mut() {
            for (function, args) in &calls {
                script.call(function, args);
            }
            if !script.on_line(&plain) {
                gag = true;
            }
        }
        if !gag_log {
            self.log(&plain, out);
        }
        if line.bell {
            out.push(Output::Bell);
        }
        let avatar = self.next_avatar.take();
        if !gag {
            match prompt {
                Some(terminated) => out.push(Output::Prompt { line: line.clone(), terminated }),
                None => {
                    if let Some(avatar) = avatar {
                        out.push(Output::LineAvatar(avatar));
                    }
                    out.push(Output::Line { line: line.clone(), target: Target::Main });
                    if self.settings.speak_output && speech.is_empty() {
                        speech.push(plain.clone());
                    }
                    if self.settings.inline_images {
                        for url in image_links(&plain).into_iter().take(3) {
                            out.push(Output::InlineImage { url });
                        }
                    }
                }
            }
        }
        for name in spawns {
            out.push(Output::Line { line: line.clone(), target: Target::Spawn(name) });
        }
        out.extend(speech.into_iter().map(Output::Speak));
        out.push(Output::Activity { important });
        self.drain_script(out);
        for command in sends {
            self.send_raw(&command);
            out.push(Output::Sent(command.clone()));
            if self.settings.local_echo {
                out.push(Output::Echo(command));
            }
        }
    }
}

/// Links in `text` that point at images, by file extension.
pub(super) fn image_links(text: &str) -> Vec<String> {
    familiar_ansi::find_urls(text)
        .into_iter()
        .map(|(a, b)| text[a..b].to_string())
        .filter(|url| {
            let path = url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
            [".png", ".jpg", ".jpeg", ".gif", ".webp"].iter().any(|ext| path.ends_with(ext))
        })
        .collect()
}

pub(super) fn parse_webview_open(json: &str) -> Result<WebViewOpen, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("webview.open: {e}"))?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let n = |k: &str| v.get(k).and_then(|x| x.as_i64()).map(|x| x.clamp(50, 10_000) as i32);
    let url = s("url");
    let source = s("source");
    if url.is_none() && source.is_none() {
        return Err("webview.open: needs a url or source".into());
    }
    let headers = v
        .get("http-request-headers")
        .and_then(|h| h.as_object())
        .map(|h| h.iter().map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_string))).collect())
        .unwrap_or_default();
    Ok(WebViewOpen {
        id: s("id").or_else(|| url.clone()).unwrap_or_else(|| "Web View".into()),
        url,
        source,
        dock: s("dock"),
        width: n("width"),
        height: n("height"),
        headers,
        from_server: true,
    })
}

pub(super) fn parse_hex(color: &str) -> Option<Color> {
    let hex = color.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some(Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

pub(super) fn apply_highlight(style: &mut familiar_ansi::Style, h: &Highlight) {
    if let Some(fg) = h.fg.as_deref().and_then(parse_hex) {
        style.fg = fg;
    }
    if let Some(bg) = h.bg.as_deref().and_then(parse_hex) {
        style.bg = bg;
    }
    style.bold |= h.bold;
    style.italic |= h.italic;
    style.underline |= h.underline;
}
