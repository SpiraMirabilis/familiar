use super::*;

const SAMPLE: &str = include_str!("../../tests/data/SampleConfig.txt");

fn world<'a>(imported: &'a Imported, name: &str) -> &'a World {
    imported.worlds.iter().find(|w| w.name == name).unwrap_or_else(|| panic!("no world {name}"))
}

fn trigger<'a>(rules: &'a Rules, name: &str) -> &'a Trigger {
    rules.triggers.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no trigger {name}"))
}

#[test]
fn sample_config_worlds_and_characters() {
    let imported = import(SAMPLE);
    let names: Vec<&str> = imported.worlds.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, ["LambdaMOO", "CthulhuMud", "Heroes Assemble Mush"]);

    let lambda = world(&imported, "LambdaMOO");
    assert_eq!((lambda.host.as_str(), lambda.port), ("lambda.moo.mud.org", 8888));
    assert_eq!(lambda.encoding, "Auto");
    assert!(!lambda.verify_certificate);
    assert_eq!(lambda.characters[0].name, "Guest");
    assert_eq!(lambda.characters[0].login, "connect guest");
    let who = trigger(&lambda.rules, "WHO List Spawn Window");
    assert_eq!(who.spawn, "WHO");
    assert!(who.gag, "spawned lines move out of the main window");
    assert!(imported.warnings.iter().any(|w| w.contains("WHO List Spawn Window") && w.contains("capturing")));

    let heroes = world(&imported, "Heroes Assemble Mush");
    assert_eq!(heroes.characters[0].login, "/delay 3s \"connect Guest guest\"", "%NAME% and %PASSWORD% are filled in");
}

#[test]
fn sample_config_global_triggers() {
    let imported = import(SAMPLE);
    let g = &imported.global;

    let sound = trigger(g, "Activity Sound");
    assert!(!sound.enabled, "away-only triggers come in disabled");
    assert_eq!(sound.sound, r"C:\Windows\Media\chord.wav");

    assert_eq!(trigger(g, "Speak all text").speak, "$0");

    let names = trigger(g, "Highlight words");
    let h = names.highlight.as_ref().unwrap();
    assert_eq!(h.bg.as_deref(), Some("#008000"));
    assert!(h.groups && h.fg.is_none());
    assert!(names.matcher.regex && !names.matcher.case_sensitive);

    assert_eq!(trigger(g, "Censor bad words").replace.as_deref(), Some("****"));
    assert_eq!(trigger(g, "Empty line").replace.as_deref(), Some("(empty line detected)"));
    assert!(trigger(g, "Rainbow Text (Hash coloring)").highlight.is_none());
    assert!(imported.warnings.iter().any(|w| w.contains("Rainbow") && w.contains("hashing")));
    let bold = trigger(g, "*Bold*");
    assert_eq!(bold.replace.as_deref(), Some(" $2"), "HTML tags become styling");
    assert!(bold.highlight.as_ref().is_some_and(|h| h.bold && !h.groups));

    // Children of the disabled "Spawn Windows" folder are imported, disabled.
    let channels = trigger(g, "[Channel] Tabs for specific channels");
    assert!(!channels.enabled);
    assert_eq!(channels.spawn, "$1", "the spawn title uses the first group");
}

#[test]
fn sample_config_aliases_and_macros() {
    let imported = import(SAMPLE);
    let g = &imported.global;
    let bt = &g.aliases[0];
    assert!(bt.enabled && bt.substitute && bt.matcher.regex);
    assert_eq!(bt.replacement, "/script for(i=0;i<$1;i++) { window.run(\"buy torch\"); }");
    let mag = g.aliases.iter().find(|a| a.matcher.pattern == "Mag").expect("aliases inside folders are kept");
    assert_eq!((mag.replacement.as_str(), mag.matcher.regex), ("Magdalena", false));

    let keys: Vec<&str> = g.macros.iter().map(|m| m.key.as_str()).collect();
    assert!(keys.contains(&"Ctrl+Alt+A") && keys.contains(&"KP_1") && keys.contains(&"Ctrl+T"), "{keys:?}");
    assert!(!imported.warnings.iter().any(|w| w.starts_with("Config.txt")), "parse errors: {:?}", imported.warnings);
}

#[test]
fn every_imported_pattern_compiles() {
    let imported = import(SAMPLE);
    let mut rules = vec![&imported.global];
    for w in &imported.worlds {
        rules.push(&w.rules);
        rules.extend(w.characters.iter().map(|c| &c.rules));
    }
    for r in rules {
        for p in r.triggers.iter().map(|t| &t.matcher).chain(r.aliases.iter().map(|a| &a.matcher)).filter(|p| p.regex) {
            fancy_regex::Regex::new(&p.pattern).unwrap_or_else(|e| panic!("{}: {e}", p.pattern));
        }
    }
}

