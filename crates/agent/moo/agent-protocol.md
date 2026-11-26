# dns-com-familiar-agent 1.0

An MCP 2.1 (MUD Client Protocol) package that lets a MOO offer **tools** to an LLM agent, such
as Claude Code or Codex, that a player runs in their client. The client serves those tools to
the agent over the Model Context Protocol. The MOO defines the tools, so the client needs no
knowledge of any particular core. Every call is authenticated, delimited and correlated, which
means the client never has to parse a command's output.

The terminal client, `familiar-tui`, implements the client side (`/agent`). There is a
reference server side for LambdaCore-style cores with JSON built-in functions (ToastStunt) in
[`moo/familiar-agent`](../moo/familiar-agent), on the MCP feature in
[`moo/mcp-feature`](../moo/mcp-feature).

## Negotiation

The package is negotiated as usual: `mcp-negotiate-can package: dns-com-familiar-agent
min-version: 1.0 max-version: 1.0`, in both directions. Once both sides support it, the
client sends `-list` at `mcp-negotiate-end`.

## Messages

### Client → server: `dns-com-familiar-agent-list`

No arguments. It asks the server to send `-tools`.

### Server → client: `dns-com-familiar-agent-tools`

| Key | Value |
|---|---|
| `tools*` | Multiline. One JSON object per line, one line per tool. |

Each tool object has these fields:

| Field | |
|---|---|
| `name` | Required. Letters, digits, `_`, `-` and `.`. |
| `description` | What the tool does, for the agent. Write it for a model: say what the arguments mean and what comes back. |
| `input_schema` | A JSON Schema object for the arguments. `"type": "object"` is added if missing. |
| `show` | Optional. What the player's world window shows when the tool is called: `"none"`, `"summary"` (the default; one line naming the tool, its arguments and whether it worked) or `"full"` (the summary and everything the tool returned). |
| `timeout` | Optional. Seconds the client waits for the result before failing the call. Default 30. |

The server sends `-tools` in answer to `-list`. It may also send it at any other time, for
example after a tool is added or the player's permissions change. Each message replaces the
whole list. The client tells connected agents that the list changed
(`notifications/tools/list_changed`).

The client ignores a line it can't read, and tells the player.

### Client → server: `dns-com-familiar-agent-call`

| Key | Value |
|---|---|
| `id` | The call's number. It is unique for the connection. |
| `tool` | The tool's name. |
| `args*` | Multiline. The arguments, as one compact JSON object split into pieces of at most 1000 characters. **Join the lines with no separator**, then parse the result. A piece never ends with a space. |

The arguments are whatever the agent sent. Validate them in the tool.

### Server → client: `dns-com-familiar-agent-result`

| Key | Value |
|---|---|
| `id` | The `id` of the call this answers. |
| `ok` | `1` if the tool did what was asked, `0` if it failed. |
| `content*` | Multiline. The text for the agent, with the lines joined by newlines. It may be JSON. On failure, say why. The first line appears in the player's window. |

Answer every call exactly once. The client drops a result for an unknown or timed-out `id`.
Calls may be answered in any order.

## Client behaviour

- If the server hasn't answered within the tool's `timeout`, the client fails the call ("The
  world didn't answer within N seconds").
- When the connection drops, the client fails every pending call and forgets the tools.
- Calls travel on the player's own connection and act as the player. The server must run each
  tool with the player's permissions (`set_task_perms(player)`), never its own.
- `world_output` and `send_command` are the client's own tools, alongside the server's.
  `world_output` returns the last lines of the world's window. `send_command` types commands
  into the world and returns what the world prints back. Server tools with those names are
  hidden.

## Example

```
S: #$#mcp-negotiate-can 3f2a package: dns-com-familiar-agent min-version: 1.0 max-version: 1.0
C: #$#dns-com-familiar-agent-list 3f2a
S: #$#dns-com-familiar-agent-tools 3f2a tools*: "" _data-tag: 81
S: #$#* 81 tools: {"name":"list_verb","description":"The code of a verb.","input_schema":{"type":"object","properties":{"obj":{"type":"string"},"verb":{"type":"string"}},"required":["obj","verb"]}}
S: #$#: 81
C: #$#dns-com-familiar-agent-call 3f2a id: "1" tool: "list_verb" args*: "" _data-tag: 2f2a
C: #$#* 2f2a args: {"obj":"#1","verb":"look"}
C: #$#: 2f2a
S: #$#dns-com-familiar-agent-result 3f2a id: 1 ok: 1 content*: "" _data-tag: 82
S: #$#* 82 content: player:tell("Hello");
S: #$#: 82
```
