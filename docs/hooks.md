# Hooks

Codex supports custom hooks that let you run external scripts at key lifecycle events. Hooks receive structured JSON input and can influence Codex's behavior through their output.

## Overview

Hooks are configured in `~/.codex/hooks.json` (or any file named `hooks.json` in a Codex config layer). When an event fires, Codex locates all matching hooks, executes them in order, and acts on their outputs.

## Configuration

```json
{
  "hooks": {
    "SessionStart": [...],
    "Stop": [...],
    "AfterToolUse": [...]
  }
}
```

Each event key maps to an array of **matcher groups**. A matcher group is an object with an optional `matcher` regex and a list of hook handlers:

```json
{
  "matcher": "startup",
  "hooks": [
    {
      "type": "command",
      "command": "~/.codex/hooks/my_hook.sh",
      "timeout": 30,
      "statusMessage": "Running my hook..."
    }
  ]
}
```

### Matcher

The `matcher` field is an optional regular expression:

- **`SessionStart`**: matched against the session `source` string (`"startup"`, `"resume"`, or `"clear"`).
- **`AfterToolUse`**: matched against the `tool_name` string (e.g., `"local_shell"`, `"bash"`).
- **`Stop`**: matcher is ignored — Stop hooks always run.

When `matcher` is `null` or omitted, the hook runs for all events of that type.

### Hook handler fields

| Field | Type | Description |
|---|---|---|
| `type` | `"command"` | Handler type. Only `"command"` is supported. |
| `command` | string | Shell command to run. |
| `timeout` / `timeoutSec` | integer | Timeout in seconds (default: 600). |
| `statusMessage` | string | Message shown in the UI while the hook runs. |
| `async` | boolean | Not yet supported; hooks always run synchronously. |

## Feature flag

Hooks require the `codex_hooks` feature to be enabled in `~/.codex/config.toml`:

```toml
[features]
codex_hooks = true
```

## Hook execution

Each hook command is run via the shell configured in Codex. The hook receives a JSON object on **stdin** and must write a JSON object to **stdout** (or nothing, to take no action).

### Input JSON

All hooks receive a common base set of fields plus event-specific fields (see event sections below).

### Output JSON

All hooks share a common output format:

```json
{
  "continue": true,
  "stopReason": null,
  "suppressOutput": false,
  "systemMessage": null
}
```

| Field | Type | Default | Description |
|---|---|---|---|
| `continue` | boolean | `true` | Set to `false` to stop the operation. |
| `stopReason` | string \| null | `null` | Human-readable reason shown when stopping. |
| `suppressOutput` | boolean | `false` | Reserved for future use. |
| `systemMessage` | string \| null | `null` | Warning message to surface in the UI. |

If the hook exits with code `0` and produces no output, Codex continues normally.

## Events

### SessionStart

Fires before a new session begins or is resumed.

**Input:**

```json
{
  "hook_event_name": "SessionStart",
  "session_id": "uuid",
  "cwd": "/path/to/working/dir",
  "transcript_path": "/path/to/transcript.jsonl",
  "model": "codex-mini-latest",
  "permission_mode": "default",
  "source": "startup"
}
```

| Field | Description |
|---|---|
| `source` | One of `"startup"`, `"resume"`, `"clear"`. |
| `permission_mode` | One of `"default"`, `"acceptEdits"`, `"plan"`, `"dontAsk"`, `"bypassPermissions"`. |
| `transcript_path` | Path to the transcript file, or `null`. |

**Output:** Standard output fields, plus:

```json
{
  "continue": true,
  "hookSpecificOutput": {
    "hookEventName": "SessionStart",
    "additionalContext": "Text to inject into the model's context"
  }
}
```

`additionalContext` is injected into the model's input at the start of the session (only when `continue` is `true`).

**Plain-text output:** If the hook exits 0 and writes plain text (not starting with `{` or `[`), the text is treated as `additionalContext`.

**Example matcher:** Run only on session startup:

```json
{
  "matcher": "^startup$",
  "hooks": [{"type": "command", "command": "~/.codex/hooks/on_startup.sh"}]
}
```

---

### Stop

Fires when the agent finishes a turn (before the session ends or pauses for user input).

**Input:**