#[test]
fn puppets_variables_and_old_forms() {
    let text = r#"Version=200
ScriptStartup="C:\\Users\\me\\beip.js"
Connections
{
  Aliases.Active=false
  Servers
  {
    {
      Name="Old Muck"
      Host="muck.example.org:8888"
      Encoding=UTF8
      TLS=true
      GMCP_WebView=1
      Characters
      {
        {
          Name="Bob"
          Connect="connect %NAME% %PASSWORD%"
          LogFileName="d:\\logfiles\\StH.log"
          LogFileNameTimeFormat=1
          Variables
          {
            {
              Name="target"
              Value="orc"
            }
          }
          Puppets
          {
            Zed
            {
              ReceivePrefix="^Zed> "
              RegularExpression=true
              SendPrefix="z "
              AutoConnect=false
              Triggers
              {
                {
                  FindString.MatchText="x"
                }
              }
            }
          }
          Aliases
          {
            {
              FindString.MatchText="k"
              Replace="kill"
            }
          }
          KeyboardMacros
          {
            KeyMacro=F5,"look",T
            KeyMacro=Shift+NumPad8,"north"
          }
        }
      }
      Triggers
      {
        {
          Description="Starts"
          FindString
          {
            MatchText="You (are) hungry."
            StartsWith=true
            MatchCase=true
          }
          Send
          {
            Active=true
            Send="eat \\1 $5\r\nsmile"
          }
          Triggers
          {
            {
              FindString.MatchText="child"
            }
          }
        }
      }
    }
  }
}
"#;
    let imported = import(text);
    assert_eq!(imported.startup_script.as_deref(), Some(r"C:\Users\me\beip.js"));
    let w = &imported.worlds[0];
    assert_eq!((w.name.as_str(), w.port, w.encoding.as_str(), w.tls, w.allow_webviews), ("Old Muck", 8888, "UTF-8", true, Some(true)));
    let bob = &w.characters[0];
    assert_eq!((bob.name.as_str(), bob.login.as_str()), ("Bob", "connect Bob %PASSWORD%"), "no password: left as it is");
    assert_eq!(bob.rules.variables.get("target").map(String::as_str), Some("orc"));
    assert_eq!((bob.log_file.as_str(), bob.log_date, bob.restore_log), ("StH.log", true, true), "RestoreLog is on unless it was switched off");
    assert!(imported.warnings.iter().any(|w| w.contains("logs to StH.log in the logs folder")));
    let zed = &bob.puppets[0];
    assert_eq!((zed.name.as_str(), zed.receive_prefix.as_str(), zed.regex, zed.auto_open), ("Zed", "^Zed> ", true, false));
    assert!(imported.warnings.iter().any(|w| w.contains("Puppet \"Zed\"")));
    assert!(!bob.rules.aliases[0].enabled, "aliases were switched off globally");
    assert!(imported.warnings.iter().any(|w| w.contains("Process Aliases")));
    let macros: Vec<(&str, &str, bool)> = bob.rules.macros.iter().map(|m| (m.key.as_str(), m.command.as_str(), m.insert)).collect();
    assert_eq!(macros, [("F5", "look", true), ("Shift+KP_8", "north", false)]);

    let starts = &w.rules.triggers[0];
    assert_eq!(starts.matcher, Pattern { pattern: r"^You \(are\) hungry\.".into(), regex: true, case_sensitive: true, whole_word: false });
    assert_eq!(starts.send, "eat $1 $$5\nsmile");
    assert!(imported.warnings.iter().any(|w| w.contains("\"child\"") && w.contains("only ran on lines that \"Starts\"")));
}

#[test]
fn merge_is_idempotent() {
    let imported = import(SAMPLE);
    let mut config = Config::default();
    let first = merge(&mut config, &imported);
    assert_eq!(first.worlds_added.len(), 3);
    assert!(first.triggers > 5 && first.aliases > 5, "{first:?}");
    assert!(first.describe().starts_with("Imported 3 worlds, 3 characters"), "{}", first.describe());
    let again = merge(&mut config, &imported);
    assert_eq!((again.worlds_added.len(), again.triggers, again.aliases), (0, 0, 0));
    assert_eq!(again.worlds_skipped.len(), 3);
    assert_eq!(config.worlds.len(), 3);
}

#[test]
fn templates_and_keys() {
    assert_eq!(template(r"say \1 costs $5 \a12 \x"), r"say $1 costs $$5 ${12} \x");
    assert_eq!(template(r"literal \\1 stays"), r"literal \1 stays");
    assert_eq!(key_chord("Control+Alt+A").as_deref(), Some("Ctrl+Alt+A"));
    assert_eq!(key_chord("?Shift+NumPad0").as_deref(), Some("KP_0"));
    assert_eq!(key_chord("F11").as_deref(), Some("F11"));
    assert_eq!(key_chord("PageUp").as_deref(), Some("Page_Up"));
    assert_eq!(key_chord("Gizmo"), None);
}

#[test]
fn ipv6_host_log_traversal_and_disabled_items() {
    let text = r#"Connections
{
  Servers
  {
    {
      Name="IPv6 World"
      Host="[2001:db8::1]:4201"
      Characters
      {
        {
          Name="Ada"
          LogFileName="../../etc/passwd"
          Aliases
          {
            {
              Disabled=true
              FindString.MatchText="a"
              Replace="b"
            }
          }
          KeyboardMacros2
          {
            {
              Disabled=true
              Key="F6"
              Macro="nope"
            }
          }
        }
      }
    }
  }
}
"#;
    let imported = import(text);
    let w = &imported.worlds[0];
    assert_eq!((w.host.as_str(), w.port), ("2001:db8::1", 4201));
    let ada = &w.characters[0];
    assert_eq!(ada.log_file, "passwd");
    assert!(!ada.rules.aliases[0].enabled);
    assert!(ada.rules.macros.is_empty());
}
