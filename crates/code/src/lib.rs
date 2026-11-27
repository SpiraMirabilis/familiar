//! Support for editing MU* programs that doesn't depend on a UI toolkit: block-structured
//! auto-indentation for MOO, MUF and Lua. (Syntax highlighting in the GTK client comes from
//! GtkSourceView language definitions; the terminal UI will need a tokenizer here too.)

/// A language with indentation rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Moo,
    Muf,
    Lua,
}

impl Lang {
    /// From a GtkSourceView-style language id ("moo", "muf", "lua").
    pub fn from_id(id: &str) -> Option<Self> {
        match id.to_ascii_lowercase().as_str() {
            "moo" => Some(Self::Moo),
            "muf" => Some(Self::Muf),
            "lua" => Some(Self::Lua),
            _ => None,
        }
    }

    fn openers(self) -> &'static [&'static str] {
        match self {
            Self::Moo => &["if", "for", "while", "fork", "try"],
            Self::Muf => &[":", "if", "begin", "for", "foreach", "try"],
            Self::Lua => &["function", "do", "then", "repeat"],
        }
    }

    /// Words that end one block and start another on the same line, like `else`.
    fn middles(self) -> &'static [&'static str] {
        match self {
            Self::Moo => &["else", "elseif", "except", "finally"],
            Self::Muf => &["else", "catch", "catch_detailed"],
            Self::Lua => &["else"],
        }
    }

    fn closers(self) -> &'static [&'static str] {
        match self {
            Self::Moo => &["endif", "endfor", "endwhile", "endfork", "endtry"],
            Self::Muf => &["then", "repeat", "until", ";", "endcatch"],
            // "elseif x then" closes with elseif and reopens with then.
            Self::Lua => &["end", "until", "elseif"],
        }
    }
}

/// Lexer state that carries from one line to the next.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum Carry {
    #[default]
    Code,
    /// Inside a block comment: MOO `/* */`, MUF `( )` or Lua `--[[ ]]`.
    Comment,
    /// Inside a Lua `[[ ]]` string.
    LongString,
}

/// The block words on one line, lowercased, outside strings and comments.
fn line_words(line: &str, lang: Lang, carry: &mut Carry) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut words = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match *carry {
            Carry::Comment => {
                let end = match lang {
                    Lang::Moo => c == '*' && chars.get(i + 1) == Some(&'/'),
                    Lang::Muf => c == ')',
                    Lang::Lua => c == ']' && chars.get(i + 1) == Some(&']'),
                };
                if end {
                    *carry = Carry::Code;
                    i += if lang == Lang::Muf { 1 } else { 2 };
                } else {
                    i += 1;
                }
                continue;
            }
            Carry::LongString => {
                if c == ']' && chars.get(i + 1) == Some(&']') {
                    *carry = Carry::Code;
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            Carry::Code => {}
        }
        // Strings: skip to the closing quote, honouring backslash escapes.
        if c == '"' || (c == '\'' && lang == Lang::Lua) {
            i += 1;
            while i < chars.len() && chars[i] != c {
                i += if chars[i] == '\\' { 2 } else { 1 };
            }
            i += 1;
            continue;
        }
        match lang {
            Lang::Moo if c == '/' && chars.get(i + 1) == Some(&'*') => {
                *carry = Carry::Comment;
                i += 2;
                continue;
            }
            Lang::Muf if c == '(' => {
                *carry = Carry::Comment;
                i += 1;
                continue;
            }
            Lang::Lua if c == '-' && chars.get(i + 1) == Some(&'-') => {
                if chars.get(i + 2) == Some(&'[') && chars.get(i + 3) == Some(&'[') {
                    *carry = Carry::Comment;
                    i += 4;
                    continue;
                }
                break; // A line comment.
            }
            Lang::Lua if c == '[' && chars.get(i + 1) == Some(&'[') => {
                *carry = Carry::LongString;
                i += 2;
                continue;
            }
            _ => {}
        }
        if lang == Lang::Muf {
            // MUF words are whitespace-separated, so ":" and ";" are words too.
            if c.is_whitespace() {
                i += 1;
                continue;
            }
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '"' && chars[i] != '(' {
                i += 1;
            }
            words.push(chars[start..i].iter().collect::<String>().to_lowercase());
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            // In MOO, x.if and obj:for() are property and verb names, not keywords.
            let previous = chars[..start].iter().rev().find(|c| !c.is_whitespace());
            let is_member = lang == Lang::Moo && matches!(previous, Some('.' | ':' | '$'));
            if !is_member {
                words.push(chars[start..i].iter().collect::<String>().to_lowercase());
            }
            continue;
        }
        i += 1;
    }
    words
}

/// Indentation levels for every line of `text`, plus the level for a line after the last.
#[derive(Debug, Clone, PartialEq)]
pub struct Indents {
    pub lines: Vec<usize>,
    pub next: usize,
}

