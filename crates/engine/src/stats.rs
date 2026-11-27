//! The `Beip.Stats` GMCP package: named panes of stats (numbers, text, bars) that the server
//! updates incrementally. See BeipMU's Documentation/GMCP.md.

use std::collections::BTreeMap;

use familiar_ansi::Palette;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StatColor {
    Rgb(u8, u8, u8),
    Transparent,
}

/// Parses "#rrggbb", "ansi256(n)" or "transparent" (case-insensitive).
pub fn parse_color(s: &str) -> Option<StatColor> {
    let s = s.trim().to_ascii_lowercase();
    if s == "transparent" {
        return Some(StatColor::Transparent);
    }
    if let Some(hex) = s.strip_prefix('#')
        && hex.len() == 6
        && hex.bytes().all(|b| b.is_ascii_hexdigit())
    {
        let v = u32::from_str_radix(hex, 16).ok()?;
        return Some(StatColor::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8));
    }
    let n: u8 = s.strip_prefix("ansi256(")?.strip_suffix(')')?.trim().parse().ok()?;
    let c = Palette::default().indexed(n);
    Some(StatColor::Rgb(c.0, c.1, c.2))
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// Colours for a bar. None means the UI's default.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BarColors {
    pub fill: Option<StatColor>,
    pub empty: Option<StatColor>,
    pub outline: Option<StatColor>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum StatValue {
    #[default]
    None,
    Int(i64),
    Text(String),
    Range { value: i64, min: i64, max: i64, colors: BarColors },
    /// `value` is 0.0 to 1.0.
    Progress { label: String, value: f64, colors: BarColors },
}

impl StatValue {
    /// The fraction of the bar that's filled, for range and progress values.
    pub fn fraction(&self) -> Option<f64> {
        match self {
            StatValue::Range { value, min, max, .. } => {
                let span = *max as f64 - *min as f64;
                Some(if span > 0.0 { ((*value as f64 - *min as f64) / span).clamp(0.0, 1.0) } else { 0.0 })
            }
            StatValue::Progress { value, .. } => Some(value.clamp(0.0, 1.0)),
            _ => None,
        }
    }

