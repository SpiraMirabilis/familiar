# The MOO side of Familiar's agent tools

Familiar puts this folder here, and replaces it each time it starts an agent, so it always holds
the MOO code that this version of the client works with. Keep your own notes somewhere else.

A MOO gives an agent its own tools (`list_verb`, `program_verb`, `grep` and so on) once it has
two things installed:

1. **`mcp-feature/`**: MCP 2.1 for each player, as a feature object. It also gives players
   `@sedit`, which opens a verb in the client's editor.
2. **`familiar-agent/`**: `$agent_tools`, the tools themselves, served through the feature.

`agent-protocol.md` describes the messages between them and the client. Each folder's
README explains its part in full: read both before installing.

## Installing with send_command

Until the MOO has its own tools, `send_command` is how you work on it: it types commands as the
player and returns what the MOO says back. Installing needs a **wizard** character, and it
changes the live MOO for everyone, so ask the player before you start, and tell them what each
step will do.

- Check first what's there: `;$agent_tools` and `;player.features` say whether either part is
  installed already.
- The `.moo` files are commands to type, one per line, with `@program` blocks ending in a
  line `.`. Send a whole file with `send_command` and `file`, rather than retyping it:
  `{"file": "familiar-moo/mcp-feature/mcp-feature.moo", "timeout": 120}`. Look through
  what comes back for errors, and for the object numbers that `@create` reports.
- Some lines need changing first: `mcp-feature.moo` creates the feature from `#65`, the
  generic feature on LambdaCore (`;$feature` may tell you the right number), and
  `agent_tools.moo` and `mcp-feature-forwarder.moo` say `FEATURE` where the feature's object
  number goes. Copy the file elsewhere in your folder, change the copy, and send that.
- Whether the forwarder is needed depends on how the server treats `#$#` lines: step 2 of
  `mcp-feature/README.md` says how to tell.
- The player has to add the feature (`@add-feature mcpedit`) and reconnect, with MCP on for the
  world in Familiar. After that the MOO's tools show up in your tool list.
