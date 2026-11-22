use mu_config::{Alias, Character, Config, Highlight, Macro, Pattern, Puppet, Rules, Scope, Trigger, World};
use mu_net::{SessionCommand, SessionEvent, SessionHandle};
use tokio::sync::mpsc::UnboundedReceiver;

use super::*;

fn pattern(p: &str) -> Pattern {
    Pattern { pattern: p.into(), ..Pattern::default() }
}

fn regex(p: &str) -> Pattern {
    Pattern { pattern: p.into(), regex: true, ..Pattern::default() }
}

/// A config with world "W" and character "C". `global`, `world` and `character` rules go in the
/// matching scopes.
fn config(global: Rules, world: Rules, character: Rules) -> Config {
    Config {
        rules: global,
        worlds: vec![World {
            name: "W".into(),
            host: "localhost".into(),
            rules: world,
            characters: vec![Character { name: "C".into(), rules: character, ..Character::default() }],
            ..World::default()
        }],
        ..Config::default()
    }
}

fn engine(config: &Config) -> (Engine, UnboundedReceiver<SessionCommand>) {
    let world = config.worlds[0].clone();
    let character = world.characters[0].clone();
    let (mut engine, notices) = Engine::new(config, world, Some(character));
    assert!(notices.is_empty(), "{notices:?}");
    engine.detach_map();
    let (handle, rx) = SessionHandle::channel();
    engine.attach(handle);
    (engine, rx)
}

fn sent(rx: &mut UnboundedReceiver<SessionCommand>) -> Vec<String> {
    let mut lines = Vec::new();
    while let Ok(cmd) = rx.try_recv() {
        if let SessionCommand::SendLine(line) = cmd {
            lines.push(line);
        }
    }
    lines
}

fn main_lines(out: &[Output]) -> Vec<&StyledLine> {
    out.iter()
        .filter_map(|o| match o {
            Output::Line { line, target: Target::Main } => Some(line),
            _ => None,
        })
        .collect()
}

