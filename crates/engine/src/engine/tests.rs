use familiar_config::{Alias, Character, Config, Highlight, Macro, Pattern, Puppet, Rules, Scope, Trigger, World};
use familiar_net::{SessionCommand, SessionEvent, SessionHandle};
use tokio::sync::mpsc::UnboundedReceiver;

use super::scripting::parse_duration;
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
fn autolog_starts_on_connect_unless_already_logging() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(Rules::default(), Rules::default(), Rules::default());
    cfg.worlds[0].characters[0].log_file = dir.path().join("C.log").to_string_lossy().into_owned();
    let (mut e, _rx) = engine(&cfg);
    assert!(!e.is_logging(), "not before connecting");
    let out = e.handle_event(SessionEvent::Connected { addr: "127.0.0.1:1".parse().unwrap() });
    assert!(out.contains(&Output::LoggingChanged(Some(dir.path().join("C.log")))), "{out:?}");
    e.handle_event(SessionEvent::Line("\x1b[1mhello\x1b[0m".into()));
    // Reconnecting keeps the same log going.
    let out = e.handle_event(SessionEvent::Connected { addr: "127.0.0.1:1".parse().unwrap() });
    assert!(!out.iter().any(|o| matches!(o, Output::LoggingChanged(_))));
    let text = std::fs::read_to_string(dir.path().join("C.log")).unwrap();
    assert!(text.lines().nth(1).is_some_and(|l| l.ends_with("] hello")), "{text}");
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
fn bouncer_sends_the_login_itself() {
    let mut cfg = config(Rules::default(), Rules::default(), Rules::default());
    cfg.bouncer.host = "bnc.example".into();
    cfg.worlds[0].bouncer = true;
    cfg.worlds[0].characters[0].login = "/delay 1s connect bob pw\n/echo hello".into();
    let (mut e, mut rx) = engine(&cfg);
    e.via_bouncer = true;
    let out = e.handle_event(SessionEvent::Connected { addr: "127.0.0.1:4000".parse().unwrap() });
    // The client command the bouncer can't run still runs here; the login line doesn't.
    assert!(out.contains(&Output::Notice("hello".into())), "{out:?}");
    e.tick(Instant::now() + std::time::Duration::from_secs(2));
    assert!(sent(&mut rx).is_empty());

    e.submit("/bnc close", false);
    assert!(matches!(rx.try_recv(), Ok(SessionCommand::Bouncer(familiar_net::BouncerRequest::Close))));
}