```json
{
  "hook_event_name": "Stop",
  "session_id": "uuid",
  "turn_id": "uuid",
  "cwd": "/path/to/working/dir",
  "transcript_path": "/path/to/transcript.jsonl",
  "model": "codex-mini-latest",
  "permission_mode": "default",
  "stop_hook_active": false,
  "last_assistant_message": "Done!"
}
```

| Field | Description |
|---|---|
| `stop_hook_active` | `true` if a previous stop hook already blocked this turn (prevents infinite loops). |
| `last_assistant_message` | The last message the assistant produced, or `null`. |

**Output:** Standard output fields, plus a decision to optionally block the stop:

```json
{
  "continue": true,
  "decision": "block",
  "reason": "Please run the tests first."
}
```

| Field | Description |
|---|---|
| `decision` | Set to `"block"` to ask the model for more work. |
| `reason` | Required when `decision` is `"block"`. Sent to the model as feedback. |

**Exit code 2:** Alternatively, exit with code `2` and write the feedback message to **stderr** to request more work:

```bash
echo "Please run the tests first." >&2
exit 2
```

**Note:** `stop_hook_active` is set to `true` on the second call within the same turn. A double-block is logged as a warning and ignored.

---

### AfterToolUse

Fires after any tool call completes (shell commands, function calls, MCP tools, etc.).

**Input:**

```json
{
  "hook_event_name": "AfterToolUse",
  "session_id": "uuid",
  "turn_id": "uuid",
  "call_id": "call_abc123",
  "tool_name": "local_shell",
  "cwd": "/path/to/working/dir",
  "transcript_path": "/path/to/transcript.jsonl",
  "model": "codex-mini-latest",
  "permission_mode": "default",
  "executed": true,
  "success": true,
  "duration_ms": 1234,
  "mutating": true,
  "sandbox": "workspace-write",
  "sandbox_policy": "workspace-write",
  "output_preview": "ok"
}
```

| Field | Description |
|---|---|
| `tool_name` | Name of the tool that was called. |
| `call_id` | Unique identifier for this tool call. |
| `executed` | Whether the tool was actually executed (vs. blocked by policy). |
| `success` | Whether the tool call succeeded. |
| `duration_ms` | Execution time in milliseconds. |
| `mutating` | Whether the tool may have mutated the environment. |
| `sandbox` | Active sandbox type (e.g., `"workspace-write"`, `"none"`). |
| `sandbox_policy` | Sandbox policy name. |
| `output_preview` | Truncated preview of the tool's output. |

**Output:** Standard output fields only. Setting `continue` to `false` stops the current operation with an error.

**Matcher example:** Run only for shell tool calls:

```json
{
  "hooks": {
    "AfterToolUse": [
      {
        "matcher": "local_shell",
        "hooks": [{"type": "command", "command": "~/.codex/hooks/after_shell.sh"}]
      },
      {
        "hooks": [{"type": "command", "command": "~/.codex/hooks/audit_log.sh"}]
      }
    ]
  }
}
```

## JSON schemas

Generated JSON schemas for hook inputs and outputs are available at:

- `codex-rs/hooks/schema/generated/session-start.command.input.schema.json`
- `codex-rs/hooks/schema/generated/session-start.command.output.schema.json`
- `codex-rs/hooks/schema/generated/stop.command.input.schema.json`
- `codex-rs/hooks/schema/generated/stop.command.output.schema.json`
- `codex-rs/hooks/schema/generated/after-tool-use.command.input.schema.json`
- `codex-rs/hooks/schema/generated/after-tool-use.command.output.schema.json`

## Example hook script

```bash
#!/usr/bin/env bash
# ~/.codex/hooks/session_start.sh
# Injects a context note at session start.

read -r JSON_INPUT

SOURCE=$(echo "$JSON_INPUT" | jq -r '.source')
MODEL=$(echo "$JSON_INPUT" | jq -r '.model')

printf '{"continue":true,"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"Session started via %s using model %s."}}\n' "$SOURCE" "$MODEL"
```

```bash
#!/usr/bin/env bash
# ~/.codex/hooks/after_tool_use.sh
# Logs every tool call to a file.

read -r JSON_INPUT

TOOL=$(echo "$JSON_INPUT" | jq -r '.tool_name')
SUCCESS=$(echo "$JSON_INPUT" | jq -r '.success')
echo "$(date -u +%FT%TZ) tool=$TOOL success=$SUCCESS" >> ~/.codex/tool_audit.log

# Exit 0 with no output = continue normally
```
