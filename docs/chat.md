# Claude chat

The agent chat panel: its keys, subagent workers and how to configure which agent it starts.

[Back to README](../README.md)

## Using the chat

In the Claude chat (`Alt+2` opens it): `Alt+Up` / `Alt+Down` switch between open chats,
`Alt+I` (or `Tab` when no text box has focus) focuses the message input, `Alt+H` opens (or closes) the list of earlier conversations, `Esc` interrupts a running turn after a Yes/No confirmation (or closes the chat when idle; **Claude: Confirm Before Stopping** in Find Action turns the confirmation off, saved as `"confirm_stop": false` in `~/.config/zide/settings.json`),
`Ctrl+V` pastes an image (click one, in the message or above the input, to see it large; `Esc` closes it), and `/` and `@` open the command and file menus. Earlier conversations
are also listed by **Claude: Resume Session** in Find Action (`Ctrl+Shift+A`).
While Claude runs subagents, a **Workers** section at the bottom of the chat list shows each one
with what it is doing; click one to watch its messages and tool calls (read-only, nothing can be
sent to it), and `Esc` or **Back** returns to the chat. A worker leaves the list when it finishes.

## Configuring the agent

Zide starts `claude` from `PATH` by default. To start it some other way, for example through a
sandbox wrapper, run **Configure Agent** in Find Action, or edit `~/.config/zide/settings.json`:

```json
{
  "agent_servers": {
    "Sandboxed": {
      "command": "~/bin/agent-sandbox.sh",
      "args": ["claude"],
      "config_dir": "~/.sandbox-agent/.claude"
    },
    "Copilot": {
      "command": "copilot",
      "args": ["--acp", "--stdio"],
      "protocol": "acp"
    }
  },
  "agent": "Sandboxed"
}
```

By default Zide runs `command args…` followed by Claude's own arguments (it speaks Claude Code's
stream-json), so the command must end up running `claude`. With `"protocol": "acp"` it speaks the
[Agent Client Protocol](https://agentclientprotocol.com) instead, which agents such as Copilot
(`copilot --acp --stdio`) implement: the chat streams its answers, tool calls and permission
requests, and `Esc` cancels the turn. Claude's rate-limit and context gauges, model and effort,
earlier conversations and background agents are not offered for an ACP agent, and a chat whose
agent exits starts a new session on the next message. `config_dir` is where that
Claude keeps its sessions, when it is not `~/.claude` (a sandbox with its own home): the chat
history is read from there. `agent` names the one in use (without it, the first entry;
`null` is `claude` from `PATH`). The dialog lists the agents: pick one to edit it, **Use** it,
**Delete** it, or add a **New Agent**. New chats use the agent in use; open ones keep theirs. **Test** in the dialog runs `--version` through the command (the `initialize`
handshake for an ACP agent) and shows what failed. Uninstalling with `--purge` leaves `~/.config/zide` in place.

[Back to README](../README.md)
