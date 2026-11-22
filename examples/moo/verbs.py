"""The MCP simple-edit feature's verbs, written once and used for the install script and the
emergency-mode test. Each entry: (verb names, dobj, prep, iobj, code)."""

CONFUNC = r'''
"Starts MCP 2.1 when a player with this feature connects. MCP clients answer with an";
"authentication key (see :handle_line); other clients just show this one line.";
sessions = this:_sessions();
sessions[player] = ["key" -> "", "pending" -> []];
this.sessions = sessions;
notify(player, "#$#mcp version: 2.1 to: 2.1");
'''

DISFUNC = r'''
"Forgets a player's MCP session when they disconnect.";
sessions = this:_sessions();
if (player in mapkeys(sessions))
  this.sessions = mapdelete(sessions, player);
endif
'''

SESSIONS = r'''
"The per-player MCP state: player -> [\"key\" -> authentication key, \"pending\" -> unfinished multiline messages].";
if (caller != this)
  return E_PERM;
endif
if (typeof(this.sessions) != MAP)
  this.sessions = [];
endif
return this.sessions;
'''

OOB_COMMAND = r'''
"Catches the client's MCP lines when the server hands #$# lines to the command parser.";
"(Servers built with OUT_OF_BAND_PREFIX send them to #0:do_out_of_band_command instead.)";
this:handle_line(argstr ? verb + " " + argstr | verb);
'''

HANDLE_LINE = r'''
"Handles one #$# line from the client, from :#$#* or #0:do_out_of_band_command.";
{line} = args;
if (caller != this && caller != #0 && !caller_perms().wizard)
  return E_PERM;
endif
sessions = this:_sessions();
if (!(player in mapkeys(sessions)) || index(line, "#$#") != 1)
  return 0;
endif
session = sessions[player];
body = line[4..$];
if (index(body, "mcp ") == 1)
  "The client's answer to our #$#mcp line, carrying its authentication key.";
  parsed = this:parse_keyvals(body[5..$]);
  key = typeof(parsed) == LIST ? `parsed[1]["authentication-key"] ! E_RANGE => ""' | "";
  if (!key)
    return 0;
  endif
  session["key"] = key;
  sessions[player] = session;
  this.sessions = sessions;
  this:send("mcp-negotiate-can", ["package" -> "mcp-negotiate", "min-version" -> "2.0", "max-version" -> "2.0"]);
  this:send("mcp-negotiate-can", ["package" -> "dns-org-mud-moo-simpleedit", "min-version" -> "1.0", "max-version" -> "1.0"]);
  this:send("mcp-negotiate-end", []);
  return 1;
endif
if (!session["key"])
  return 0;
endif
pending = session["pending"];
if (index(body, "* ") == 1)
  "One line of a multiline value: #$#* <tag> <key>: <value>";
  rest = body[3..$];
  space = index(rest, " ");
  colon = index(rest, ": ");
  tag = space ? rest[1..space - 1] | "";
  if (!space || !colon || !(tag in mapkeys(pending)))
    return 0;
  endif
  key = rest[space + 1..colon - 1];
  {name, vals} = pending[tag];
  vals[key] = {@`vals[key] ! E_RANGE => {}', rest[colon + 2..$]};
  pending[tag] = {name, vals};
elseif (index(body, ": ") == 1)
  "The end of a multiline message: #$#: <tag>";
  tag = body[3..$];
  if (!(tag in mapkeys(pending)))
    return 0;
  endif
  {name, vals} = pending[tag];
  session["pending"] = mapdelete(pending, tag);
  sessions[player] = session;
  this.sessions = sessions;
  this:dispatch(name, vals);
  return 1;
else
  "A message: #$#<name> <authentication key> <key: value> ...";
  space = index(body, " ");
  if (!space)
    return 0;
  endif
  name = body[1..space - 1];
  rest = body[space + 1..$];
  space = index(rest, " ");
  given = space ? rest[1..space - 1] | rest;
  if (strcmp(given, session["key"]) != 0)
    "The wrong key: this didn't come from the player's client.";
    return 0;
  endif
  parsed = this:parse_keyvals(space ? rest[space + 1..$] | "");
  if (typeof(parsed) != LIST)
    return 0;
  endif
  {vals, multi} = parsed;
  if (!multi)
    this:dispatch(name, vals);
    return 1;
  endif
  tag = `vals["_data-tag"] ! E_RANGE => ""';
  if (!tag)
    return 0;
  endif
  pending[tag] = {name, vals};
endif
session["pending"] = pending;
sessions[player] = session;
this.sessions = sessions;
return 1;
'''