fn notices(out: &[Output]) -> Vec<&str> {
    out.iter()
        .filter_map(|o| match o {
            Output::Notice(n) => Some(n.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn trigger_highlights_and_sends_with_captures_and_variables() {
    let global = Rules {
        triggers: vec![Trigger {
            matcher: regex(r"(\w+) attacks you"),
            highlight: Some(Highlight { fg: Some("#ff0000".into()), bold: true, ..Highlight::default() }),
            send: "kill $1\nsay %battlecry%".into(),
            important: true,
            ..Trigger::default()
        }],
        ..Rules::default()
    };
    let character = Rules { variables: [("battlecry".to_string(), "for glory".to_string())].into(), ..Rules::default() };
    let (mut e, mut rx) = engine(&config(global, Rules::default(), character));

    let out = e.handle_event(SessionEvent::Line("The orc attacks you!".into()));
    let line = main_lines(&out)[0];
    let red: Vec<_> = line.spans.iter().filter(|s| s.style.fg == Color::Rgb(255, 0, 0)).map(|s| line.span_text(s)).collect();
    assert_eq!(red, vec!["orc attacks you"]);
    assert!(out.contains(&Output::Activity { important: true }));
    assert_eq!(sent(&mut rx), vec!["kill orc", "say for glory"]);
    assert!(out.contains(&Output::Echo("kill orc".into())));
}

#[test]
fn whole_line_highlight() {
    let global = Rules {
        triggers: vec![Trigger {
            matcher: pattern("page"),
            highlight: Some(Highlight { bg: Some("#000080".into()), whole_line: true, ..Highlight::default() }),
            ..Trigger::default()
        }],
        ..Rules::default()
    };
    let (mut e, _rx) = engine(&config(global, Rules::default(), Rules::default()));
    let out = e.handle_event(SessionEvent::Line("Bob pages: hi".into()));
    let line = main_lines(&out)[0];
    assert_eq!(line.spans.len(), 1);
    assert_eq!(line.spans[0].style.bg, Color::Rgb(0, 0, 128));
}

#[test]
fn gag_and_spawn() {
    let global = Rules {
        triggers: vec![Trigger { matcher: regex(r"^\[OOC\]"), gag: true, spawn: "OOC".into(), ..Trigger::default() }],
        ..Rules::default()
    };
    let (mut e, _rx) = engine(&config(global, Rules::default(), Rules::default()));
    let out = e.handle_event(SessionEvent::Line("[OOC] Bob: hello".into()));
    assert!(main_lines(&out).is_empty());
    assert!(out.iter().any(|o| matches!(o, Output::Line { target: Target::Spawn(name), line } if name == "OOC" && line.text == "[OOC] Bob: hello")));

    let out = e.handle_event(SessionEvent::Line("normal".into()));
    assert_eq!(main_lines(&out).len(), 1);
}

#[test]
fn stop_processing_and_priority() {
    // The character's trigger runs first and stops the global one.
    let global = Rules { triggers: vec![Trigger { matcher: pattern("x"), send: "global".into(), ..Trigger::default() }], ..Rules::default() };
    let character = Rules {
        triggers: vec![Trigger { matcher: pattern("x"), send: "character".into(), stop: true, ..Trigger::default() }],
        ..Rules::default()
    };
    let (mut e, mut rx) = engine(&config(global, Rules::default(), character));
    e.handle_event(SessionEvent::Line("x".into()));
    assert_eq!(sent(&mut rx), vec!["character"]);
}

#[test]
fn prompts_run_triggers_but_partial_lines_do_not() {
    let global = Rules { triggers: vec![Trigger { matcher: pattern("HP:"), send: "ok".into(), ..Trigger::default() }], ..Rules::default() };
    let (mut e, mut rx) = engine(&config(global, Rules::default(), Rules::default()));
    let out = e.handle_event(SessionEvent::PartialLine("HP: 10>".into()));
    assert!(matches!(out[0], Output::Prompt { terminated: false, .. }));
    assert!(sent(&mut rx).is_empty());
    let out = e.handle_event(SessionEvent::Prompt("HP: 10>".into()));
    assert!(out.iter().any(|o| matches!(o, Output::Prompt { terminated: true, .. })));
    assert_eq!(sent(&mut rx), vec!["ok"]);
}

#[test]
fn runaway_trigger_is_suspended() {
    let global = Rules { triggers: vec![Trigger { matcher: pattern("echo"), send: "echo".into(), ..Trigger::default() }], ..Rules::default() };
    let (mut e, mut rx) = engine(&config(global, Rules::default(), Rules::default()));
    let mut all = Vec::new();
    for _ in 0..30 {
        all.extend(e.handle_event(SessionEvent::Line("echo".into())));
    }
    assert_eq!(sent(&mut rx).len(), MAX_SENDS);
    assert_eq!(notices(&all).iter().filter(|n| n.contains("fired more than")).count(), 1);
}

#[test]
fn aliases_plain_regex_and_priority() {
    let global = Rules {
        aliases: vec![
            Alias { matcher: pattern("k"), replacement: "kill $1".into(), ..Alias::default() },
            Alias { matcher: regex(r"^go (\w+) (\w+)$"), replacement: "$1\n$2".into(), ..Alias::default() },
        ],
        ..Rules::default()
    };
    let character = Rules {
        aliases: vec![Alias { matcher: pattern("K"), replacement: "kick $*".into(), ..Alias::default() }],
        ..Rules::default()
    };
    let (mut e, mut rx) = engine(&config(global, Rules::default(), character));
    e.submit("k big orc", false);
    e.submit("go north east", false);
    e.submit("look", false);
    assert_eq!(sent(&mut rx), vec!["kick big orc", "north", "east", "look"]);
}

#[test]
fn set_variable_is_saved_at_the_most_specific_scope() {
    let (mut e, mut rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    let out = e.submit("/set target the orc", false);
    assert!(out.contains(&Output::VariablesChanged {
        scope: Scope::Character { world: "W".into(), character: "C".into() },
        variables: [("target".to_string(), "the orc".to_string())].into(),
    }));
    assert_eq!(e.variable("target").as_deref(), Some("the orc"));
    let out = e.submit("/unset target", false);
    assert!(out.iter().any(|o| matches!(o, Output::VariablesChanged { variables, .. } if variables.is_empty())));
    assert!(sent(&mut rx).is_empty());
}

#[test]
fn client_commands() {
    let (mut e, mut rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    assert_eq!(e.submit("/clear", false), vec![Output::ClearOutput]);
    assert!(notices(&e.submit("/bogus", false))[0].contains("Unknown command /bogus"));
    e.submit("//me waves", false);
    e.submit("", false);
    assert_eq!(sent(&mut rx), vec!["/me waves", ""]);
    assert!(notices(&e.submit("/help", false))[0].contains("/set"));
    assert_eq!(e.submit("/reconnect", false), vec![Output::Notice("Already connected.".into())]);
}

#[test]
fn secret_input_is_sent_verbatim_without_echo() {
    let global = Rules { aliases: vec![Alias { matcher: pattern("hunter2"), replacement: "x".into(), ..Alias::default() }], ..Rules::default() };
    let (mut e, mut rx) = engine(&config(global, Rules::default(), Rules::default()));
    assert!(e.submit("hunter2", true).is_empty());
    assert_eq!(sent(&mut rx), vec!["hunter2"]);
}

#[test]
fn not_connected() {
    let cfg = config(Rules::default(), Rules::default(), Rules::default());
    let (mut e, _) = Engine::new(&cfg, cfg.worlds[0].clone(), None);
    assert!(notices(&e.submit("look", false))[0].starts_with("Not connected"));
    assert_eq!(e.submit("/reconnect", false), vec![Output::Reconnect]);
}

#[test]
fn macros_and_links() {
    let global = Rules { macros: vec![Macro { key: "KP_8".into(), command: "north".into(), insert: false }], ..Rules::default() };
    let (mut e, mut rx) = engine(&config(global, Rules::default(), Rules::default()));
    assert!(e.run_macro(&KeyChord::new("kp_8", false, false, false)).is_some());
    assert_eq!(sent(&mut rx), vec!["north"]);
    assert!(e.run_macro(&KeyChord::new("KP_8", true, false, false)).is_none());

    e.follow_link("send:look%20at%20me");
    assert_eq!(sent(&mut rx), vec!["look at me"]);
    assert_eq!(e.follow_link("prompt:say%20"), vec![Output::SetInput("say ".into())]);
    assert_eq!(e.follow_link("https://x.org"), vec![Output::OpenUri("https://x.org".into())]);
}

#[test]
fn login_on_connect_and_disconnect() {
    let mut cfg = config(Rules::default(), Rules::default(), Rules::default());
    cfg.worlds[0].characters[0].login = "connect C pw".into();
    let (mut e, mut rx) = engine(&cfg);
    let out = e.handle_event(SessionEvent::Connected { addr: "127.0.0.1:4000".parse().unwrap() });
    assert!(out.contains(&Output::Status(Status::Connected)));
    assert_eq!(sent(&mut rx), vec!["connect C pw"]);
    // The login isn't echoed.
    assert!(!out.iter().any(|o| matches!(o, Output::Echo(_))));

    e.handle_event(SessionEvent::ServerEcho(true));
    let out = e.handle_event(SessionEvent::Disconnected { error: None });
    assert!(out.contains(&Output::PasswordMode(false)));
    assert!(!e.is_connected());
}

#[test]
fn reload_picks_up_new_rules() {
    let mut cfg = config(Rules::default(), Rules::default(), Rules::default());
    let (mut e, mut rx) = engine(&cfg);
    cfg.worlds[0].rules.triggers.push(Trigger { matcher: pattern("hi"), send: "wave".into(), ..Trigger::default() });
    cfg.worlds[0].rules.triggers.push(Trigger { matcher: regex("("), ..Trigger::default() });
    let out = e.reload(&cfg);
    assert_eq!(out.len(), 1, "the broken regex is reported");
    e.handle_event(SessionEvent::Line("hi".into()));
    assert_eq!(sent(&mut rx), vec!["wave"]);
}

fn gmcp(package: &str, data: &str) -> SessionEvent {
    SessionEvent::Gmcp { package: package.into(), data: data.into() }
}

#[test]
fn gmcp_stats_media_and_debug() {
    let (mut e, _rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    let out = e.handle_event(gmcp("Beip.Stats", r#"{"Player":{"values":{"HP":{"int":5}}}}"#));
    assert!(out.iter().any(|o| matches!(o, Output::Stats { pane, data: Some(p) } if pane == "Player" && p.values.len() == 1)));
    assert!(out.iter().any(|o| matches!(o, Output::Gmcp { package, .. } if package == "Beip.Stats")));

    e.handle_event(gmcp("Client.Media.Default", r#"{"url":"https://x.org/"}"#));
    let out = e.handle_event(gmcp("client.media.play", r#"{"name":"a.wav"}"#));
    assert!(out.iter().any(|o| matches!(o, Output::Media(crate::media::MediaCommand::Play { url, .. }) if url == "https://x.org/a.wav")));
    let out = e.handle_event(gmcp("Client.Media.Play", "{}"));
    assert!(!notices(&out).is_empty());

    assert_eq!(notices(&e.submit("/gmcp", false)), vec!["GMCP traffic is shown"]);
    let out = e.handle_event(gmcp("Char.Vitals", r#"{"hp":1}"#));
    assert_eq!(notices(&out), vec![r#"GMCP ← Char.Vitals {"hp":1}"#]);
}

#[test]
fn webview_requests() {
    let (mut e, _rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    let out = e.handle_event(gmcp(
        "WebView.Open",
        r#"{"id":"Editor","dock":"right","url":"https://x.org/","http-request-headers":{"X-Token":"abc"}}"#,
    ));
    let open = out.iter().find_map(|o| match o {
        Output::WebView(WebViewCommand::Open(open)) => Some(open.clone()),
        _ => None,
    });
    let open = open.expect("open command");
    assert_eq!((open.id.as_str(), open.dock.as_deref(), open.from_server), ("Editor", Some("right"), true));
    assert_eq!(open.headers, vec![("X-Token".to_string(), "abc".to_string())]);
    let out = e.handle_event(gmcp("webview.close", r#"{"id":"Editor"}"#));
    assert!(out.contains(&Output::WebView(WebViewCommand::Close { id: "Editor".into() })));
    assert!(!notices(&e.handle_event(gmcp("webview.open", "{}"))).is_empty());

    let out = e.submit(r#"/webview url="https://beipdev.github.io/x.html""#, false);
    assert!(matches!(&out[0], Output::WebView(WebViewCommand::Open(o)) if o.url.as_deref() == Some("https://beipdev.github.io/x.html") && !o.from_server));
}

#[test]
fn mcp_is_handled_before_display_and_triggers() {
    let global = Rules { triggers: vec![Trigger { matcher: pattern("#$#"), send: "oops".into(), ..Trigger::default() }], ..Rules::default() };
    let (mut e, mut rx) = engine(&config(global, Rules::default(), Rules::default()));
    let out = e.handle_event(SessionEvent::Line("#$#mcp version: 2.1 to: 2.1".into()));
    assert!(main_lines(&out).is_empty());
    let sent_lines = sent(&mut rx);
    assert!(sent_lines[0].starts_with("#$#mcp authentication-key: "));
    let key = sent_lines[0].split_whitespace().nth(2).unwrap().to_string();

    let out = e.handle_event(SessionEvent::Line(format!(r#"#$#dns-com-awns-status {key} text: "On duty""#)));
    assert!(out.contains(&Output::StatusText("On duty".into())));
    let out = e.handle_event(SessionEvent::Line(format!(
        r##"#$#dns-org-mud-moo-simpleedit-content {key} reference: "#1.d" name: "desc" type: string-list content*: "" _data-tag: 7"##
    )));
    assert!(out.is_empty());
    e.handle_event(SessionEvent::Line("#$#* 7 content: hello".into()));
    let out = e.handle_event(SessionEvent::Line("#$#: 7".into()));
    let Some(Output::Edit(request)) = out.first() else { panic!("{out:?}") };
    assert_eq!(request.content, "hello");
    // string-list content (a description) isn't code.
    assert_eq!(request.language, None);
    assert!(sent(&mut rx).is_empty(), "the trigger never saw MCP lines");

    e.save_edit(request, "changed");
    let lines = sent(&mut rx);
    assert!(lines[0].starts_with(&format!("#$#dns-org-mud-moo-simpleedit-set {key} reference: \"#1.d\"")));
    assert!(lines[1].ends_with("content: changed"));

    // Quoted lines are shown without the #$" prefix.
    let out = e.handle_event(SessionEvent::Line("#$\"#$#looks like mcp".into()));
    assert_eq!(main_lines(&out)[0].text, "#$#looks like mcp");
}

#[test]
fn mcp_can_be_turned_off() {
    let mut cfg = config(Rules::default(), Rules::default(), Rules::default());
    cfg.worlds[0].mcp = false;
    let (mut e, mut rx) = engine(&cfg);
    let out = e.handle_event(SessionEvent::Line("#$#mcp version: 2.1 to: 2.1".into()));
    assert_eq!(main_lines(&out)[0].text, "#$#mcp version: 2.1 to: 2.1");
    assert!(sent(&mut rx).is_empty());
}

#[test]
fn send_text_with_and_without_aliases() {
    let global = Rules { aliases: vec![Alias { matcher: pattern("k"), replacement: "kill $1".into(), ..Alias::default() }], ..Rules::default() };
    let (mut e, mut rx) = engine(&config(global, Rules::default(), Rules::default()));
    e.send_text("k orc", true);
    e.send_text("k orc\n/notacommand", false);
    assert_eq!(sent(&mut rx), vec!["kill orc", "k orc", "/notacommand"]);
}

#[test]
fn compile_errors_after_an_upload() {
    let (mut e, mut rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    // Before any upload, "Line 3:" is just text.
    let out = e.handle_event(SessionEvent::Line("Line 3:  syntax error".into()));
    assert!(!out.iter().any(|o| matches!(o, Output::CompileError { .. })));

    let out = e.submit("/edit", false);
    let Some(Output::Edit(request)) = out.first() else { panic!("{out:?}") };
    assert!(request.is_local());
    assert_eq!(request.language.as_deref(), Some("moo"));
    e.save_edit(request, "@program #1:test\nreturn 1\n.");
    assert_eq!(sent(&mut rx), vec!["@program #1:test", "return 1", "."]);

    let out = e.handle_event(SessionEvent::Line("Line 2:  parse error".into()));
    assert!(out.contains(&Output::CompileError { reference: String::new(), line: 2, message: "parse error".into() }));
    let out = e.handle_event(SessionEvent::Line("Error in line 7: Unrecognized word foo.".into()));
    assert!(out.iter().any(|o| matches!(o, Output::CompileError { line: 7, .. })));
    // It's still shown in the output too.
    assert_eq!(main_lines(&out).len(), 1);

    let out = e.submit("/edit MUF", false);
    assert!(matches!(&out[0], Output::Edit(r) if r.language.as_deref() == Some("muf")));
}

#[test]
fn moo_code_uses_the_world_language() {
    let mut cfg = config(Rules::default(), Rules::default(), Rules::default());
    cfg.worlds[0].program_language = "muf".into();
    let (mut e, mut rx) = engine(&cfg);
    e.handle_event(SessionEvent::Line("#$#mcp version: 2.1 to: 2.1".into()));
    let key = sent(&mut rx)[0].split_whitespace().nth(2).unwrap().to_string();
    let out = e.handle_event(SessionEvent::Line(format!(
        r##"#$#dns-org-mud-moo-simpleedit-content {key} reference: "#5:do" name: "#5:do" type: moo-code content: "return;""##
    )));
    assert!(matches!(&out[0], Output::Edit(r) if r.language.as_deref() == Some("muf")));
}

#[test]
fn room_info_builds_the_map_and_walking_follows_it() {
    let (mut e, mut rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    let out = e.handle_event(gmcp("Room.Info", r#"{"num": 1, "name": "Square", "area": "Town", "exits": {"n": 2}}"#));
    assert!(out.contains(&Output::MapChanged));
    let square = e.current_room().unwrap();
    e.submit("n", false);
    e.handle_event(gmcp("room.info", r#"{"num": 2, "name": "Gate", "area": "Town", "exits": {"s": 1}}"#));
    let gate = e.current_room().unwrap();
    assert_ne!(square, gate);
    assert_eq!(e.map().path(square, gate), Some(vec!["n".to_string()]));
    sent(&mut rx);

    e.map_walk_to(square);
    assert_eq!(sent(&mut rx), vec!["s"]);
    // With Room.Info, the position waits for the server to confirm.
    assert_eq!(e.current_room(), Some(gate));
}

#[test]
fn map_commands_and_tracking_by_name() {
    let (mut e, mut rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    e.submit("/map addroom Hall", false);
    e.submit(r#"/map addroom "Long Kitchen" north south"#, false);
    let kitchen = e.current_room().unwrap();
    assert_eq!(e.map().room(kitchen).unwrap().name, "Long Kitchen");
    e.submit("/map here hall", false);
    let hall = e.current_room().unwrap();
    assert_ne!(hall, kitchen);

    // No Room.Info: moving north and seeing the kitchen's name moves us there.
    e.submit("north", false);
    e.handle_event(SessionEvent::Line("Long Kitchen".into()));
    assert_eq!(e.current_room(), Some(kitchen));

    sent(&mut rx);
    e.submit("/map go hall", false);
    assert_eq!(sent(&mut rx), vec!["south"]);
    assert_eq!(e.current_room(), Some(hall), "without Room.Info, a walk is assumed to work");

    assert!(notices(&e.submit("/map go nowhere", false))[0].contains("No room"));
    assert!(notices(&e.submit("/map clear", false))[0].contains("/map clear yes"));
    e.submit("/map clear yes", false);
    assert!(e.map().is_empty());
    assert_eq!(e.submit("/map", false), vec![Output::ShowMap]);
}

#[test]
fn avatars_by_url_and_id() {
    let (mut e, mut rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    e.handle_event(gmcp("beip.line.image-url", r#""https://x.org/a.png""#));
    let out = e.handle_event(SessionEvent::Line("Bob says hi".into()));
    let i = out.iter().position(|o| matches!(o, Output::LineAvatar(a) if a.url.as_deref() == Some("https://x.org/a.png"))).unwrap();
    assert!(matches!(out[i + 1], Output::Line { .. }), "the avatar comes right before its line");
    // It applies to one line only.
    assert!(!e.handle_event(SessionEvent::Line("next".into())).iter().any(|o| matches!(o, Output::LineAvatar(_))));

    // An unknown id is requested once, and shown without a picture until the server answers.
    e.handle_event(gmcp("beip.line.id", r#""42""#));
    let out = e.handle_event(SessionEvent::Line("Alice waves".into()));
    assert!(out.iter().any(|o| matches!(o, Output::LineAvatar(a) if a.id.as_deref() == Some("42") && a.url.is_none())));
    e.handle_event(gmcp("beip.line.id", r#""42""#));
    let requests: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
        .filter(|c| matches!(c, SessionCommand::SendGmcp { package, .. } if package == "beip.id.request"))
        .collect();
    assert_eq!(requests.len(), 1);

    let out = e.handle_event(gmcp("beip.ids", r#"{"42": {"url": "https://x.org/alice.png", "hover-text": "Alice\nA cat"}}"#));
    assert!(out.iter().any(|o| matches!(o, Output::AvatarUpdated(a) if a.id.as_deref() == Some("42") && a.hover.as_deref() == Some("Alice\nA cat"))));
    e.handle_event(gmcp("beip.line.id", r#""42""#));
    let out = e.handle_event(SessionEvent::Line("Alice purrs".into()));
    assert!(out.iter().any(|o| matches!(o, Output::LineAvatar(a) if a.url.as_deref() == Some("https://x.org/alice.png"))));
}

#[test]
fn inline_images_are_opt_in() {
    let (mut e, _rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    let line = "Look: https://x.org/cat.JPG?size=2 and https://x.org/page.html";
    assert!(!e.handle_event(SessionEvent::Line(line.into())).iter().any(|o| matches!(o, Output::InlineImage { .. })));
    let out = e.submit("/images on", false);
    assert!(out.iter().any(|o| matches!(o, Output::SettingsChanged(s) if s.inline_images)));
    let out = e.handle_event(SessionEvent::Line(line.into()));
    let images: Vec<_> = out.iter().filter_map(|o| match o { Output::InlineImage { url } => Some(url.as_str()), _ => None }).collect();
    assert_eq!(images, vec!["https://x.org/cat.JPG?size=2"]);
}

fn with_puppets(puppets: Vec<Puppet>) -> (Engine, UnboundedReceiver<SessionCommand>) {
    let mut cfg = config(Rules::default(), Rules::default(), Rules::default());
    cfg.worlds[0].characters[0].puppets = puppets;
    engine(&cfg)
}

#[test]
fn puppet_lines_go_to_their_tab() {
    let zed = Puppet { name: "Zed".into(), receive_prefix: "Zed> ".into(), send_prefix: "z ".into(), ..Puppet::default() };
    let (mut e, mut rx) = with_puppets(vec![zed]);
    let out = e.handle_event(SessionEvent::Line("\x1b[32mzed> \x1b[0mA goblin is here.".into()));
    assert_eq!(out.iter().filter(|o| matches!(o, Output::OpenPuppet { name, focus: false } if name == "Zed")).count(), 1, "auto-opened once");
    let Some(Output::PuppetLine { line, .. }) = out.iter().find(|o| matches!(o, Output::PuppetLine { .. })) else { panic!("{out:?}") };
    assert_eq!(line.text, "A goblin is here.", "the prefix is hidden (matched case-insensitively)");
    assert!(main_lines(&out).is_empty());
    // Already open: no second OpenPuppet.
    let out = e.handle_event(SessionEvent::Line("Zed> More.".into()));
    assert!(!out.iter().any(|o| matches!(o, Output::OpenPuppet { .. })));
    // Typing in the puppet's tab sends with its prefix and echoes there.
    let out = e.puppet_submit("Zed", "look");
    assert_eq!(sent(&mut rx), vec!["z look"]);
    assert!(out.contains(&Output::PuppetEcho { name: "Zed".into(), text: "look".into() }));
}

#[test]
fn puppet_regex_closed_tabs_and_accidental_prefix() {
    let quiet = Puppet {
        name: "Quiet".into(),
        receive_prefix: r"^(\[Q\] )".into(),
        regex: true,
        send_prefix: "@force quiet=".into(),
        auto_open: false,
        remove_accidental_prefix: true,
        ..Puppet::default()
    };
    let (mut e, mut rx) = with_puppets(vec![quiet]);
    // Not open and not auto-opening: the line stays in the main window.
    let out = e.handle_event(SessionEvent::Line("[Q] hello".into()));
    assert_eq!(main_lines(&out)[0].text, "[Q] hello");
    let out = e.submit("/puppet quiet", false);
    assert!(out.contains(&Output::OpenPuppet { name: "Quiet".into(), focus: true }));
    let out = e.handle_event(SessionEvent::Line("[Q] hello".into()));
    assert!(out.iter().any(|o| matches!(o, Output::PuppetLine { line, .. } if line.text == "hello")), "the regex group is hidden");
    e.puppet_submit("Quiet", "@force quiet=sit");
    assert_eq!(sent(&mut rx), vec!["@force quiet=sit"], "the accidental prefix isn't doubled");
    e.set_puppet_open("Quiet", false);
    assert_eq!(main_lines(&e.handle_event(SessionEvent::Line("[Q] bye".into()))).len(), 1);
}

#[test]
fn puppets_can_open_on_connect() {
    let p = Puppet { name: "Bot".into(), receive_prefix: "Bot>".into(), open_on_connect: true, ..Puppet::default() };
    let (mut e, _rx) = with_puppets(vec![p]);
    let out = e.handle_event(SessionEvent::Connected { addr: "127.0.0.1:1".parse().unwrap() });
    assert!(out.contains(&Output::OpenPuppet { name: "Bot".into(), focus: false }));
}

#[test]
fn speech_from_triggers_and_settings() {
    let global = Rules {
        triggers: vec![Trigger { matcher: regex(r"(\w+) pages you"), speak: "Page from $1".into(), ..Trigger::default() }],
        ..Rules::default()
    };
    let (mut e, _rx) = engine(&config(global, Rules::default(), Rules::default()));
    let out = e.handle_event(SessionEvent::Line("Bob pages you: hi".into()));
    assert!(out.contains(&Output::Speak("Page from Bob".into())));
    assert!(!e.handle_event(SessionEvent::Line("quiet line".into())).iter().any(|o| matches!(o, Output::Speak(_))));

    let out = e.submit("/speech on", false);
    assert!(out.iter().any(|o| matches!(o, Output::SettingsChanged(s) if s.speak_output)));
    assert!(e.handle_event(SessionEvent::Line("now spoken".into())).contains(&Output::Speak("now spoken".into())));
    // A trigger's own speech replaces speaking the whole line, rather than both.
    let out = e.handle_event(SessionEvent::Line("Ann pages you: yo".into()));
    assert_eq!(out.iter().filter(|o| matches!(o, Output::Speak(_))).count(), 1);
    assert_eq!(e.submit("/speech stop", false), vec![Output::SpeechStop]);
    assert_eq!(e.submit("/speak hello there", false), vec![Output::Speak("hello there".into())]);
}

#[test]
fn substituting_aliases_edit_the_line_and_chain() {
    let global = Rules {
        aliases: vec![
            Alias { matcher: pattern("Mag"), replacement: "Magdalena".into(), substitute: true, ..Alias::default() },
            Alias { matcher: regex("“|”"), replacement: "\"".into(), substitute: true, ..Alias::default() },
            Alias { matcher: regex(r"^bt (\d+)$"), replacement: "buy torch $1".into(), ..Alias::default() },
        ],
        ..Rules::default()
    };
    let (e, _rx) = engine(&config(global, Rules::default(), Rules::default()));
    assert_eq!(e.expand_aliases("page Mag=“hi” mag"), ["page Magdalena=\"hi\" Magdalena"]);
    assert_eq!(e.expand_aliases("bt 3"), ["buy torch 3"], "whole-line aliases still apply after substitutions");
    assert_eq!(e.expand_aliases(""), [""]);
}

#[test]
fn trigger_replace_and_group_highlight() {
    let global = Rules {
        triggers: vec![
            Trigger { matcher: regex(r"\b(naughty|words)\b"), replace: Some("****".into()), ..Trigger::default() },
            Trigger {
                matcher: regex(r"\*+ (\w+)"),
                highlight: Some(Highlight { bold: true, groups: true, ..Highlight::default() }),
                ..Trigger::default()
            },
        ],
        ..Rules::default()
    };
    let (mut e, _rx) = engine(&config(global, Rules::default(), Rules::default()));
    let out = e.handle_event(SessionEvent::Line("a naughty sentence".into()));
    let line = main_lines(&out)[0];
    assert_eq!(line.text, "a **** sentence");
    let bold: Vec<&str> = line.spans.iter().filter(|s| s.style.bold).map(|s| line.span_text(s)).collect();
    assert_eq!(bold, ["sentence"], "the later trigger saw the replaced text and styled only its group");
}

#[test]
fn macros_can_fill_the_input() {
    let global = Rules {
        macros: vec![
            Macro { key: "F2".into(), command: "say ".into(), insert: true },
            Macro { key: "F3".into(), command: "look".into(), insert: false },
        ],
        ..Rules::default()
    };
    let (mut e, mut rx) = engine(&config(global, Rules::default(), Rules::default()));
    let out = e.run_macro(&KeyChord::parse("F2").unwrap()).unwrap();
    assert!(matches!(out.as_slice(), [Output::SetInput(t)] if t == "say "));
    e.run_macro(&KeyChord::parse("F3").unwrap()).unwrap();
    assert_eq!(sent(&mut rx), ["look"]);
    assert!(e.run_macro(&KeyChord::parse("F4").unwrap()).is_none());
}

#[test]
fn delayed_commands() {
    let (mut e, mut rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    e.submit("/delay 2s \"connect bob pw\"", false);
    e.submit("/delay id ping every 1s idle", false);
    let start = Instant::now();
    assert!(e.next_deadline().unwrap() <= start + std::time::Duration::from_secs(1));
    e.tick(start + std::time::Duration::from_millis(1500));
    assert_eq!(sent(&mut rx), ["idle"]);
    e.tick(start + std::time::Duration::from_millis(2500));
    assert_eq!(sent(&mut rx), ["connect bob pw", "idle"]);
    e.submit("/delay kill ping", false);
    assert!(e.next_deadline().is_none());
    assert_eq!(parse_duration("1m30s"), Some(std::time::Duration::from_secs(90)));
    assert_eq!(parse_duration("500ms"), Some(std::time::Duration::from_millis(500)));
    assert_eq!(parse_duration("soon"), None);
}

#[test]
fn login_runs_client_commands() {
    let mut cfg = config(Rules::default(), Rules::default(), Rules::default());
    cfg.worlds[0].characters[0].login = "/delay 1s connect bob pw\n//slash".into();
    let (mut e, mut rx) = engine(&cfg);
    e.handle_event(SessionEvent::Connected { addr: "127.0.0.1:4000".parse().unwrap() });
    assert_eq!(sent(&mut rx), ["/slash"]);
    e.tick(Instant::now() + std::time::Duration::from_secs(2));
    assert_eq!(sent(&mut rx), ["connect bob pw"]);
}

#[test]
fn aliases_can_expand_to_client_commands() {
    let global = Rules {
        aliases: vec![
            Alias { matcher: regex(r"^note (.*)$"), replacement: "/echo $1".into(), ..Alias::default() },
            Alias { matcher: regex(r"^slash$"), replacement: "//who".into(), ..Alias::default() },
        ],
        ..Rules::default()
    };
    let (mut e, mut rx) = engine(&config(global, Rules::default(), Rules::default()));
    let out = e.submit("note hello", false);
    assert_eq!(notices(&out), ["hello"]);
    e.submit("slash", false);
    assert_eq!(sent(&mut rx), ["/who"]);
}

#[test]
fn map_import_from_a_beipmu_atlas() {
    let (mut e, _rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../mu-import/tests/data/Fluff.atlas");
    let out = e.submit(&format!("/map import {}", path.display()), false);
    assert!(notices(&out)[0].starts_with("Imported 96 rooms"), "{:?}", notices(&out));
    assert!(out.iter().any(|o| matches!(o, Output::ShowMap)));
    assert_eq!(e.map().len(), 96);
    let out = e.submit("/map import /nonexistent.atlas", false);
    assert!(notices(&out)[0].starts_with("Couldn't import"));
}

#[test]
fn mcp_restarts_and_partial_lines_and_debug() {
    let (mut e, mut rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    // A partial #$# line is never shown as a prompt.
    assert!(e.handle_event(SessionEvent::PartialLine("#$#mcp version: 2.1 to: 2.1".into())).is_empty());
    let out = e.handle_event(SessionEvent::Line("#$#mcp version: 2.1 to: 2.1".into()));
    assert!(main_lines(&out).is_empty());
    let first = sent(&mut rx);
    assert!(first[0].starts_with("#$#mcp authentication-key: "), "{first:?}");
    assert!(notices(&e.submit("/mcp", false))[0].contains("hasn't replied"));
    // The MOO starts again (e.g. confunc run by hand): answer again with a new key.
    let out = e.submit("/mcp debug", false);
    assert_eq!(notices(&out), ["MCP traffic is shown"]);
    let out = e.handle_event(SessionEvent::Line("#$#mcp version: 2.1 to: 2.1".into()));
    let again = sent(&mut rx);
    assert!(again[0].starts_with("#$#mcp authentication-key: ") && again[0] != first[0]);
    let shown = notices(&out);
    assert!(shown[0].starts_with("MCP ← #$#mcp version") && shown[1].starts_with("MCP → #$#mcp authentication-key"), "{shown:?}");
    let key = again[0].split_whitespace().nth(2).unwrap().to_string();
    e.handle_event(SessionEvent::Line(format!("#$#mcp-negotiate-can {key} package: mcp-negotiate min-version: 2.0 max-version: 2.0")));
    assert_eq!(notices(&e.submit("/mcp", false)), ["MCP 2.1 is active"]);
}

#[test]
fn mcp_lines_wrapped_in_colour_codes() {
    // Some cores' :tell wraps every line in ANSI codes.
    let (mut e, mut rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    e.handle_event(SessionEvent::Line("[lognet] someone has reconnected.".into()));
    let out = e.handle_event(SessionEvent::Line("\x1b[0m#$#mcp version: 2.1 to: 2.1\x1b[0m".into()));
    assert!(main_lines(&out).is_empty(), "{out:?}");
    let key = sent(&mut rx)[0].split_whitespace().nth(2).unwrap().to_string();
    e.handle_event(SessionEvent::Line(format!("\x1b[0m#$#mcp-negotiate-can {key} package: \"mcp-negotiate\" min-version: \"2.0\" max-version: \"2.0\"\x1b[0m")));
    assert_eq!(notices(&e.submit("/mcp", false)), ["MCP 2.1 is active"]);
    // Ordinary coloured text is untouched.
    let out = e.handle_event(SessionEvent::Line("\x1b[31mred\x1b[0m".into()));
    assert_eq!(main_lines(&out)[0].text, "red");
}
