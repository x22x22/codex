## Slash Commands

### What are slash commands?

Slash commands are special commands you can type that start with `/`.

---

### Built-in slash commands

Control Codex’s behavior during an interactive session with slash commands.

| Command      | Purpose                                                     |
| ------------ | ----------------------------------------------------------- |
| `/model`     | choose what model and reasoning effort to use               |
| `/approvals` | choose what Codex can do without approval                   |
| `/review`    | review my current changes and find issues                   |
| `/new`       | start a new chat during a conversation                      |
| `/resume`    | resume an old chat                                          |
| `/init`      | create an AGENTS.md file with instructions for Codex        |
| `/compact`   | summarize conversation to prevent hitting the context limit |
| `/undo`      | undo the last turn's file changes (requires `ghost_commit`) |
| `/diff`      | show git diff (including untracked files)                   |
| `/mention`   | mention a file                                              |
| `/status`    | show current session configuration and token usage          |
| `/mcp`       | list configured MCP tools                                   |
| `/logout`    | log out of Codex                                            |
| `/quit`      | exit Codex                                                  |
| `/exit`      | exit Codex                                                  |
| `/feedback`  | send logs to maintainers                                    |

---

### /undo

The `/undo` command restores your working directory to its state before the most recent turn. This is useful when Codex makes changes you want to discard.

**Prerequisites:**

- The `ghost_commit` feature must be enabled in your config:

  ```toml
  [features]
  ghost_commit = true
  ```

**How it works:**

When `ghost_commit` is enabled, Codex creates a snapshot (called a "ghost commit") of your working tree before each turn. The `/undo` command restores files to the most recent snapshot, effectively reversing all file changes made during the last turn.

**Limitations:**

- Only the most recent snapshot can be restored. There is no way to select from multiple historical snapshots or jump to an arbitrary point in the conversation.
- If no ghost snapshot is available (e.g., `ghost_commit` is disabled or you're at the start of a session), `/undo` will report that no snapshot is available.

---