PARSE_KEYVALS = r'''
"Parses MCP keyvals (key: value key: \"quoted value\" lines*: \"\") into {values map, multiline keys}.";
"Multiline keys start out as empty lists. Returns E_INVARG if the text is malformed.";
{text} = args;
if (caller != this)
  return E_PERM;
endif
vals = [];
multi = {};
i = 1;
n = length(text);
while (1)
  while (i <= n && text[i] == " ")
    i = i + 1;
  endwhile
  if (i > n)
    return {vals, multi};
  endif
  start = i;
  while (i <= n && index("abcdefghijklmnopqrstuvwxyz0123456789_-", text[i]))
    i = i + 1;
  endwhile
  key = text[start..i - 1];
  is_multi = i <= n && text[i] == "*";
  if (is_multi)
    i = i + 1;
  endif
  if (!key || text[i..min(i + 1, n)] != ": ")
    return E_INVARG;
  endif
  i = i + 2;
  if (i <= n && text[i] == "\"")
    value = "";
    i = i + 1;
    while (i <= n && text[i] != "\"")
      if (text[i] == "\\" && i < n)
        i = i + 1;
      endif
      value = value + text[i];
      i = i + 1;
    endwhile
    i = i + 1;
  else
    start = i;
    while (i <= n && text[i] != " ")
      i = i + 1;
    endwhile
    value = text[start..i - 1];
  endif
  if (is_multi)
    multi = {@multi, key};
    vals[key] = {};
  else
    vals[key] = value;
  endif
endwhile
'''

DISPATCH = r'''
"Acts on a complete message from the client.";
{name, vals} = args;
if (caller != this)
  return E_PERM;
endif
if (name == "dns-org-mud-moo-simpleedit-set")
  content = `vals["content"] ! E_RANGE => {}';
  this:handle_set(`vals["reference"] ! E_RANGE => ""', `vals["type"] ! E_RANGE => ""', typeof(content) == LIST ? content | {content});
endif
"The client's mcp-negotiate-can and mcp-negotiate-end need no reply.";
'''

SEND = r'''
"Sends a message to the player's client: :send(name, values map [, multiline values map]).";
{name, vals, ?multi = []} = args;
if (caller != this)
  return E_PERM;
endif
line = "#$#" + name + " " + this:_sessions()[player]["key"];
for k in (mapkeys(vals))
  line = line + " " + k + ": " + this:quote(vals[k]);
endfor
tail = {};
for k in (mapkeys(multi))
  tag = tostr(random(1000000000));
  line = line + " " + k + "*: \"\" _data-tag: " + tag;
  for item in (multi[k])
    tail = {@tail, "#$#* " + tag + " " + k + ": " + item};
  endfor
  tail = {@tail, "#$#: " + tag};
endfor
notify(player, line);
for item in (tail)
  notify(player, item);
endfor
'''

QUOTE = r'''
"Quotes a value for MCP: \"...\" with backslashes and quotes escaped.";
return "\"" + strsub(strsub(tostr(args[1]), "\\", "\\\\"), "\"", "\\\"") + "\"";
'''

