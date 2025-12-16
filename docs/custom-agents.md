# Custom Agents

Custom agents allow you to create specialized subagents with custom instructions, configured through markdown files. These agents can be invoked to perform specific tasks with tailored behavior, model selection, and sandbox policies.

## Overview

Custom agents are defined using markdown files placed in `$CODEX_HOME/agents/`. Each agent has:
- **Custom instructions** - Specialized prompts defining the agent's expertise and behavior
- **Model selection** - Optional override to use a specific model
- **Sandbox policy** - Security boundaries for the agent's operations
- **Description** - Short description shown in UI

## Creating a Custom Agent

### 1. Create the Agent File

Create a markdown file in `~/.codex/agents/` (or `$CODEX_HOME/agents/`):

```bash
mkdir -p ~/.codex/agents
```

### 2. Define the Agent

Create a file like `~/.codex/agents/code-reviewer.md`:

```markdown
---
description: "Expert code reviewer focused on best practices"
model: "claude-3-5-sonnet-20241022"
sandbox: "read-only"
---

You are an expert code reviewer with deep knowledge of software engineering best practices.

Your role is to:
1. Analyze code for bugs and security issues
2. Provide constructive feedback
3. Suggest specific improvements
...
```

### 3. Frontmatter Configuration

The YAML frontmatter at the top of the file supports these fields:

- **`description`** (optional) - Short description shown in UI
- **`model`** (optional) - Model to use for this agent (e.g., `"claude-3-5-sonnet-20241022"`)
- **`sandbox`** (optional) - Sandbox policy: `"read-only"`, `"workspace-write"`, or `"danger-full-access"`
  - Default is `"read-only"` if not specified

### 4. Agent Instructions

The markdown body after the frontmatter contains the agent's system prompt. This defines:
- The agent's expertise and role
- How it should behave
- What tasks it should focus on
- Its response format and style

## Using Custom Agents

### Programmatic Usage

Custom agents can be invoked via the protocol:

```rust
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;

// Run a custom agent
let op = Op::RunCustomAgent {
    agent_name: "code-reviewer".to_string(),
    items: vec![
        UserInput::Text {
            text: "Review the recent changes in src/main.rs".to_string()
        }
    ],
};

conversation.submit(op).await?;
```

### Listing Available Agents

Get a list of all available custom agents:

```rust
use codex_protocol::protocol::Op;

let op = Op::ListCustomAgents;
conversation.submit(op).await?;

// Listen for ListCustomAgentsResponse event
```

## Agent Design Best Practices

### 1. Be Specific

Define clear responsibilities and scope:

```markdown
You are a security auditor specializing in identifying vulnerabilities.
Focus on: SQL injection, XSS, authentication flaws, and data exposure.
```

### 2. Provide Structure

Give the agent a clear response format:

```markdown
For each issue found:
- **Severity**: Critical/High/Medium/Low
- **Description**: What is the problem?
- **Location**: Where is it in the code?
- **Recommendation**: How to fix it?
```

### 3. Set Constraints

Define what the agent should and shouldn't do:

```markdown
Do:
- Prioritize security issues over style issues
- Provide actionable recommendations
- Reference industry standards (OWASP, CWE)

Don't:
- Make assumptions about business logic without asking
- Suggest changes to unrelated code
```

### 4. Choose Appropriate Sandbox Policy

- **`read-only`** - Agent can only read files, safe for analysis and review tasks
- **`workspace-write`** - Agent can write to the workspace, needed for code generation
- **`danger-full-access`** - Full system access (use with extreme caution)

## Example Agents

See the [examples/custom-agents/](examples/custom-agents/) directory for complete example agents:

- **code-reviewer** - Code review specialist
- **documentation-writer** - Technical documentation expert
- **test-engineer** - Test automation specialist
- **security-auditor** - Security vulnerability assessment

## Architecture

Custom agents work similarly to the built-in `/review` feature:

1. Agent definitions are discovered from `$CODEX_HOME/agents/`
2. When invoked, a subagent conversation is created
3. The subagent runs with the custom instructions and configuration
4. Events from the subagent are forwarded to the parent session
5. The subagent completes when its task is done

## File Discovery

Custom agents are discovered by:
1. Looking in `$CODEX_HOME/agents/` (default: `~/.codex/agents/`)
2. Reading all `.md` files in that directory
3. Parsing frontmatter for configuration
4. Using the filename (without `.md`) as the agent name

Only `.md` files are recognized. Subdirectories are not currently scanned.

## Limitations

- Custom agents inherit most configuration from the parent session
- Project documentation (`AGENTS.md`) is not loaded for custom agents
- Feature flags from the parent session apply to custom agents
- Network access depends on sandbox policy

## Security Considerations

When creating custom agents:

1. **Use appropriate sandbox policies** - Default to `read-only` unless write access is needed
2. **Review agent instructions** - They define the agent's behavior and capabilities
3. **Model selection** - More capable models may be needed for complex tasks
4. **Sensitive operations** - Consider what data the agent has access to

## Troubleshooting

### Agent Not Found

If `RunCustomAgent` returns an error that the agent is not found:
- Check that the file exists in `$CODEX_HOME/agents/`
- Verify the file has a `.md` extension
- Ensure the agent name matches the filename without `.md`
- Check that `$CODEX_HOME` is set correctly (default: `~/.codex`)

### Agent Behaves Unexpectedly

- Review the agent's instructions in the markdown file
- Check if sandbox policy is appropriate for the task
- Verify the model is suitable for the complexity of the task
- Ensure frontmatter YAML is valid

### Listing Shows No Agents

- Check that `$CODEX_HOME/agents/` directory exists
- Verify there are `.md` files in the directory
- Check file permissions allow reading

## Future Enhancements

Potential future improvements:
- UI integration in TUI and app-server
- Agent templates and scaffolding
- Agent composition (agents using other agents)
- Dynamic agent discovery and hot-reloading
- Agent marketplace or sharing mechanism
