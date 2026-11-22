//! The engine with a real JavaScript host: hooks, actions, triggers and timers end to end.

use std::time::{Duration, Instant};

use mu_config::{Alias, Character, Config, Pattern, Rules, Trigger, World};
use mu_engine::{Engine, Output, Target};
use mu_net::{SessionCommand, SessionEvent, SessionHandle};
use mu_script::JsHost;
use tokio::sync::mpsc::UnboundedReceiver;

fn setup(rules: Rules) -> (Engine, UnboundedReceiver<SessionCommand>) {
    let config = Config {
        rules,
        worlds: vec![World {
            name: "W".into(),
            host: "localhost".into(),
            characters: vec![Character { name: "C".into(), ..Character::default() }],
            ..World::default()
        }],
        ..Config::default()
    };
    let world = config.worlds[0].clone();
    let (mut engine, _) = Engine::new(&config, world.clone(), Some(world.characters[0].clone()));
    engine.detach_map();
    let (handle, rx) = SessionHandle::channel();
    engine.attach(handle);
    let out = engine.set_script_host(Box::new(JsHost::new().unwrap()));
    assert!(out.is_empty(), "{out:?}");
    (engine, rx)
}

fn sent(rx: &mut UnboundedReceiver<SessionCommand>) -> Vec<String> {
    std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|c| match c {
            SessionCommand::SendLine(l) => Some(l),
            _ => None,
        })
        .collect()
}

fn notices(out: &[Output]) -> Vec<String> {
    out.iter()
        .filter_map(|o| match o {
            Output::Notice(n) => Some(n.clone()),
            _ => None,
        })
        .collect()
}

fn main_text(out: &[Output]) -> Vec<String> {
    out.iter()
        .filter_map(|o| match o {
            Output::Line { line, target: Target::Main } => Some(line.text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn immediate_mode_and_multiline_functions() {
    let (mut e, mut rx) = setup(Rules::default());
    assert_eq!(notices(&e.submit("/@ 6 * 7", false)), vec!["42"]);
    e.submit("/@function greet(who) {\n  mu.send('wave ' + who)\n}", false);
    e.submit("/@greet('Bob')", false);
    assert_eq!(sent(&mut rx), vec!["wave Bob"]);
    let out = e.submit("/@nope()", false);
    assert!(notices(&out)[0].starts_with("Script error: ReferenceError"), "{out:?}");
}

#[test]
fn line_hook_hides_and_trigger_calls_script() {
    let rules = Rules {
        triggers: vec![Trigger {
            matcher: Pattern { pattern: r"^(\w+) tells you '(.*)'$".into(), regex: true, ..Pattern::default() },
            script: "onTell".into(),
            ..Trigger::default()
        }],
        ..Rules::default()
    };
    let (mut e, mut rx) = setup(rules);
    e.submit(
        "/@mu.on('line', t => !t.includes('[ad]'));\nfunction onTell(line, who, msg) { mu.spawn('Tells', who + ': ' + msg); if (msg === 'ping') mu.send('tell ' + who + ' pong') }",
        false,
    );
    let out = e.handle_event(SessionEvent::Line("[ad] buy gold".into()));
    assert!(main_text(&out).is_empty());
    let out = e.handle_event(SessionEvent::Line("Bob tells you 'ping'".into()));
    assert_eq!(main_text(&out), vec!["Bob tells you 'ping'"]);
    assert!(out.iter().any(|o| matches!(o, Output::Line { target: Target::Spawn(w), line } if w == "Tells" && line.text == "Bob: ping")));
    assert_eq!(sent(&mut rx), vec!["tell Bob pong"]);
}

#[test]
fn send_hook_and_input() {
    let rules = Rules {
        aliases: vec![Alias { matcher: Pattern { pattern: "k".into(), ..Pattern::default() }, replacement: "kill $1".into(), ..Alias::default() }],
        ..Rules::default()
    };
    let (mut e, mut rx) = setup(rules);
    e.submit("/@mu.on('send', c => c.startsWith('kill') ? c.toUpperCase() : c === 'oops' ? false : undefined)", false);
    e.submit("k orc", false);
    e.submit("oops", false);
    e.submit("look", false);
    e.submit("/@mu.input('k rat'); mu.input('/set target rat')", false);
    assert_eq!(sent(&mut rx), vec!["KILL ORC", "look", "KILL RAT"]);
    assert_eq!(e.variable("target").as_deref(), Some("rat"));
}

#[test]
fn script_loops_are_cut_off() {
    let (mut e, mut rx) = setup(Rules::default());
    // Every command re-enters the input from the send hook: an infinite loop.
    e.submit("/@mu.on('send', c => { mu.input(c + '!'); return false })", false);
    let out = e.submit("go", false);
    assert!(notices(&out).iter().any(|n| n.contains("nested too deeply")), "{out:?}");
    assert!(sent(&mut rx).is_empty());
}

#[test]
fn variables_events_and_gmcp() {
    let (mut e, _rx) = setup(Rules::default());
    let out = e.submit("/@mu.vars.mood = 'happy'", false);
    assert!(out.iter().any(|o| matches!(o, Output::VariablesChanged { variables, .. } if variables["mood"] == "happy")));
    assert_eq!(e.variable("mood").as_deref(), Some("happy"));
    assert_eq!(notices(&e.submit("/@mu.vars.mood", false)), vec!["happy"]);

    e.submit("/@mu.on('connect', () => mu.notice('hello ' + mu.world + ' ' + mu.connected)); mu.on('gmcp:room.info', r => mu.notice('in ' + r.name))", false);
    let out = e.handle_event(SessionEvent::Connected { addr: "127.0.0.1:1".parse().unwrap() });
    assert!(notices(&out).contains(&"hello W true".to_string()), "{out:?}");
    let out = e.handle_event(SessionEvent::Gmcp { package: "Room.Info".into(), data: r#"{"name":"Town Square"}"#.into() });
    assert!(notices(&out).contains(&"in Town Square".to_string()));
}

#[test]
fn timers_through_the_engine() {
    let (mut e, mut rx) = setup(Rules::default());
    assert_eq!(e.next_deadline(), None);
    e.submit("/@setTimeout(() => mu.send('later'), 30)", false);
    let due = e.next_deadline().expect("timer scheduled");
    // Not due yet.
    e.tick(Instant::now());
    assert!(sent(&mut rx).is_empty());
    e.tick(due + Duration::from_millis(1));
    assert_eq!(sent(&mut rx), vec!["later"]);
    assert_eq!(e.next_deadline(), None);
}