pub fn indents(text: &str, lang: Lang) -> Indents {
    let mut depth = 0usize;
    let mut carry = Carry::default();
    let mut lines = Vec::new();
    // `str::lines` drops a trailing empty line; split keeps it, so the cursor's line counts.
    for line in text.split('\n') {
        let starts_in_comment = carry != Carry::Code;
        let words = line_words(line, lang, &mut carry);
        let first = words.first().map(String::as_str);
        let dedent = !starts_in_comment
            && first.is_some_and(|w| lang.closers().contains(&w) || lang.middles().contains(&w));
        lines.push(depth.saturating_sub(dedent as usize));
        for w in &words {
            if lang.openers().contains(&w.as_str()) {
                depth += 1;
            } else if lang.closers().contains(&w.as_str()) {
                depth = depth.saturating_sub(1);
            }
        }
    }
    Indents { lines, next: depth }
}

/// What to do when Enter is pressed: `before_cursor` is all the text before the cursor.
/// Returns the level for the line being finished and for the new line.
pub fn on_enter(before_cursor: &str, lang: Lang) -> (usize, usize) {
    let indents = indents(before_cursor, lang);
    (*indents.lines.last().unwrap_or(&0), indents.next)
}

/// Re-indents every line with `unit` per level. Blank lines become empty. Lines inside block
/// comments keep their own indentation.
pub fn reindent(text: &str, lang: Lang, unit: &str) -> String {
    let mut depth = 0usize;
    let mut carry = Carry::default();
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let (line, cr) = match raw.strip_suffix('\r') {
            Some(l) => (l, "\r"),
            None => (raw, ""),
        };
        let starts_in_comment = carry != Carry::Code;
        let words = line_words(line, lang, &mut carry);
        let first = words.first().map(String::as_str);
        let dedent = !starts_in_comment && first.is_some_and(|w| lang.closers().contains(&w) || lang.middles().contains(&w));
        let level = depth.saturating_sub(dedent as usize);
        for w in &words {
            if lang.openers().contains(&w.as_str()) {
                depth += 1;
            } else if lang.closers().contains(&w.as_str()) {
                depth = depth.saturating_sub(1);
            }
        }
        let body = line.trim_start();
        out.push(if body.is_empty() {
            cr.to_string()
        } else if starts_in_comment {
            format!("{line}{cr}")
        } else {
            format!("{}{body}{cr}", unit.repeat(level))
        });
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moo(text: &str) -> String {
        reindent(text, Lang::Moo, "  ")
    }

    #[test]
    fn moo_blocks() {
        let code = "\
if (x)
y = 1;
elseif (z)
for i in [1..5]
try
player:tell(i);
except e (ANY)
continue;
endtry
endfor
else
while outer (1)
fork (0)
return;
endfork
endwhile
endif
x = 1;";
        let expected = "\
if (x)
  y = 1;
elseif (z)
  for i in [1..5]
    try
      player:tell(i);
    except e (ANY)
      continue;
    endtry
  endfor
else
  while outer (1)
    fork (0)
      return;
    endfork
  endwhile
endif
x = 1;";
        assert_eq!(moo(code), expected);
        // Already-indented code stays the same.
        assert_eq!(moo(expected), expected);
    }

    #[test]
    fn moo_ignores_strings_comments_and_member_names() {
        let code = "\
\"if this were code, endif\";
/* if
   endfor */
x = this.if + obj:for();
y = $while;
if (a) b; endif
z;";
        let expected = "\
\"if this were code, endif\";
/* if
   endfor */
x = this.if + obj:for();
y = $while;
if (a) b; endif
z;";
        assert_eq!(moo(code), expected);
    }

    #[test]
    fn keywords_are_case_insensitive_and_unbalanced_code_is_safe() {
        assert_eq!(moo("IF (x)\ny;\nENDIF"), "IF (x)\n  y;\nENDIF");
        assert_eq!(moo("endif\nendif\nx;"), "endif\nendif\nx;");
    }

    #[test]
    fn enter_dedents_the_finished_line() {
        // Typing "endif" and pressing Enter: that line moves out, the next starts at 0.
        assert_eq!(on_enter("if (x)\n  y;\n  endif", Lang::Moo), (0, 0));
        assert_eq!(on_enter("if (x)", Lang::Moo), (0, 1));
        assert_eq!(on_enter("if (x)\n  y;\n  else", Lang::Moo), (0, 1));
        assert_eq!(on_enter("", Lang::Moo), (0, 0));
    }

    #[test]
    fn muf() {
        let code = "\
( a comment with if and then )
: main ( s -- )
me @ swap notify
begin
dup while
if \"yes\" else \"no\" then
1 - repeat
foreach
pop
repeat
;";
        let expected = "\
( a comment with if and then )
: main ( s -- )
  me @ swap notify
  begin
    dup while
    if \"yes\" else \"no\" then
    1 - repeat
  foreach
    pop
  repeat
;";
        assert_eq!(reindent(code, Lang::Muf, "  "), expected);
    }

    #[test]
    fn lua() {
        let code = "\
function f(x)
if x then
return 1
elseif y then
for i = 1, 3 do
print(i) -- end
end
else
local s = [[ end ]]
end
end";
        let expected = "\
function f(x)
  if x then
    return 1
  elseif y then
    for i = 1, 3 do
      print(i) -- end
    end
  else
    local s = [[ end ]]
  end
end";
        assert_eq!(reindent(code, Lang::Lua, "  "), expected);
    }

    #[test]
    fn language_ids() {
        assert_eq!(Lang::from_id("MOO"), Some(Lang::Moo));
        assert_eq!(Lang::from_id("python3"), None);
    }
}