SEDIT = r'''
"Usage:  @sedit <object>:<verb>   or   @sedit <object>.<property>";
"";
"Opens a verb, or a text property, in your client's editor (MCP simpleedit). When you send it";
"back from the editor it is saved with your own permissions. Compile errors come back as";
"\"Line N:  ...\", which editors that understand them mark on the right lines.";
spec = argstr;
sessions = this:_sessions();
if (!(player in mapkeys(sessions)) || !sessions[player]["key"])
  player:tell("@sedit needs an MCP client. Connect with one (Familiar, for example) and try again.");
  return;
endif
colon = index(spec, ":");
dot = index(spec, ".");
if (!spec || (!colon && !dot))
  player:tell("Usage: @sedit <object>:<verb>  or  @sedit <object>.<property>");
  return;
endif
is_verb = colon && (!dot || colon < dot);
split = is_verb ? colon | dot;
what = spec[split + 1..$];
o = this:match_object(spec[1..split - 1]);
if (!valid(o))
  player:tell("I don't see \"", spec[1..split - 1], "\" here.");
  return;
endif
"Read it with the player's own permissions, not this verb owner's.";
set_task_perms(player);
if (is_verb)
  where = o;
  code = E_VERBNF;
  while (valid(where) && (code = `verb_code(where, what) ! ANY') == E_VERBNF)
    where = `parents(where)[1] ! ANY => #-1';
  endwhile
  if (typeof(code) == ERR)
    player:tell("Can't read ", spec, ": ", code == E_VERBNF ? "there's no such verb" | tostr(code));
    return;
  endif
  ref = tostr(where) + ":" + what;
  this:send("dns-org-mud-moo-simpleedit-content", ["reference" -> ref, "name" -> ref, "type" -> "moo-code"], ["content" -> code]);
  return;
endif
value = `o.(what) ! ANY';
ref = tostr(o) + "." + what;
if (typeof(value) == STR)
  this:send("dns-org-mud-moo-simpleedit-content", ["reference" -> ref, "name" -> ref, "type" -> "string"], ["content" -> {value}]);
  return;
elseif (typeof(value) == LIST)
  for item in (value)
    if (typeof(item) != STR)
      player:tell("Can't edit ", spec, ": @sedit only edits text, or lists of text");
      return;
    endif
  endfor
  this:send("dns-org-mud-moo-simpleedit-content", ["reference" -> ref, "name" -> ref, "type" -> "string-list"], ["content" -> value]);
  return;
endif
player:tell("Can't edit ", spec, ": ", typeof(value) == ERR ? tostr(value) | "@sedit only edits text, or lists of text");
'''

MATCH_OBJECT = r'''
"Finds an object by #number, $name, me or here, or (on LambdaCore) by a name the player can see.";
{name} = args;
if (name == "me")
  o = player;
elseif (name == "here")
  o = player.location;
elseif (index(name, "#") == 1)
  o = toobj(name);
elseif (index(name, "$") == 1)
  o = `#0.(name[2..$]) ! ANY => #-1';
elseif (respond_to(player, "my_match_object"))
  o = player:my_match_object(name);
else
  o = #-1;
endif
return typeof(o) == OBJ ? o | #-1;
'''

HANDLE_SET = r'''
"Saves text sent back from the editor, with the player's own permissions.";
{reference, type, content} = args;
if (caller != this)
  return E_PERM;
endif
set_task_perms(player);
colon = index(reference, ":");
dot = index(reference, ".");
if (colon)
  o = toobj(reference[1..colon - 1]);
  name = reference[colon + 1..$];
  errors = `set_verb_code(o, name, content) ! ANY';
  if (typeof(errors) == ERR)
    player:tell(reference, " was not saved: ", tostr(errors));
  elseif (errors)
    player:tell(reference, " was not saved:");
    for error in (errors)
      player:tell(error);
    endfor
  else
    player:tell(reference, " saved.");
  endif
elseif (dot)
  o = toobj(reference[1..dot - 1]);
  name = reference[dot + 1..$];
  if (type == "string")
    "A single string; lines typed in the editor are joined with spaces.";
    value = "";
    for item in (content)
      value = value ? value + " " + item | item;
    endfor
  else
    value = content;
  endif
  result = `o.(name) = value ! ANY';
  if (typeof(result) == ERR)
    player:tell(reference, " was not saved: ", tostr(result));
  else
    player:tell(reference, " saved.");
  endif
else
  player:tell("Don't know where to save ", reference);
endif
'''

OOB_FORWARDER = r'''
"Hands MCP lines (#$#...) to the MCP simple-edit feature, for servers built with OUT_OF_BAND_PREFIX.";
"Replace FEATURE with the feature's object number.";
if (callers())
  return E_PERM;
endif
if (valid(player) && FEATURE in `player.features ! ANY => {}')
  return FEATURE:handle_line(argstr);
endif
'''

VERBS = [
    ("confunc", "this", "none", "this", CONFUNC),
    ("disfunc", "this", "none", "this", DISFUNC),
    ("_sessions", "this", "none", "this", SESSIONS),
    ("#$#*", "any", "any", "any", OOB_COMMAND),
    ("handle_line", "this", "none", "this", HANDLE_LINE),
    ("parse_keyvals", "this", "none", "this", PARSE_KEYVALS),
    ("dispatch", "this", "none", "this", DISPATCH),
    ("send", "this", "none", "this", SEND),
    ("quote", "this", "none", "this", QUOTE),
    ("@sedit @simpleedit", "any", "any", "any", SEDIT),
    ("match_object", "this", "none", "this", MATCH_OBJECT),
    ("handle_set", "this", "none", "this", HANDLE_SET),
]
