"""Builds the install script for a LambdaCore-style MOO and an emergency-mode test script.
Usage: python3 generate.py <install-script-out> <test-script-out>"""
import sys

from verbs import OOB_FORWARDER, VERBS

install_path, test_path = sys.argv[1], sys.argv[2]


def body(code):
    return code.strip("\n").split("\n")


# --- The install script, typed (or pasted) into the MOO as a wizard. ---
out = []
# Only commands: a line starting with " would be `say` on a LambdaCore MOO.
out += [
    "@create #65 named MCP,mcpedit",
    '@property mcpedit.sessions 0 ""',
    '@property mcpedit.handle_map 0 ""',
    '@set mcpedit.feature_verbs to {"@sedit"}',
]
for names, dobj, prep, iobj, code in VERBS:
    first = names.split()[0]
    quoted = f'"{names}"' if " " in names or "#" in names else names
    out.append(f"@verb mcpedit:{quoted} {dobj} {prep} {iobj} rxd")
    out.append(f"@program mcpedit:{first}")
    out += body(code)
    out.append(".")
out += ["@fix-commands mcpedit"]
with open(install_path.replace(".moo", "-forwarder.moo"), "w") as f:
    f.write("\n".join(body(OOB_FORWARDER)) + "\n")
with open(install_path, "w") as f:
    f.write("\n".join(out) + "\n")

# --- The emergency-mode test, against a scratch copy of Minimal.db. ---
F = "#4"
test = [
    ';"--- setup"',
    ";player",
    ";create(#1)",
    f';add_property({F}, "sessions", 0, {{player, ""}})',
    f';add_property({F}, "handle_map", 0, {{player, ""}})',
    f';add_property({F}, "note", "old text", {{player, "rw"}})',
    ';add_verb(player, {player, "rxd", "tell"}, {"this", "none", "this"})',
    "program #3:tell",
    "notify(this, tostr(@args));",
    ".",
    f';add_verb({F}, {{player, "rxd", "scratch"}}, {{"this", "none", "this"}})',
]
for names, dobj, prep, iobj, code in VERBS:
    first = names.split()[0]
    test.append(f';add_verb({F}, {{player, "rxd", "{names}"}}, {{"{dobj}", "{prep}", "{iobj}"}})')
    test.append(f"program {F}:{first}")
    test += body(code)
    test.append(".")
# A handler for another package, as moo/familiar-agent is one.
H = "#5"
test += [
    ";create(#1)",
    f';add_verb({H}, {{player, "rxd", "mcp_message"}}, {{"this", "none", "this"}})',
    f"program {H}:mcp_message",
    "{who, name, vals} = args;",
    'notify(who, tostr("handler got ", name, " ", toliteral(vals)));',
    f'sent = {F}:send_to(who, name + "-reply", ["n" -> "1"], ["lines" -> {{"a", 2}}, "more" -> {{"b"}}]);',
    'notify(who, tostr("reply sent: ", sent));',
    ".",
    f';{F}:set_handler({H}, {{["package" -> "test-pkg", "min-version" -> "1.0", "max-version" -> "1.0"], ["package" -> "test-other", "min-version" -> "1.0", "max-version" -> "1.0"]}})',
]
key = "k3y"


def oob(line):
    return f";{F}:handle_line({repr_moo(line)})"


def repr_moo(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


test += [
    ';"--- connect: the server starts MCP"',
    f";{F}:confunc()",
    ';"--- the client answers with its key; the server negotiates"',
    oob(f"#$#mcp authentication-key: {key} version: 2.1 to: 2.1"),
    oob(f'#$#mcp-negotiate-can {key} package: "dns-org-mud-moo-simpleedit" min-version: "1.0" max-version: "1.0"'),
    ';"--- the client offers test-pkg in 1.0-2.0, which overlaps; test-other it doesn\'t offer"',
    oob(f'#$#mcp-negotiate-can {key} package: test-pkg min-version: 1.0 max-version: 2.0'),
    oob(f"#$#mcp-negotiate-end {key}"),
    f';{{{F}:supports(player, "test-pkg"), {F}:supports(player, "test-other"), {F}:supports(player, "dns-org-mud-moo-simpleedit")}}',
    ';"--- a message for the handler\'s package goes to it, and its reply (one data tag, 2 as a literal) to the client"',
    oob(f"#$#test-pkg-ping {key} x: 1"),
    ';"--- a reply in a package the client didn\'t negotiate is not sent (reply sent: 0)"',
    oob(f"#$#test-other-ping {key} x: 1"),
    ';"--- a name that only starts the same way does not"',
    oob(f"#$#test-pkgs-ping {key} x: 1"),
    ';"--- dispatch refuses other callers (E_PERM); send_to sends nothing without a session (0)"',
    f';{F}:dispatch("test-pkg-ping", [])',
    f';{F}:send_to(#0, "x", [])',
    ';"--- @sedit a verb: its code goes to the client"',
    f';;argstr = "{F}:quote"; return {F}:("@sedit")()',
    ';"--- a message with the wrong key is ignored (returns 0)"',
    oob(f'#$#dns-org-mud-moo-simpleedit-set wrongkey reference: "{F}:scratch" type: "moo-code" content*: "" _data-tag: 1'),
    ';"--- sending back code with a syntax error"',
    oob(f'#$#dns-org-mud-moo-simpleedit-set {key} reference: "{F}:scratch" type: "moo-code" content*: "" _data-tag: 7'),
    oob("#$#* 7 content: x = ;"),
    oob('#$#* 7 content: return "unreached";'),
    oob("#$#: 7"),
    ';"--- sending back working code"',
    oob(f'#$#dns-org-mud-moo-simpleedit-set {key} reference: "{F}:scratch" type: "moo-code" content*: "" _data-tag: 8'),
    oob('#$#* 8 content: "Doubles 21.";'),
    oob("#$#* 8 content: x = 21;"),
    oob("#$#* 8 content: return x * 2;"),
    oob("#$#: 8"),
    f";{F}:scratch()",
    ';"--- @sedit a text property, then save it"',
    f';;argstr = "{F}.note"; return {F}:("@sedit")()',
    oob(f'#$#dns-org-mud-moo-simpleedit-set {key} reference: "{F}.note" type: "string" content*: "" _data-tag: 9'),
    oob('#$#* 9 content: new text with "quotes" and a \\ backslash'),
    oob("#$#: 9"),
    f";{F}.note",
    ';"--- errors"',
    f';;argstr = "{F}:nothing"; return {F}:("@sedit")()',
    f';;argstr = "nonsense"; return {F}:("@sedit")()',
]
test.append("abort")
with open(test_path, "w") as f:
    f.write("\n".join(test) + "\n")
print(f"{len(VERBS)} verbs")
