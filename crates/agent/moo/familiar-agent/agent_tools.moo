@create $root_class named Familiar agent tools,agent_tools
@corify agent_tools as $agent_tools
@describe $agent_tools as Tools for LLM agents working through the Familiar client, served over MCP 2.1 (dns-com-familiar-agent) by the MCP feature in .dispatcher. To add a tool, add its spec to :specs() and a verb tool_<name>(args map), then run :announce_tools().
@property $agent_tools.allow_eval 0 r
@property $agent_tools.dispatcher #-1 r

@verb $agent_tools:mcp_message this none this rxd
@program $agent_tools:mcp_message
":mcp_message(who, name, values map) - a dns-com-familiar-agent message from who's client, passed on by the MCP feature (this.dispatcher).";
{who, name, vals} = args;
if (caller != this.dispatcher)
  return E_PERM;
endif
if (name == "dns-com-familiar-agent-list")
  this.dispatcher:send_to(who, "dns-com-familiar-agent-tools", [], ["tools" -> this:tools_for(who)]);
elseif (name == "dns-com-familiar-agent-call")
  id = `vals["id"] ! E_RANGE => ""';
  tool = `vals["tool"] ! E_RANGE => ""';
  "args* arrives as a list of pieces; joined with no separator, they're the arguments as JSON.";
  pieces = `vals["args"] ! E_RANGE => {}';
  json = typeof(pieces) == LIST ? tostr(@pieces) | tostr(pieces);
  "Tools can suspend (grep does), so each call runs in a task of its own, with the player's permissions.";
  fork (0)
    set_task_perms(who);
    try
      {ok, lines} = this:call(who, tool, json);
    except e (ANY)
      {ok, lines} = {0, {tostr("The tool failed: ", e[2])}};
    endtry
    this.dispatcher:send_to(who, "dns-com-familiar-agent-result", ["id" -> id, "ok" -> ok ? "1" | "0"], ["content" -> lines]);
  endfork
endif
return 1;
.

@verb $agent_tools:announce_tools this none this rxd
@program $agent_tools:announce_tools
":announce_tools([players]) => the players sent a fresh tool list. Run after changing :specs, :offers or allow_eval.";
"Default: every connected player. The dispatcher's :send_to skips anyone whose client didn't negotiate dns-com-familiar-agent.";
{?who = connected_players()} = args;
sent = {};
for p in (who)
  if (valid(p) && this.dispatcher:send_to(p, "dns-com-familiar-agent-tools", [], ["tools" -> this:tools_for(p)]))
    sent = {@sent, p};
  endif
endfor
return sent;
.

@verb $agent_tools:specs this none this rxd
@program $agent_tools:specs
"Each tool: its name, a description written for a model, and a JSON Schema for its arguments.";
"show is what the player's window shows of a call: \"none\", \"summary\" or \"full\". timeout is in seconds.";
obj = ["type" -> "string", "description" -> "An object: #123, $corified_name, me, here, or a name the player can match"];
verb = ["type" -> "string", "description" -> "A verb name (any of its aliases), or its 1-based index on the object"];
lines = ["type" -> "array", "items" -> ["type" -> "string"], "description" -> "The code, one string per line, without the final \".\""];
return {
  ["name" -> "list_verbs", "description" -> "Lists the verbs defined on an object: index, names, owner, permissions and argument specifiers. With inherited, also those on its ancestors, nearest first.", "input_schema" -> ["type" -> "object", "properties" -> ["obj" -> obj, "inherited" -> ["type" -> "boolean"]], "required" -> {"obj"}], "show" -> "none"],
  ["name" -> "list_verb", "description" -> "The code of a verb, one line per line, after a header line with where it is defined, its names, owner, permissions and arguments. Inherited verbs are found on the ancestor that defines them.", "input_schema" -> ["type" -> "object", "properties" -> ["obj" -> obj, "verb" -> verb], "required" -> {"obj", "verb"}], "show" -> "none"],
  ["name" -> "grep", "description" -> "Finds verbs whose code contains text, or matches a regular expression (MOO match() syntax, % escapes). Searches root and its descendants, or every object. Returns obj:verb, line number and the line.", "input_schema" -> ["type" -> "object", "properties" -> ["pattern" -> ["type" -> "string"], "regex" -> ["type" -> "boolean"], "root" -> obj, "limit" -> ["type" -> "integer", "description" -> "Most matches to return (default 200)"]], "required" -> {"pattern"}], "show" -> "summary", "timeout" -> 300],
  ["name" -> "add_verb", "description" -> "Adds a verb to an object, owned by the player. The verb is empty until program_verb sets its code.", "input_schema" -> ["type" -> "object", "properties" -> ["obj" -> obj, "names" -> ["type" -> "string", "description" -> "Space-separated names, e.g. \"l*ook examine\""], "perms" -> ["type" -> "string", "description" -> "Some of r, w, x, d (default rxd)"], "args" -> ["type" -> "string", "description" -> "dobj prep iobj, e.g. \"this none this\" (the default) or \"any at this\""]], "required" -> {"obj", "names"}]],
  ["name" -> "program_verb", "description" -> "Replaces a verb's code. Fails with the compiler's errors if it doesn't compile, leaving the old code in place.", "input_schema" -> ["type" -> "object", "properties" -> ["obj" -> obj, "verb" -> verb, "code" -> lines], "required" -> {"obj", "verb", "code"}]],
  ["name" -> "delete_verb", "description" -> "Deletes a verb from an object.", "input_schema" -> ["type" -> "object", "properties" -> ["obj" -> obj, "verb" -> verb], "required" -> {"obj", "verb"}], "show" -> "full"],
  ["name" -> "set_verb_info", "description" -> "Changes a verb's permissions (like @chmod), names or owner, and/or its argument specifiers. Leave out what shouldn't change.", "input_schema" -> ["type" -> "object", "properties" -> ["obj" -> obj, "verb" -> verb, "perms" -> ["type" -> "string"], "names" -> ["type" -> "string"], "owner" -> obj, "args" -> ["type" -> "string", "description" -> "dobj prep iobj"]], "required" -> {"obj", "verb"}]],
  ["name" -> "create", "description" -> "Creates an object as a child of parent, owned by the player, and names it. Returns its number.", "input_schema" -> ["type" -> "object", "properties" -> ["parent" -> obj, "name" -> ["type" -> "string"]], "required" -> {"parent", "name"}]],
  ["name" -> "chparent", "description" -> "Changes an object's parent.", "input_schema" -> ["type" -> "object", "properties" -> ["obj" -> obj, "parent" -> obj], "required" -> {"obj", "parent"}]],
  ["name" -> "recycle", "description" -> "Destroys an object. This cannot be undone.", "input_schema" -> ["type" -> "object", "properties" -> ["obj" -> obj], "required" -> {"obj"}], "show" -> "full"],
  ["name" -> "object_info", "description" -> "An object's name, parent, ancestors, owner, location, flags, children count, and the properties defined on it.", "input_schema" -> ["type" -> "object", "properties" -> ["obj" -> obj], "required" -> {"obj"}], "show" -> "none"],
  ["name" -> "get_property", "description" -> "A property's value, as a MOO literal, with its owner and permissions.", "input_schema" -> ["type" -> "object", "properties" -> ["obj" -> obj, "name" -> ["type" -> "string"]], "required" -> {"obj", "name"}], "show" -> "none"],
  ["name" -> "set_property", "description" -> "Sets a property to a value written as a MOO literal (e.g. \"hello\", 3, {#1, \"a\"}). With add, adds it to the object first, owned by the player, with perms (default rc).", "input_schema" -> ["type" -> "object", "properties" -> ["obj" -> obj, "name" -> ["type" -> "string"], "value" -> ["type" -> "string"], "add" -> ["type" -> "boolean"], "perms" -> ["type" -> "string"]], "required" -> {"obj", "name", "value"}]],
  ["name" -> "eval", "description" -> "Runs MOO code as the player (like ;) and returns the value as a literal. Statements need an explicit return.", "input_schema" -> ["type" -> "object", "properties" -> ["code" -> ["type" -> "string"]], "required" -> {"code"}], "show" -> "full"]
};
.

@verb $agent_tools:tools_for this none this rxd
@program $agent_tools:tools_for
":tools_for(who) => the specs who may use, as JSON lines for dns-com-familiar-agent-tools.";
{who} = args;
lines = {};
for spec in (this:specs())
  if (this:offers(spec["name"], who))
    lines = {@lines, generate_json(spec)};
  endif
endfor
return lines;
.

@verb $agent_tools:offers this none this rxd
@program $agent_tools:offers
":offers(tool name, who) => whether who may call it.";
{name, who} = args;
if (name == "eval")
  return this.allow_eval && is_player(who) && who.programmer;
endif
return 1;
.

@verb $agent_tools:call this none this rxd
@program $agent_tools:call
":call(who, tool, args json) => {ok, lines}. Runs with the caller's permissions.";
{who, tool, json} = args;
set_task_perms(caller_perms());
known = 0;
for spec in (this:specs())
  if (spec["name"] == tool)
    known = 1;
    break;
  endif
endfor
if (!known || !this:offers(tool, who))
  return {0, {"There's no tool called " + tool}};
endif
try
  a = json ? parse_json(json) | [];
except e (ANY)
  return {0, {"The arguments aren't JSON: " + e[2]}};
endtry
if (typeof(a) != MAP)
  return {0, {"The arguments must be a JSON object"}};
endif
try
  result = this:("tool_" + tool)(a);
except e (ANY)
  "A tool can raise a list of strings as the error's value, for lines after the message. Builtin errors carry other values (E_PROPNF: {object, name}), which aren't lines.";
  more = length(e) >= 3 && typeof(e[3]) == LIST && this:all_strings(e[3]) ? e[3] | {};
  return {0, {tostr(e[2]), @more}};
endtry
if (typeof(result) == STR)
  return {1, {result}};
elseif (typeof(result) == LIST && this:all_strings(result))
  return {1, result};
endif
try
  return {1, {generate_json(result)}};
except (ANY)
  return {1, {toliteral(result)}};
endtry
.

@verb $agent_tools:all_strings this none this rxd
@program $agent_tools:all_strings
for x in (args[1])
  if (typeof(x) != STR)
    return 0;
  endif
endfor
return 1;
.

@verb $agent_tools:arg this none this rxd
@program $agent_tools:arg
":arg(args map, key [, default]) => the argument, or default (raising E_ARGS when there's none).";
{a, key, ?default = E_NONE} = args;
if (maphaskey(a, key))
  return a[key];
elseif (default == E_NONE)
  raise(E_ARGS, "Missing argument: " + key);
endif
return default;
.

@verb $agent_tools:obj this none this rxd
@program $agent_tools:obj
":obj(text) => the object it names, for the player.";
{s} = args;
set_task_perms(caller_perms());
if (typeof(s) == OBJ)
  o = s;
elseif (typeof(s) != STR || !s)
  raise(E_INVARG, "Not an object: " + toliteral(s));
elseif (s[1] == "#")
  o = toobj(s);
elseif (s[1] == "$")
  o = #0;
  for name in ($string_utils:explode(s[2..$], "."))
    o = o.(name);
  endfor
elseif (s == "me")
  o = player;
elseif (s == "here")
  o = player.location;
else
  o = player:my_match_object(s);
endif
if (typeof(o) != OBJ || !valid(o))
  raise(E_INVARG, "No object " + toliteral(s));
endif
return o;
.

@verb $agent_tools:verb this none this rxd
@program $agent_tools:verb
":verb(o, text) => {where it's defined, the verb's name or index there}.";
{o, v} = args;
set_task_perms(caller_perms());
if (typeof(v) == STR && v && toint(v) > 0 && tostr(toint(v)) == v)
  v = toint(v);
endif
if (typeof(v) == INT)
  return {o, v};
endif
where = o;
while (valid(where))
  try
    verb_info(where, v);
    return {where, v};
  except (E_VERBNF)
  endtry
  where = parent(where);
endwhile
raise(E_VERBNF, "No verb " + toliteral(v) + " on " + tostr(o) + " or its ancestors");
.

@verb $agent_tools:describe_verb this none this rxd
@program $agent_tools:describe_verb
":describe_verb(o, verb) => a one-line summary.";
{o, v} = args;
set_task_perms(caller_perms());
{owner, perms, names} = verb_info(o, v);
{dobj, prep, iobj} = verb_args(o, v);
return tostr(o, ":\"", names, "\" owner ", owner, " perms ", perms, " args ", dobj, " ", prep, " ", iobj);
.

@verb $agent_tools:has_recycler this none this rxd
@program $agent_tools:has_recycler
":has_recycler(verb) => whether the core has $recycler with that verb.";
return "recycler" in properties(#0) && valid($recycler) && $object_utils:has_verb($recycler, args[1]);
.

@verb $agent_tools:pause this none this rxd
@program $agent_tools:pause
"Suspends briefly when the task is running out of ticks or seconds.";
if (ticks_left() < 10000 || seconds_left() < 2)
  suspend(0);
endif
.

@verb $agent_tools:tool_list_verbs this none this rxd
@program $agent_tools:tool_list_verbs
set_task_perms(caller_perms());
{a} = args;
o = this:obj(this:arg(a, "obj"));
places = this:arg(a, "inherited", 0) ? {o, @ancestors(o)} | {o};
lines = {};
for where in (places)
  for i in [1..length(verbs(where))]
    lines = {@lines, tostr(i, ". ", this:describe_verb(where, i))};
  endfor
endfor
return lines || {"No verbs"};
.

@verb $agent_tools:tool_list_verb this none this rxd
@program $agent_tools:tool_list_verb
set_task_perms(caller_perms());
{a} = args;
{where, v} = this:verb(this:obj(this:arg(a, "obj")), this:arg(a, "verb"));
return {this:describe_verb(where, v), @verb_code(where, v, 0, 1)};
.

@verb $agent_tools:tool_grep this none this rxd
@program $agent_tools:tool_grep
set_task_perms(caller_perms());
{a} = args;
pattern = this:arg(a, "pattern");
regex = this:arg(a, "regex", 0);
limit = this:arg(a, "limit", 200);
"Every object, or root and its descendants.";
objects = maphaskey(a, "root") ? {this:obj(a["root"]), @descendants(this:obj(a["root"]))} | 0;
found = {};
for i in [1..objects ? length(objects) | toint(max_object()) + 1]
  o = objects ? objects[i] | toobj(i - 1);
  if (valid(o))
    for v in [1..length(verbs(o))]
      try
        code = verb_code(o, v, 0, 1);
      except (ANY)
        code = {};
      endtry
      for n in [1..length(code)]
        if (regex ? match(code[n], pattern) | index(code[n], pattern))
          found = {@found, tostr(o, ":", verb_info(o, v)[3], " line ", n, ": ", code[n])};
          if (length(found) >= limit)
            return {@found, tostr("(stopped at ", limit, " matches)")};
          endif
        endif
      endfor
      this:pause();
    endfor
  endif
  this:pause();
endfor
return found || {"No matches"};
.

@verb $agent_tools:tool_add_verb this none this rxd
@program $agent_tools:tool_add_verb
set_task_perms(caller_perms());
{a} = args;
o = this:obj(this:arg(a, "obj"));
names = this:arg(a, "names");
spec = $string_utils:explode(this:arg(a, "args", "this none this"));
if (length(spec) != 3)
  raise(E_INVARG, "args must be three words: dobj prep iobj");
endif
add_verb(o, {player, this:arg(a, "perms", "rxd"), names}, spec);
return tostr("Added ", o, ":\"", names, "\" (verb ", length(verbs(o)), ")");
.

@verb $agent_tools:tool_program_verb this none this rxd
@program $agent_tools:tool_program_verb
set_task_perms(caller_perms());
{a} = args;
o = this:obj(this:arg(a, "obj"));
v = this:arg(a, "verb");
if (typeof(v) == STR && toint(v) > 0 && tostr(toint(v)) == v)
  v = toint(v);
endif
code = this:arg(a, "code");
if (typeof(code) != LIST || !this:all_strings(code))
  raise(E_INVARG, "code must be a list of lines");
endif
errors = set_verb_code(o, v, code);
if (errors)
  raise(E_INVARG, tostr("Verb not programmed: ", length(errors), " error(s)"), errors);
endif
return tostr("Programmed ", o, ":", v, " (", length(code), " lines)");
.

@verb $agent_tools:tool_delete_verb this none this rxd
@program $agent_tools:tool_delete_verb
set_task_perms(caller_perms());
{a} = args;
o = this:obj(this:arg(a, "obj"));
v = this:arg(a, "verb");
summary = this:describe_verb(o, v);
delete_verb(o, v);
return "Deleted " + summary;
.

@verb $agent_tools:tool_set_verb_info this none this rxd
@program $agent_tools:tool_set_verb_info
set_task_perms(caller_perms());
{a} = args;
o = this:obj(this:arg(a, "obj"));
v = this:arg(a, "verb");
"Arguments first: once the names change, v may no longer find the verb.";
if (maphaskey(a, "args"))
  spec = $string_utils:explode(a["args"]);
  if (length(spec) != 3)
    raise(E_INVARG, "args must be three words: dobj prep iobj");
  endif
  set_verb_args(o, v, spec);
endif
{owner, perms, names} = verb_info(o, v);
if (maphaskey(a, "owner"))
  owner = this:obj(a["owner"]);
endif
perms = this:arg(a, "perms", perms);
summary = this:describe_verb(o, v);
if (maphaskey(a, "names"))
  names = a["names"];
  summary = tostr(summary, "; renamed \"", names, "\"");
endif
set_verb_info(o, v, {owner, perms, names});
return maphaskey(a, "names") ? summary | this:describe_verb(o, v);
.

@verb $agent_tools:tool_create this none this rxd
@program $agent_tools:tool_create
set_task_perms(caller_perms());
{a} = args;
parent = this:obj(this:arg(a, "parent"));
"Through $recycler where the core has one, so quota and reuse work as with @create.";
if (this:has_recycler("_create"))
  o = $recycler:_create(parent);
else
  o = create(parent);
endif
o.name = this:arg(a, "name");
return tostr("Created ", o, " (", o.name, "), a child of ", parent);
.

@verb $agent_tools:tool_chparent this none this rxd
@program $agent_tools:tool_chparent
set_task_perms(caller_perms());
{a} = args;
o = this:obj(this:arg(a, "obj"));
p = this:obj(this:arg(a, "parent"));
chparent(o, p);
return tostr(o, " is now a child of ", p);
.

@verb $agent_tools:tool_recycle this none this rxd
@program $agent_tools:tool_recycle
set_task_perms(caller_perms());
{a} = args;
o = this:obj(this:arg(a, "obj"));
name = tostr(o, " (", o.name, ")");
if (this:has_recycler("_recycle"))
  $recycler:_recycle(o);
else
  recycle(o);
endif
return "Recycled " + name;
.

@verb $agent_tools:tool_object_info this none this rxd
@program $agent_tools:tool_object_info
set_task_perms(caller_perms());
{a} = args;
o = this:obj(this:arg(a, "obj"));
flags = {};
for f in ({"player", "programmer", "wizard", "r", "w", "f"})
  if (f == "player" ? is_player(o) | o.(f))
    flags = {@flags, f};
  endif
endfor
props = {};
for p in (properties(o))
  {owner, perms} = property_info(o, p);
  props = {@props, tostr(p, " (owner ", owner, ", perms ", perms, ")")};
endfor
return {tostr(o, " \"", o.name, "\""), tostr("parent ", parent(o), "; ancestors ", toliteral(ancestors(o))), tostr("owner ", o.owner, "; location ", o.location, "; flags ", toliteral(flags), "; ", length(children(o)), " children, ", length(verbs(o)), " verbs"), "properties defined here:", @props};
.

@verb $agent_tools:tool_get_property this none this rxd
@program $agent_tools:tool_get_property
set_task_perms(caller_perms());
{a} = args;
o = this:obj(this:arg(a, "obj"));
name = this:arg(a, "name");
value = o.(name);
try
  {owner, perms} = property_info(o, name);
  about = tostr("owner ", owner, ", perms ", perms);
except (E_PROPNF)
  about = "a built-in property";
endtry
return {about, toliteral(value)};
.

@verb $agent_tools:tool_set_property this none this rxd
@program $agent_tools:tool_set_property
set_task_perms(caller_perms());
{a} = args;
o = this:obj(this:arg(a, "obj"));
name = this:arg(a, "name");
parsed = eval("return " + this:arg(a, "value") + ";");
if (!parsed[1])
  raise(E_INVARG, "The value isn't a MOO literal", parsed[2]);
endif
value = parsed[2];
if (this:arg(a, "add", 0))
  add_property(o, name, value, {player, this:arg(a, "perms", "rc")});
else
  o.(name) = value;
endif
return tostr(o, ".", name, " = ", toliteral(value));
.

@verb $agent_tools:tool_eval this none this rxd
@program $agent_tools:tool_eval
set_task_perms(caller_perms());
{a} = args;
result = eval(this:arg(a, "code"));
if (!result[1])
  raise(E_INVARG, "It didn't compile", result[2]);
endif
return toliteral(result[2]);
.

;$agent_tools.dispatcher = FEATURE
;FEATURE:set_handler($agent_tools, {["package" -> "dns-com-familiar-agent", "min-version" -> "1.0", "max-version" -> "1.0"]})
