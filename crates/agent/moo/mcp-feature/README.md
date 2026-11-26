# MCP feature for MOO

A feature object that gives a MOO MCP 2.1 with no other MCP support needed. It runs each
player's session (the authentication key, negotiation, multiline messages, sending) and:

- handles `dns-org-mud-moo-simpleedit` itself. `@sedit #123:verb` (or `@sedit obj.property`)
  opens the verb or text in the client's editor, and sending it from the editor saves it.
  Compile errors come back as `Line N:  message`, which Familiar marks on the right lines.
- passes other packages to the objects registered with `:set_handler`. The
  [familiar-agent](../familiar-agent) tools are one.

It is written for Stunt (it uses maps) on a LambdaCore-style core whose generic feature calls
`:confunc()` when a player with the feature connects. The simple-edit verbs were compiled and
exercised on a Stunt server built from `~/stunt`, in emergency mode against `Minimal.db`. The
handler and negotiation verbs (`:set_handler`, `:_handlers`, `:_offers`, `:_package_of`,
`:supports`, `:_version`, `:send_to` and the routing in `:dispatch`) run on ToastStunt at
cybersphere, and the test script covers them, but it hasn't been run against them yet.

## Install

1. As a **wizard**, paste `mcp-feature.moo` into the MOO. It creates the feature from
   `#65` (change the first line if your generic feature has another number), adds the verbs and
   runs `@fix-commands`. It must be owned by a wizard: reading and saving use
   `set_task_perms(player)`, so everyone edits with their own permissions.
2. Check how your server treats `#$#` lines: type `#$#ping` in your MOO. "I don't understand
   that" (or your core's equivalent) means they reach the command parser; silence means they go
   out of band. Then:
   - If it was built with `OUT_OF_BAND_PREFIX "#$#"` (the default in `~/stunt/options.h`), those
     lines go to `#0:do_out_of_band_command`, not the command parser. Program that verb with
     `mcp-feature-forwarder.moo`, replacing `FEATURE` with the feature's number.
   - Otherwise the feature's `#$#*` command verb receives them and nothing else is needed.
   If `#0:do_out_of_band_command` already exists, add the forwarder's `if` block at its top
   rather than replacing it.
3. Each player who wants it: `@add-feature mcpedit`, then reconnect with an MCP client.

## Troubleshooting

- In Familiar, `/mcp` says whether MCP is running. "The client answered with its key, but the
  server hasn't replied" means the client's answer isn't reaching the feature: usually step 2.
  `/mcp debug` shows every MCP line in both directions.
- On the MOO, `;FEATURE.sessions` shows each player's key. An empty key (`"key" -> ""`) means
  the client's answer never arrived.
- Running `;me:confunc_features()` (or the feature's `:confunc`) again restarts MCP; the client
  answers again with a new key.

## Other packages

A package of its own lives on another object, which the feature hands its messages to:

```
;FEATURE:set_handler(HANDLER, {["package" -> "dns-example-thing", "min-version" -> "1.0", "max-version" -> "1.0"]})
```

- The feature offers the package in `mcp-negotiate-can`, from the next time a player connects.
- A message whose name is the package's, or starts with it and `-`, goes to
  `HANDLER:mcp_message(player, name, values)`. `values` is a map; a multiline value is a list
  of lines, under its key without the `*`.
- The handler answers with `FEATURE:send_to(player, name, values [, multiline values])`, which
  only the feature and its handlers may call. It sends nothing, and returns 0, unless the
  player's client negotiated the message's package, so a handler can send to anyone without
  checking first. `FEATURE:supports(player, package)` says whether it did.
- `;FEATURE:set_handler(HANDLER, {})` removes it. `;FEATURE.handle_map` lists them.

## How it works

- `:confunc` sends `#$#mcp version: 2.1 to: 2.1`. Clients without MCP show that one line.
- The client answers with an authentication key. `:handle_line` stores it per player, sends
  `mcp-negotiate-can` for `mcp-negotiate`, `dns-org-mud-moo-simpleedit` and each registered
  handler's packages, then
  `mcp-negotiate-end`. Messages without that player's key are ignored, so other players can't
  spoof them.
- The client's own `mcp-negotiate-can` lines are stored in the session, and `:supports` checks
  that the versions overlap. `:send_to` sends only messages of packages that both sides
  support (and `mcp-negotiate`, which goes out before the client's list arrives); `@sedit`
  says so if the client has no simpleedit.
- `:handle_line` also assembles multiline messages (`#$#* tag key: value`, ending `#$#: tag`),
  and hands each complete message to `:dispatch`, which passes it to its package's handler or
  acts on it here.
- `@sedit` sends `dns-org-mud-moo-simpleedit-content` (`type: moo-code` for verbs, `string` or
  `string-list` for properties). `:handle_set` saves what comes back with `set_verb_code()` or a
  property assignment and reports the result.
- `:disfunc` forgets a player's session, if your core calls it on disconnect. If not, the entry
  is just replaced when they next connect.

## Changing it

The verbs live in `verbs.py`. `python3 generate.py mcp-feature.moo test.txt` rebuilds
the install script, the forwarder and an emergency-mode test script. To run the test:

```sh
cp ~/stunt/Minimal.db /tmp/Minimal.db
moo -e /tmp/Minimal.db /tmp/out.db < test.txt
```
