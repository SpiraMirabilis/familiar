# familiar-agent: MOO tools for LLM agents

This is the server side of [dns-com-familiar-agent](../../docs/agent-protocol.md). It lets a
player who runs an LLM agent in Familiar's terminal client (`/agent claude`, `/agent codex`)
give that agent tools that work on the MOO, as that player.

It's written for LambdaCore-style cores on a server with maps and `parse_json` /
`generate_json`, which is to say ToastStunt. It also uses `ancestors()` and `descendants()`.

It needs the [MCP feature](../mcp-feature), which runs MCP 2.1 for each player and passes this
package's messages on. The feature doesn't need this package.

## Files

- **`agent_tools.moo`** creates `$agent_tools`:
  - The tool specs, in `:specs()`.
  - The dispatcher, `:call(who, tool, json)`, which returns `{ok, lines}`. It has nothing to do
    with MCP, so you can try it on its own first:
    `;$agent_tools:call(me, "list_verb", "{\"obj\":\"#1\",\"verb\":\"1\"}")`
  - One verb per tool, `tool_<name>`.
  - `:mcp_message(who, name, values)`, which the MCP feature calls with the package's messages:
    `-list` gets `-tools`, and `-call` runs the tool and answers with `-result`.
  - `:announce_tools([players])`, which sends a fresh tool list to those whose clients
    negotiated the package.

## Installing

1. Install the [MCP feature](../mcp-feature) and check that MCP works with it (`@sedit` opens
   an editor).
2. As a wizard, upload `agent_tools.moo`, for example with `/edit` in Familiar, pasting it in.
   Each line is a command, and the `@program` blocks end with `.`. First replace `FEATURE`, in
   the last two lines, with the feature's object number. Those lines tell `$agent_tools` where
   the feature is and register it with the feature, by `:set_handler`, for
   `dns-com-familiar-agent`.
3. Try a few tools with `;$agent_tools:call(...)` as above.
4. Reconnect with Familiar's terminal client, with MCP on for the world (the default), then run
   `/agent`. When you ask the agent to list its MCP tools, it should see the MOO's tools as
   well as the client's `world_output` and `send_command`.

The agent can do this installation itself, with `send_command`, as a wizard: Familiar puts a
copy of this folder and the MCP feature in its folder, under `familiar-moo/`.

If the agent sees only the client's tools, look at what the client sends: `/mcp debug` in
Familiar shows every MCP line. After `mcp-negotiate-end` the client sends
`#$#dns-com-familiar-agent-list` only if the server offered the package, which it does once
`:set_handler` has run and the player has reconnected (`;me:confunc_features()` or the
feature's `:confunc` restarts MCP without reconnecting).

## How it stays safe

- **Every tool runs with the player's permissions.**
  - `:mcp_message` forks a task for each call, calls `set_task_perms(player)`, and calls
    `$agent_tools:call`.
  - Each tool verb starts with `set_task_perms(caller_perms())`.
  - The verbs are wizard-owned only so that `set_task_perms` is allowed. Calling one directly
    gets you your own permissions, no more.
  - So an agent can do exactly what its player could do by typing, and nothing more.
- **Only the feature can pass messages on.** `:mcp_message` refuses any caller but
  `.dispatcher`, and the feature's `:dispatch` refuses any caller but the feature itself, so a
  message only arrives from the player's own client, with the session's key.
- **`eval` is off.** `;$agent_tools.allow_eval = 1` turns it on, and even then it's only
  offered to programmers. Run `;$agent_tools:announce_tools()` afterwards.
- **Results are authenticated.** They travel as MCP messages carrying the session's key, so
  another player can't forge one by printing text.

## When a call fails

A tool's error, including one raised inside `eval`, comes back to the agent as a failed result
with the error's message. If the tool raised a list of strings as the error's value (as
`program_verb` does with compiler errors), those lines follow. A task that runs out of ticks or
seconds can't be caught, though: the player sees the traceback and the agent sees the call time
out. Tools that take a long time should suspend now and then, as `grep` does with `:pause()`.

## Adding a tool

1. Add a spec to `:specs()`. A spec is a map with these keys:
   - `name`.
   - `description`: write it for a model, saying what the arguments are and what comes back.
   - `input_schema`: a JSON Schema as a MOO map.
   - Optionally `show`, which is `"none"`, `"summary"` or `"full"`, for what the player's
     window shows.
   - Optionally `timeout`, in seconds.
2. Add a verb `tool_<name>` (`this none this rxd`) that:
   - begins with `set_task_perms(caller_perms());`
   - takes the arguments as a map (`this:arg(a, "key" [, default])` reads one)
   - returns a string, a list of lines, or any value, which is sent as JSON.
   - fails with `raise(E_INVARG, "why")`. An error value that is a list of strings adds lines
     after the message.
3. Run `;$agent_tools:announce_tools()`. Clients that are connected then get the new list.

Code goes both ways as lists of lines: `program_verb` takes `code` as a JSON array of strings,
and `list_verb` returns one line per line of code. This is because MOO strings can't hold
newlines.
