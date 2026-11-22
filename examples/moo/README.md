# MCP simple-edit feature for MOO

A feature object that gives a MOO MCP 2.1 and the `dns-org-mud-moo-simpleedit` package with no
other MCP support needed. With it, `@sedit #123:verb` (or `@sedit obj.property`) opens the verb
or text in the client's editor, and sending it from the editor saves it. Compile errors come
back as `Line N:  message`, which MU Client marks on the right lines.

It is written for Stunt (it uses maps) on a LambdaCore-style core whose generic feature calls
`:confunc()` when a player with the feature connects. All 12 verbs were compiled and exercised
on a Stunt server built from `~/stunt`, in emergency mode against `Minimal.db`.

## Install

1. As a **wizard**, paste `mcp-simpleedit-feature.moo` into the MOO. It creates the feature from
   `#65` (change the first line if your generic feature has another number), adds the verbs and
   runs `@fix-commands`. It must be owned by a wizard: reading and saving use
   `set_task_perms(player)`, so everyone edits with their own permissions.
2. Check how your server treats `#$#` lines: type `#$#ping` in your MOO. "I don't understand
   that" (or your core's equivalent) means they reach the command parser; silence means they go
   out of band. Then:
   - If it was built with `OUT_OF_BAND_PREFIX "#$#"` (the default in `~/stunt/options.h`), those
     lines go to `#0:do_out_of_band_command`, not the command parser. Program that verb with
     `mcp-simpleedit-feature-forwarder.moo`, replacing `FEATURE` with the feature's number.
   - Otherwise the feature's `#$#*` command verb receives them and nothing else is needed.
   If `#0:do_out_of_band_command` already exists, add the forwarder's `if` block at its top
   rather than replacing it.
3. Each player who wants it: `@add-feature mcpedit`, then reconnect with an MCP client.

## Troubleshooting

- In MU Client, `/mcp` says whether MCP is running. "The client answered with its key, but the
  server hasn't replied" means the client's answer isn't reaching the feature: usually step 2.
  `/mcp debug` shows every MCP line in both directions.
- On the MOO, `;FEATURE.sessions` shows each player's key. An empty key (`"key" -> ""`) means
  the client's answer never arrived.
- Running `;me:confunc_features()` (or the feature's `:confunc`) again restarts MCP; the client
  answers again with a new key.

## How it works

- `:confunc` sends `#$#mcp version: 2.1 to: 2.1`. Clients without MCP show that one line.
- The client answers with an authentication key. `:handle_line` stores it per player, sends
  `mcp-negotiate-can` for `mcp-negotiate` and `dns-org-mud-moo-simpleedit`, then
  `mcp-negotiate-end`. Messages without that player's key are ignored, so other players can't
  spoof them.
- `:handle_line` also assembles multiline messages (`#$#* tag key: value`, ending `#$#: tag`).
- `@sedit` sends `dns-org-mud-moo-simpleedit-content` (`type: moo-code` for verbs, `string` or
  `string-list` for properties). `:handle_set` saves what comes back with `set_verb_code()` or a
  property assignment and reports the result.
- `:disfunc` forgets a player's session, if your core calls it on disconnect. If not, the entry
  is just replaced when they next connect.

## Changing it

The verbs live in `verbs.py`. `python3 generate.py mcp-simpleedit-feature.moo test.txt` rebuilds
the install script, the forwarder and an emergency-mode test script. To run the test:

```sh
cp ~/stunt/Minimal.db /tmp/Minimal.db
moo -e /tmp/Minimal.db /tmp/out.db < test.txt
```
