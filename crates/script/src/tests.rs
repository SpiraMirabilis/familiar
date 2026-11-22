use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use familiar_engine::script::{ScriptAction, ScriptContext, ScriptHost};

use super::JsHost;

fn host() -> JsHost {
    JsHost::new().expect("QuickJS starts")
}

fn errors(actions: &[ScriptAction]) -> Vec<&str> {
    actions
        .iter()
        .filter_map(|a| match a {
            ScriptAction::Error(e) => Some(e.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn eval_results_and_errors() {
    let mut js = host();
    assert_eq!(js.eval("1 + 2").unwrap().as_deref(), Some("3"));
    assert_eq!(js.eval("({a: [1, 'x']})").unwrap().as_deref(), Some(r#"{"a":[1,"x"]}"#));
    assert_eq!(js.eval("'text'").unwrap().as_deref(), Some("text"));
    assert_eq!(js.eval("var x = 5").unwrap(), None);
    // Definitions persist between evals, like BeipMU's /@function.
    js.eval("function double(n) { return n * 2 }").unwrap();
    assert_eq!(js.eval("double(x)").unwrap().as_deref(), Some("10"));
    let err = js.eval("undefinedThing()").unwrap_err();
    assert!(err.contains("undefinedThing"), "{err}");
    let syntax = js.eval("function (").unwrap_err();
    assert!(syntax.starts_with("SyntaxError: "), "{syntax}");
    assert!(err.starts_with("ReferenceError: "), "{err}");
}

#[test]
fn api_calls_become_actions() {
    let mut js = host();
    js.eval(r#"mu.send("look"); mu.input("k orc"); mu.echo("hi", 1, {a: 1}); mu.notice("note"); mu.spawn("Chat", "msg"); mu.gmcp("Char.Skills.Get", {group: "x"}); mu.gmcp("Core.Ping"); console.log("logged")"#)
        .unwrap();
    assert_eq!(
        js.take_actions(),
        vec![
            ScriptAction::Send("look".into()),
            ScriptAction::Input("k orc".into()),
            ScriptAction::Display(r#"hi 1 {"a":1}"#.into()),
            ScriptAction::Notice("note".into()),
            ScriptAction::Spawn { window: "Chat".into(), text: "msg".into() },
            ScriptAction::Gmcp { package: "Char.Skills.Get".into(), data: Some(r#"{"group":"x"}"#.into()) },
            ScriptAction::Gmcp { package: "Core.Ping".into(), data: None },
            ScriptAction::Notice("logged".into()),
        ]
    );
    assert!(js.take_actions().is_empty());
}

#[test]
fn line_and_send_hooks() {
    let mut js = host();
    js.eval(
        r#"
        mu.on("line", text => !text.startsWith("[spam]"));
        mu.on("send", cmd => cmd === "secret" ? false : cmd === "n" ? "north" : undefined);
        mu.on("receive", text => { if (text.includes("bell")) mu.notice("ding") });
    "#,
    )
    .unwrap();
    assert!(js.on_line("hello"));
    assert!(!js.on_line("[spam] buy now"));
    assert_eq!(js.on_send("n").as_deref(), Some("north"));
    assert_eq!(js.on_send("look").as_deref(), Some("look"));
    assert_eq!(js.on_send("secret"), None);
    js.on_receive("the bell rings");
    assert_eq!(js.take_actions(), vec![ScriptAction::Notice("ding".into())]);
}

#[test]
fn handler_errors_are_reported_and_others_still_run() {
    let mut js = host();
    js.eval(r#"mu.on("LINE", t => { throw new Error("boom") }); mu.on("line", t => false);"#).unwrap();
    assert!(!js.on_line("x"), "the second handler still ran");
    let actions = js.take_actions();
    let errs = errors(&actions);
    assert_eq!(errs.len(), 1);
    assert!(errs[0].contains("boom"), "{errs:?}");
    // A broken send hook passes the command through rather than eating it.
    js.eval(r#"mu.on("send", c => { throw "nope" })"#).unwrap();
    assert_eq!(js.on_send("look").as_deref(), Some("look"));
}

#[test]
fn timers() {
    let mut js = host();
    assert_eq!(js.next_deadline(), None);
    js.eval(
        r#"
        let ticks = 0;
        setTimeout(() => mu.notice("once"), 50);
        const id = setInterval(() => { ticks++; if (ticks === 3) clearInterval(id) }, 20);
    "#,
    )
    .unwrap();
    let first = js.next_deadline().expect("a timer is pending");
    assert!(first > Instant::now() + Duration::from_millis(5));
    // Run the clock forward by simulated time.
    let start = Instant::now();
    for step in 1..=20 {
        js.tick(start + Duration::from_millis(step * 10));
    }
    assert_eq!(js.take_actions(), vec![ScriptAction::Notice("once".into())]);
    assert_eq!(js.eval("ticks").unwrap().as_deref(), Some("3"));
    assert_eq!(js.next_deadline(), None);
}

#[test]
fn variables_and_context() {
    let mut js = host();
    js.set_variables(&BTreeMap::from([("target".to_string(), "orc".to_string())]));
    js.set_context(&ScriptContext { world: "Aardwolf".into(), character: Some("Bob".into()), connected: true, ..Default::default() });
    assert_eq!(js.eval("mu.vars.target + ' ' + mu.world + ' ' + mu.character + ' ' + mu.connected").unwrap().as_deref(), Some("orc Aardwolf Bob true"));
    js.eval("mu.vars.hp = 50; delete mu.vars.target").unwrap();
    assert_eq!(
        js.take_actions(),
        vec![
            ScriptAction::SetVariable { name: "hp".into(), value: Some("50".into()) },
            ScriptAction::SetVariable { name: "target".into(), value: None },
        ]
    );
    assert_eq!(js.eval("Object.keys(mu.vars)").unwrap().as_deref(), Some(r#"["hp"]"#));
}

#[test]
fn gmcp_events_and_trigger_calls() {
    let mut js = host();
    js.eval(
        r#"
        mu.on("gmcp:Char.Vitals", v => mu.notice("hp " + v.hp));
        mu.on("gmcp", (pkg, data) => mu.notice("any " + pkg));
        function onAttack(line, who) { mu.send("kill " + who) }
    "#,
    )
    .unwrap();
    js.on_gmcp("char.vitals", r#"{"hp": 7}"#);
    js.call("onAttack", &["The orc attacks you".into(), "orc".into()]);
    js.call("missing", &[]);
    let actions = js.take_actions();
    assert_eq!(actions[0], ScriptAction::Notice("any char.vitals".into()));
    assert_eq!(actions[1], ScriptAction::Notice("hp 7".into()));
    assert_eq!(actions[2], ScriptAction::Send("kill orc".into()));
    assert!(errors(&actions)[0].contains("isn't a function"));
}

#[test]
fn promises_run() {
    let mut js = host();
    js.eval("(async () => { await null; mu.notice('after await') })()").unwrap();
    assert_eq!(js.take_actions(), vec![ScriptAction::Notice("after await".into())]);
    js.eval("Promise.reject(new Error('rejected'))").unwrap();
}

#[test]
fn runaway_scripts_are_stopped() {
    let mut js = host();
    let started = Instant::now();
    let err = js.eval("while (true) {}").unwrap_err();
    assert!(err.contains("was stopped"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(5));
    // The runtime still works afterwards.
    assert_eq!(js.eval("2 * 21").unwrap().as_deref(), Some("42"));
}

#[test]
fn script_files() {
    let dir = std::env::temp_dir().join(format!("mu-script-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let good = dir.join("good.js");
    std::fs::write(&good, "function hello() { return 'hi' }").unwrap();
    let bad = dir.join("bad.js");
    std::fs::write(&bad, "\n\nnot valid js (").unwrap();
    let mut js = host();
    js.load_file(&good).unwrap();
    assert_eq!(js.eval("hello()").unwrap().as_deref(), Some("hi"));
    let err = js.load_file(&bad).unwrap_err();
    assert!(err.contains("bad.js"), "the file name is in the error: {err}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn beipmu_hooks_and_objects() {
    let mut js = host();
    // From BeipMU's scripting examples: OnReceive returns true when it has handled the line.
    js.eval(
        r#"
        var count = 0;
        function Toggle(string, window) {
            if (string.search("trigger") > -1) count++;
            if (count == 2) { window.connection.display("<Triggered>" + string); count = 0; return true; }
            return false;
        }
        window.connection.SetOnReceive(Toggle, window);
        function Who(string, window) { if (string == "@@practice") { window.output.write("handled"); return true; } return false; }
        window.Connection.setOnSend(Who, window);
        "#,
    )
    .unwrap();
    assert!(js.on_line("a trigger here"));
    assert!(!js.on_line("trigger again"), "handled lines are hidden");
    assert_eq!(js.on_send("@@practice"), None, "handled commands aren't sent");
    assert_eq!(js.on_send("look").as_deref(), Some("look"));
    assert_eq!(js.take_actions(), [ScriptAction::Display("<Triggered>trigger again".into()), ScriptAction::Display("handled".into())]);

    js.eval(r#"window.run("buy torch"); window.SetVariable("hp", 5); window.input.Set("say "); app.PlaySound("/tmp/x.wav"); window.connection.Receive("fake line")"#)
        .unwrap();
    assert_eq!(
        js.take_actions(),
        [
            ScriptAction::Input("buy torch".into()),
            ScriptAction::SetVariable { name: "hp".into(), value: Some("5".into()) },
            ScriptAction::SetInput("say ".into()),
            ScriptAction::PlaySound("/tmp/x.wav".into()),
            ScriptAction::Receive("fake line".into()),
        ]
    );
    let err = js.eval("app.ActiveXObject('WScript.Shell')").unwrap_err();
    assert!(err.contains("isn't available"), "{err}");
}

#[test]
fn beipmu_text_windows_timers_and_trigger_scripts() {
    let mut js = host();
    js.eval(
        r#"
        var wndText = app.NewWindow_Text();
        window.MakeDocking(wndText.properties.hwnd).Dock(2);
        function ShowText(ranges, line, window) {
            wndText.Add(line.String.substring(ranges.Item(2), ranges.Item(3)) + " " + ranges.Count + " " + window.Title);
        }
        var fired = 0;
        var t = app.CreateInterval(100, function (data) { fired += data; return fired >= 2; }, 1);
        "#,
    )
    .unwrap();
    js.set_context(&ScriptContext { world: "W".into(), character: Some("Bob".into()), ..ScriptContext::default() });
    // Engine-style arguments for a "beipmu:" trigger: the line, then start/end pairs.
    js.call("beipmu:ShowText", &["Bob pages: hi".into(), "0".into(), "13".into(), "0".into(), "3".into()]);
    assert_eq!(js.take_actions(), [ScriptAction::Spawn { window: "Script 1".into(), text: "Bob 4 Bob @ W".into() }]);

    let start = Instant::now();
    for step in 1..=4 {
        js.tick(start + Duration::from_millis(150 * step));
    }
    assert_eq!(js.eval("fired + ' ' + t.Active").unwrap().as_deref(), Some("2 false"), "returning true stops the timer");
    assert!(errors(&js.take_actions()).is_empty());
}
