use mu_config::{Alias, Character, Config, Highlight, Macro, Pattern, Rules, Scope, Trigger, World};
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
    let global = Rules { macros: vec![Macro { key: "KP_8".into(), command: "north".into() }], ..Rules::default() };
    let (mut e, mut rx) = engine(&config(global, Rules::default(), Rules::default()));
    assert_eq!(e.macro_for(&KeyChord::new("kp_8", false, false, false)).as_deref(), Some("north"));
    assert_eq!(e.macro_for(&KeyChord::new("KP_8", true, false, false)), None);

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