    /// The text to show for the value (on top of the bar, for bars).
    pub fn text(&self) -> String {
        match self {
            StatValue::None => String::new(),
            StatValue::Int(n) => n.to_string(),
            StatValue::Text(s) => s.clone(),
            StatValue::Range { value, max, .. } => format!("{value} / {max}"),
            StatValue::Progress { label, .. } => label.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Stat {
    /// Leading characters of the key to hide. Lets servers sort stats with prefixes like "0_".
    pub prefix_length: usize,
    pub name_alignment: Align,
    pub name_color: Option<StatColor>,
    pub value_color: Option<StatColor>,
    pub value: StatValue,
}

impl Stat {
    pub fn display_name<'a>(&self, key: &'a str) -> &'a str {
        key.char_indices().nth(self.prefix_length).map_or("", |(i, _)| &key[i..])
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct StatPane {
    pub background: Option<StatColor>,
    /// Sorted by key, which is how they're displayed.
    pub values: BTreeMap<String, Stat>,
}

#[derive(Debug, Default)]
pub struct Stats {
    panes: BTreeMap<String, StatPane>,
}

fn as_color(v: &Value) -> Option<StatColor> {
    v.as_str().and_then(parse_color)
}

fn merge_bar(colors: &mut BarColors, obj: &Map<String, Value>) {
    for (k, v) in obj {
        match k.as_str() {
            // "bar-fill" appears in BeipMU's docs and "bar-color" in its pre-309 builds.
            "fill-color" | "bar-fill" | "bar-color" => colors.fill = as_color(v),
            "empty-color" => colors.empty = as_color(v),
            "outline-color" => colors.outline = as_color(v),
            _ => {}
        }
    }
}

fn merge_stat(stat: &mut Stat, obj: &Map<String, Value>) {
    // Properties apply in document order, so the last value type listed wins.
    for (k, v) in obj {
        match k.as_str() {
            "prefix-length" => stat.prefix_length = v.as_u64().unwrap_or(0) as usize,
            "name-alignment" => {
                stat.name_alignment = match v.as_str().unwrap_or("") {
                    "right" => Align::Right,
                    "center" => Align::Center,
                    _ => Align::Left,
                }
            }
            "color" => {
                stat.name_color = as_color(v);
                stat.value_color = stat.name_color;
            }
            "name-color" => stat.name_color = as_color(v),
            "value-color" => stat.value_color = as_color(v),
            "int" => stat.value = StatValue::Int(v.as_i64().unwrap_or_else(|| v.as_f64().unwrap_or(0.0) as i64)),
            "string" => stat.value = StatValue::Text(v.as_str().map_or_else(|| v.to_string(), str::to_string)),
            "range" => {
                let Some(obj) = v.as_object() else { continue };
                if !matches!(stat.value, StatValue::Range { .. }) {
                    stat.value = StatValue::Range { value: 0, min: 0, max: 0, colors: BarColors::default() };
                }
                if let StatValue::Range { value, min, max, colors } = &mut stat.value {
                    let int = |v: &Value| v.as_i64().unwrap_or_else(|| v.as_f64().unwrap_or(0.0) as i64);
                    if let Some(x) = obj.get("value") {
                        *value = int(x);
                    }
                    if let Some(x) = obj.get("min") {
                        *min = int(x);
                    }
                    if let Some(x) = obj.get("max") {
                        *max = int(x);
                    }
                    merge_bar(colors, obj);
                }
            }
            "progress" => {
                let Some(obj) = v.as_object() else { continue };
                if !matches!(stat.value, StatValue::Progress { .. }) {
                    stat.value = StatValue::Progress { label: String::new(), value: 0.0, colors: BarColors::default() };
                }
                if let StatValue::Progress { label, value, colors } = &mut stat.value {
                    if let Some(x) = obj.get("label") {
                        *label = x.as_str().unwrap_or("").to_string();
                    }
                    if let Some(x) = obj.get("value") {
                        *value = x.as_f64().unwrap_or(0.0);
                    }
                    merge_bar(colors, obj);
                }
            }
            _ => {}
        }
    }
}

impl Stats {
    pub fn pane(&self, name: &str) -> Option<&StatPane> {
        self.panes.get(name)
    }

    pub fn clear(&mut self) {
        self.panes.clear();
    }

    /// Applies a `beip.stats` message and returns the names of the panes it changed.
    pub fn apply(&mut self, json: &str) -> Result<Vec<String>, String> {
        let root: Value = serde_json::from_str(json).map_err(|e| format!("beip.stats: {e}"))?;
        let Some(root) = root.as_object() else { return Err("beip.stats: expected an object".into()) };
        let mut changed = Vec::new();
        for (pane_name, pane_value) in root {
            changed.push(pane_name.clone());
            let Some(obj) = pane_value.as_object() else {
                self.panes.remove(pane_name);
                continue;
            };
            let pane = self.panes.entry(pane_name.clone()).or_default();
            if let Some(bg) = obj.get("background-color") {
                pane.background = as_color(bg);
            }
            match obj.get("values") {
                Some(Value::Null) => pane.values.clear(),
                Some(Value::Object(values)) => {
                    for (key, v) in values {
                        match v.as_object() {
                            // null or {} deletes the stat.
                            Some(props) if !props.is_empty() => merge_stat(pane.values.entry(key.clone()).or_default(), props),
                            _ => {
                                pane.values.remove(key);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(parse_color("#FF8000"), Some(StatColor::Rgb(255, 128, 0)));
        assert_eq!(parse_color("Ansi256(196)"), Some(StatColor::Rgb(255, 0, 0)));
        assert_eq!(parse_color("transparent"), Some(StatColor::Transparent));
        assert_eq!(parse_color("red"), None);
    }

    #[test]
    fn documented_example_and_updates() {
        let mut stats = Stats::default();
        let changed = stats
            .apply(
                r##"{"Player":{"values":{
                    "0_Name":{"prefix-length":2,"string":"Bennet","name-color":"Ansi256(56)"},
                    "1_Hit Points":{"prefix-length":2,"range":{"value":823,"max":1000,"bar-fill":"#00FF00"}},
                    "4_Money":{"prefix-length":2,"int":123456,"color":"#FFFF00","name-alignment":"right"},
                    "5_Progress":{"prefix-length":2,"progress":{"label":"75%","value":0.75}}
                  },"background-color":"#002040"},
                  "Attributes":{"values":{"Strength":{"int":10}}}}"##,
            )
            .unwrap();
        assert_eq!(changed, vec!["Player", "Attributes"]);
        let player = stats.pane("Player").unwrap();
        assert_eq!(player.background, Some(StatColor::Rgb(0, 0x20, 0x40)));
        let keys: Vec<_> = player.values.iter().map(|(k, s)| s.display_name(k)).collect();
        assert_eq!(keys, vec!["Name", "Hit Points", "Money", "Progress"]);
        let hp = &player.values["1_Hit Points"];
        assert_eq!(hp.value.text(), "823 / 1000");
        assert!((hp.value.fraction().unwrap() - 0.823).abs() < 1e-9);
        let money = &player.values["4_Money"];
        assert_eq!((money.name_color, money.value_color), (Some(StatColor::Rgb(255, 255, 0)), Some(StatColor::Rgb(255, 255, 0))));
        assert_eq!(money.name_alignment, Align::Right);

        // Partial update keeps the other range fields.
        stats.apply(r#"{"Player":{"values":{"1_Hit Points":{"range":{"value":500}}}}}"#).unwrap();
        let hp = &stats.pane("Player").unwrap().values["1_Hit Points"];
        assert_eq!(hp.value.text(), "500 / 1000");
        assert!(matches!(&hp.value, StatValue::Range { colors, .. } if colors.fill == Some(StatColor::Rgb(0, 255, 0))));
    }

    #[test]
    fn deletes_and_last_type_wins() {
        let mut stats = Stats::default();
        stats.apply(r#"{"P":{"values":{"a":{"int":1},"b":{"int":2},"c":{"int":3,"string":"three"}}}}"#).unwrap();
        assert_eq!(stats.pane("P").unwrap().values["c"].value, StatValue::Text("three".into()));
        stats.apply(r#"{"P":{"values":{"a":null,"b":{}}}}"#).unwrap();
        assert_eq!(stats.pane("P").unwrap().values.keys().collect::<Vec<_>>(), vec!["c"]);
        stats.apply(r#"{"P":{"values":null}}"#).unwrap();
        assert!(stats.pane("P").unwrap().values.is_empty());
        stats.apply(r#"{"P":null}"#).unwrap();
        assert!(stats.pane("P").is_none());
        assert!(stats.apply("[1]").is_err());
    }

    #[test]
    fn prefix_longer_than_name() {
        let stat = Stat { prefix_length: 10, ..Stat::default() };
        assert_eq!(stat.display_name("ab"), "");
        let stat = Stat { prefix_length: 1, ..Stat::default() };
        assert_eq!(stat.display_name("é_x"), "_x");
    }
}