#[test]
fn bnc_without_a_bouncer_explains() {
    let (mut e, _rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    let out = e.submit("/bnc", false);
    assert!(matches!(&out[..], [Output::Notice(n)] if n.contains("doesn't use the bouncer")), "{out:?}");
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
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../import/tests/data/Fluff.atlas");
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

#[test]
fn simpleedit_through_a_core_that_colours_every_line() {
    // What a real core's :tell sent: each line ends with " ESC[00m".
    let (mut e, mut rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    e.handle_event(SessionEvent::Line("#$#mcp version: 2.1 to: 2.1".into()));
    let key = sent(&mut rx)[0].split_whitespace().nth(2).unwrap().to_string();
    let tell = |t: String| SessionEvent::Line(format!("{t} \x1b[00m"));
    let mut out = Vec::new();
    for line in [
        format!("#$#dns-org-mud-moo-simpleedit-content {key} name: \"#1:x\" reference: \"#1:x\" type: \"moo-code\" content*: \"\" _data-tag: 9301"),
        "#$#* 9301 content: noParents = {};".into(),
        "#$#* 9301 content:   return 1;".into(),
        "#$#: 9301".into(),
    ] {
        out.extend(e.handle_event(tell(line)));
    }
    let edit = out.iter().find_map(|o| if let Output::Edit(r) = o { Some(r.clone()) } else { None }).expect("an editor opens");
    assert_eq!(edit.content, "noParents = {};\n  return 1;");
    assert!(main_lines(&out).is_empty());
    // Compile errors in coloured lines are still found.
    e.save_edit(&edit, "noParents = {}");
    let out = e.handle_event(tell("\x1b[1mLine 1:  syntax error".into()));
    assert!(out.iter().any(|o| matches!(o, Output::CompileError { line: 1, message, .. } if message == "syntax error")), "{out:?}");
    // A broken message with our key is reported rather than dropped silently.
    let out = e.handle_event(SessionEvent::Line(format!("#$#dns-com-awns-status {key} text: \"unterminated")));
    assert!(notices(&out)[0].starts_with("Ignored an MCP message"), "{out:?}");
}

/// An engine that has negotiated MCP with a server offering dns-com-familiar-agent and two
/// tools. Returns it with the auth key.
fn agent_engine() -> (Engine, UnboundedReceiver<SessionCommand>, String) {
    let (mut e, mut rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    e.set_agents_supported(true);
    assert_eq!(e.agent_tools(), None);
    e.handle_event(SessionEvent::Line("#$#mcp version: 2.1 to: 2.1".into()));
    let key = sent(&mut rx)[0].split_whitespace().nth(2).unwrap().to_string();
    e.handle_event(SessionEvent::Line(format!("#$#mcp-negotiate-can {key} package: dns-com-familiar-agent min-version: 1.0 max-version: 1.0")));
    e.handle_event(SessionEvent::Line(format!("#$#mcp-negotiate-end {key}")));
    // The client asks for the tools.
    assert_eq!(sent(&mut rx), vec![format!("#$#dns-com-familiar-agent-list {key}")]);
    assert_eq!(e.agent_tools(), Some(&[][..]));
    let lines = [
        format!(r#"#$#dns-com-familiar-agent-tools {key} tools*: "" _data-tag: t1"#),
        r#"#$#* t1 tools: {"name":"list_verb","description":"Shows a verb","input_schema":{"type":"object","properties":{"obj":{"type":"string"}}},"show":"full"}"#.into(),
        r#"#$#* t1 tools: {"name":"program_verb","timeout":5}"#.into(),
        "#$#* t1 tools: not json".into(),
        "#$#: t1".into(),
    ];
    let mut out = Vec::new();
    for line in lines {
        out.extend(e.handle_event(SessionEvent::Line(line)));
    }
    assert!(main_lines(&out).is_empty());
    assert_eq!(notices(&out).len(), 1, "the bad line is reported: {out:?}");
    let Some(Output::AgentTools(tools)) = out.iter().find(|o| matches!(o, Output::AgentTools(_))) else { panic!("{out:?}") };
    assert_eq!(tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["list_verb", "program_verb"]);
    assert_eq!(e.agent_tools().unwrap().len(), 2);
    (e, rx, key)
}

#[test]
fn agent_tools_calls_and_results() {
    let (mut e, mut rx, key) = agent_engine();
    let tools = e.agent_tools().unwrap();
    assert_eq!(tools[1].timeout, 5);
    assert_eq!(tools[1].input_schema["type"], "object", "a schema is always an object");

    assert!(e.agent_call("nope", &serde_json::json!({})).unwrap_err().contains("no tool called nope"));
    let id = e.agent_call("list_verb", &serde_json::json!({"obj": "#1", "verb": "look"})).unwrap();
    let lines = sent(&mut rx);
    assert!(lines[0].starts_with(&format!("#$#dns-com-familiar-agent-call {key} id: \"{id}\" tool: \"list_verb\" args*: \"\" _data-tag: ")), "{lines:?}");
    let tag = lines[0].rsplit(' ').next().unwrap();
    assert_eq!(&lines[1..], [format!(r##"#$#* {tag} args: {{"obj":"#1","verb":"look"}}"##), format!("#$#: {tag}")]);

    // A result with someone else's key is ignored.
    let out = e.handle_event(SessionEvent::Line(format!("#$#dns-com-familiar-agent-result badkey id: {id} ok: 1 content: spoofed")));
    assert!(out.iter().all(|o| !matches!(o, Output::AgentResult { .. })));

    let lines = [
        format!(r#"#$#dns-com-familiar-agent-result {key} id: {id} ok: 1 content*: "" _data-tag: r1"#),
        "#$#* r1 content: return this.name;".into(),
        "#$#* r1 content: ".into(),
        "#$#: r1".into(),
    ];
    let mut out = Vec::new();
    for line in lines {
        out.extend(e.handle_event(SessionEvent::Line(line)));
    }
    assert!(out.contains(&Output::AgentResult { id, ok: true, content: "return this.name;\n".into() }), "{out:?}");
    // show = full: the summary and what came back.
    assert_eq!(notices(&out), ["[agent] list_verb obj=#1 verb=look → ok", "[agent]   return this.name;"]);
    // A second result for the same call is dropped.
    let out = e.handle_event(SessionEvent::Line(format!("#$#dns-com-familiar-agent-result {key} id: {id} ok: 1 content: again")));
    assert!(out.is_empty(), "{out:?}");
}

#[test]
fn long_agent_arguments_are_sent_in_pieces() {
    let (mut e, mut rx, _) = agent_engine();
    let code: String = (0..300).map(|i| format!("x = {i}; \"é\";\n")).collect();
    let args = serde_json::json!({"obj": "#1", "verb": "v", "code": code});
    e.agent_call("program_verb", &args).unwrap();
    let lines = sent(&mut rx);
    let tag = lines[0].rsplit(' ').next().unwrap().to_string();
    let prefix = format!("#$#* {tag} args: ");
    let pieces: Vec<&str> = lines[1..lines.len() - 1].iter().map(|l| l.strip_prefix(&prefix).unwrap()).collect();
    assert!(pieces.len() > 3);
    assert!(pieces.iter().all(|p| p.len() <= 1000 && !p.ends_with(' ')));
    let joined: serde_json::Value = serde_json::from_str(&pieces.concat()).unwrap();
    assert_eq!(joined, args);
}

#[test]
fn agent_calls_fail_on_timeout_and_disconnect() {
    let (mut e, _rx, key) = agent_engine();
    let slow = e.agent_call("program_verb", &serde_json::json!({"code": "a\nb"})).unwrap();
    let deadline = e.next_deadline().unwrap();
    assert!(deadline <= Instant::now() + Duration::from_secs(5));
    let out = e.tick(deadline);
    let Some(Output::AgentResult { id, ok: false, content }) = out.iter().find(|o| matches!(o, Output::AgentResult { .. })) else { panic!("{out:?}") };
    assert_eq!(*id, slow);
    assert!(content.contains("5 seconds"));
    assert_eq!(notices(&out), ["[agent] program_verb code=(2 lines) → failed: The world didn't answer within 5 seconds"]);
    // Its late result is dropped.
    let out = e.handle_event(SessionEvent::Line(format!("#$#dns-com-familiar-agent-result {key} id: {slow} ok: 1 content: late")));
    assert!(out.is_empty());

    let pending = e.agent_call("list_verb", &serde_json::json!({})).unwrap();
    let out = e.handle_event(SessionEvent::Disconnected { error: None });
    assert!(out.iter().any(|o| matches!(o, Output::AgentResult { id, ok: false, .. } if *id == pending)));
    assert!(out.contains(&Output::AgentTools(Vec::new())));
    assert!(e.agent_call("list_verb", &serde_json::json!({})).is_err());
}

#[test]
fn agents_send_commands_and_hear_the_answer() {
    let (mut e, mut rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    e.set_agents_supported(true);
    e.handle_event(SessionEvent::Line("Before".into()));
    e.handle_event(SessionEvent::PartialLine("> ".into()));
    let mark = e.agent_send(&["look".into()]).unwrap();
    assert_eq!(sent(&mut rx), ["look"]);
    // The prompt from before isn't part of the answer.
    assert_eq!(e.agent_heard(mark), AgentHeard { next: 1, ..AgentHeard::default() });
    assert_eq!(e.agent_heard(0).lines, ["Before"]);
    e.handle_event(SessionEvent::Line("A room".into()));
    e.handle_event(SessionEvent::Line("#$#mcp version: 2.1 to: 2.1".into()));
    e.handle_event(SessionEvent::PartialLine("Password: ".into()));
    let heard = e.agent_heard(mark);
    assert_eq!((heard.lines, heard.partial), (vec!["A room".to_string()], Some("Password: ".to_string())));
    e.handle_event(SessionEvent::Line("Password: ok".into()));
    let heard = e.agent_heard(mark);
    assert_eq!((heard.lines.as_slice(), heard.partial.as_deref(), heard.next), (&["A room".to_string(), "Password: ok".to_string()][..], None, 3));
    assert_eq!(heard.since(2).collect::<Vec<_>>(), ["Password: ok"]);
    assert_eq!(e.agent_heard(u64::MAX), AgentHeard { next: 3, ..AgentHeard::default() });

    // What was sent shows at the next tick, which is due now.
    let due = e.next_deadline().unwrap();
    assert!(due <= Instant::now());
    let out = e.tick(Instant::now());
    assert_eq!(notices(&out), ["[agent] → look"]);
    assert!(out.contains(&Output::Sent("look".into())));
    assert!(e.next_deadline().is_none());
    let lines: Vec<String> = (1..=5).map(|i| format!("line {i}")).collect();
    e.agent_send(&lines).unwrap();
    assert_eq!(notices(&e.tick(Instant::now())), ["[agent] → line 1 (and 4 more lines)"]);

    // Old lines are dropped, and counted.
    let mark = e.agent_send(&[]).unwrap();
    for i in 0..2100 {
        e.handle_event(SessionEvent::Line(format!("spam {i}")));
    }
    let heard = e.agent_heard(mark);
    assert_eq!((heard.lines.len(), heard.dropped, heard.next), (2000, 100, 2103));
    assert_eq!(heard.lines[0], "spam 100");

    e.handle_event(SessionEvent::Disconnected { error: None });
    assert!(e.agent_send(&["look".into()]).is_err());
}

#[test]
fn agents_only_where_the_frontend_runs_them() {
    let (mut e, mut rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    e.handle_event(SessionEvent::Line("#$#mcp version: 2.1 to: 2.1".into()));
    let offered = sent(&mut rx);
    assert!(!offered.iter().any(|l| l.contains("dns-com-familiar-agent")), "{offered:?}");
    let key = offered[0].split_whitespace().nth(2).unwrap().to_string();
    e.handle_event(SessionEvent::Line(format!("#$#mcp-negotiate-can {key} package: dns-com-familiar-agent min-version: 1.0 max-version: 1.0")));
    e.handle_event(SessionEvent::Line(format!("#$#mcp-negotiate-end {key}")));
    assert!(sent(&mut rx).is_empty());
    assert_eq!(e.agent_tools(), None);
    let out = e.submit("/agent claude", false);
    assert!(matches!(&out[..], [Output::Notice(n)] if n.starts_with("Unknown command /agent")), "{out:?}");
    assert!(!matches!(&e.submit("/help", false)[..], [Output::Notice(n)] if n.contains("/agent")));

    e.set_agents_supported(true);
    assert_eq!(e.submit("/agent  codex ", false), vec![Output::Agent("codex".into())]);
    assert!(matches!(&e.submit("/help", false)[..], [Output::Notice(n)] if n.contains("/agent")));
}

#[test]
fn delay_bounds_and_secret_login_delays() {
    assert_eq!(parse_duration("infs"), None);
    assert_eq!(parse_duration("1e20s"), None);
    assert_eq!(parse_duration("NaNs"), None);
    let mut cfg = config(
        Rules { aliases: vec![Alias { matcher: pattern("connect"), replacement: "hijacked".into(), ..Alias::default() }], ..Rules::default() },
        Rules::default(),
        Rules::default(),
    );
    cfg.worlds[0].characters[0].login = "/delay 1s connect bob hunter2".into();
    let (mut e, mut rx) = engine(&cfg);
    e.handle_event(SessionEvent::Connected { addr: "127.0.0.1:4000".parse().unwrap() });
    // Secret login delays are hidden from /delay list and bypass aliases/echo when firing.
    assert!(notices(&e.submit("/delay list", false)).iter().all(|n| !n.contains("hunter2")));
    let out = e.tick(Instant::now() + Duration::from_secs(2));
    assert!(!out.iter().any(|o| matches!(o, Output::Echo(_))));
    assert_eq!(sent(&mut rx), ["connect bob hunter2"]);
}

#[test]
fn follow_link_sanitizes_crlf_and_blocks_unsafe_schemes() {
    let (mut e, mut rx) = engine(&config(Rules::default(), Rules::default(), Rules::default()));
    e.follow_link("send:look%0d%0akill%20guard");
    assert_eq!(sent(&mut rx), ["lookkill guard"]);
    assert!(notices(&e.follow_link("file:///etc/passwd"))[0].contains("Refused"));
    assert!(notices(&e.follow_link("javascript:alert(1)"))[0].contains("Refused"));
    assert_eq!(e.follow_link("mailto:test@example.org"), vec![Output::OpenUri("mailto:test@example.org".into())]);
}

#[test]
fn expand_unknown_var_preserves_following_var() {
    let caps = crate::expand::Captures::default();
    let out = crate::expand::expand("%unknown%%target%", &caps, |name| (name == "target").then(|| "orc".to_string()));
    assert_eq!(out, "%unknown%orc");
}

#[test]
fn ansi_colored_room_name_tracks_map_and_reconnect_unsuspends_triggers() {
    let global = Rules { triggers: vec![Trigger { matcher: pattern("echo"), send: "echo".into(), ..Trigger::default() }], ..Rules::default() };
    let (mut e, mut rx) = engine(&config(global, Rules::default(), Rules::default()));
    e.submit("/map addroom Hall", false);
    e.submit("/map addroom Kitchen n s", false);
    let kitchen = e.current_room().unwrap();
    e.submit("/map here hall", false);
    e.submit("n", false);
    e.handle_event(SessionEvent::Line("\x1b[1;33mKitchen\x1b[0m".into()));
    assert_eq!(e.current_room(), Some(kitchen));

    // Suspend the trigger via runaway sends, then verify reconnect unsuspends it.
    sent(&mut rx);
    for _ in 0..30 {
        e.handle_event(SessionEvent::Line("echo".into()));
    }
    assert_eq!(sent(&mut rx).len(), MAX_SENDS);
    e.handle_event(SessionEvent::Line("echo".into()));
    assert!(sent(&mut rx).is_empty());
    e.handle_event(SessionEvent::Disconnected { error: None });
    let (handle, mut rx) = SessionHandle::channel();
    e.attach(handle);
    e.handle_event(SessionEvent::Connected { addr: "127.0.0.1:4000".parse().unwrap() });
    e.handle_event(SessionEvent::Line("echo".into()));
    assert_eq!(sent(&mut rx), ["echo"]);
}

#[test]
fn reload_updates_world_and_stats_fraction_handles_extreme_i64() {
    let mut cfg = config(Rules::default(), Rules::default(), Rules::default());
    let (mut e, _rx) = engine(&cfg);
    cfg.worlds[0].program_language = "muf".into();
    e.reload(&cfg);
    assert_eq!(e.world().program_language, "muf");

    let extreme = crate::stats::StatValue::Range { value: i64::MAX, min: i64::MIN, max: i64::MAX, colors: Default::default() };
    assert!((extreme.fraction().unwrap() - 1.0).abs() < 1e-6);

    let big_timeout = crate::mcp::AgentTool::parse(r#"{"name":"t","timeout":18446744073709551615}"#).unwrap();
    assert_eq!(big_timeout.timeout, 3600);
}

#[test]
fn restore_log_dedup_keeps_latest_and_strips_spoofed_markers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("restore.log");
    std::fs::write(&path, "\u{1}2026-01-01 00:00:00\n\u{1}2026-01-02 00:00:00\n").unwrap();
    let mut log = crate::restore::RestoreLog::open(&path).unwrap();
    log.line("\u{1}\u{1}spoofed").unwrap();
    drop(log);
    let entries = crate::restore::RestoreLog::read(&path).unwrap();
    assert_eq!(entries.len(), 2);
    assert!(matches!(entries[0], crate::restore::Entry::Start(dt) if dt.to_string() != "2026-01-01 00:00:00"));
    assert_eq!(entries[1], crate::restore::Entry::Line("spoofed".into()));
}
