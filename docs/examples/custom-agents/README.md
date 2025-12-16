# Custom Agent Examples

This directory contains example custom agent definitions that demonstrate how to create specialized subagents for different tasks.

## Available Examples

### code-reviewer.md
**Description**: Expert code reviewer focused on best practices and security

A comprehensive code reviewer that analyzes code for:
- Logic errors and bugs
- Security vulnerabilities
- Performance issues
- Code quality and maintainability

**Configuration**:
- Model: `claude-3-5-sonnet-20241022`
- Sandbox: `read-only`

**Use Cases**:
- Pre-commit code reviews
- Pull request analysis
- Refactoring guidance
- Learning from feedback

### documentation-writer.md
**Description**: Technical documentation specialist

An expert in creating clear, comprehensive technical documentation including:
- API documentation
- User guides and tutorials
- README files
- Architecture documentation

**Configuration**:
- Sandbox: `workspace-write` (needs to create/modify docs)

**Use Cases**:
- Writing new documentation
- Updating existing docs
- Creating tutorials
- Generating API docs

### test-engineer.md
**Description**: Test automation and quality assurance expert

Specializes in comprehensive test coverage and testing strategies:
- Unit tests
- Integration tests
- End-to-end tests
- Test-driven development

**Configuration**:
- Sandbox: `workspace-write` (needs to create test files)

**Use Cases**:
- Writing test cases
- Improving test coverage
- Test code reviews
- Testing strategy guidance

### security-auditor.md
**Description**: Security vulnerability assessment specialist

Focuses on identifying and mitigating security vulnerabilities:
- Common vulnerabilities (OWASP Top 10)
- Secure coding practices
- Language-specific security issues
- Dependency vulnerabilities

**Configuration**:
- Model: `claude-3-5-sonnet-20241022`
- Sandbox: `read-only`

**Use Cases**:
- Security code reviews
- Vulnerability assessments
- Security best practices guidance
- Compliance checking

## Installation

To use these examples:

1. Create the agents directory:
```bash
mkdir -p ~/.codex/agents
```

2. Copy the agents you want to use:
```bash
cp code-reviewer.md ~/.codex/agents/
cp documentation-writer.md ~/.codex/agents/
cp test-engineer.md ~/.codex/agents/
cp security-auditor.md ~/.codex/agents/
```

3. List available agents to verify installation:
```rust
use codex_protocol::protocol::Op;
conversation.submit(Op::ListCustomAgents).await?;
```

## Customization

These examples are templates that you can customize for your needs:

### Modifying Instructions

Edit the markdown body to change the agent's behavior:
```markdown
---
description: "Your custom description"
---

Your custom instructions here...
```

### Changing Models

Update the `model` field in frontmatter:
```markdown
---
model: "gpt-4"
---
```

### Adjusting Permissions

Change the `sandbox` policy:
- `read-only` - Can only read files
- `workspace-write` - Can write to workspace
- `danger-full-access` - Full system access (use carefully!)

## Creating Your Own Agents

Use these examples as templates:

1. **Start with a similar example** - Copy the one closest to your needs
2. **Define clear responsibilities** - What should the agent focus on?
3. **Provide structure** - How should responses be formatted?
4. **Set appropriate permissions** - What access does it need?
5. **Test thoroughly** - Try different scenarios

## Agent Design Patterns

### Read-Only Analyzer
For review, analysis, and advisory tasks:
```markdown
---
sandbox: "read-only"
---
You are an analyst who reviews and provides feedback...
```

### Code Generator
For tasks that need to create or modify files:
```markdown
---
sandbox: "workspace-write"
---
You are a code generator who creates implementations...
```

### Specialized Expert
For domain-specific tasks with a specific model:
```markdown
---
model: "claude-3-5-sonnet-20241022"
sandbox: "read-only"
---
You are an expert in [specific domain]...
```

## Best Practices

1. **Clear Purpose** - Each agent should have one main responsibility
2. **Specific Instructions** - Be explicit about what the agent should do
3. **Output Format** - Define how results should be structured
4. **Appropriate Model** - Choose models based on task complexity
5. **Minimal Permissions** - Use the most restrictive sandbox that works
6. **Documentation** - Explain the agent's purpose and usage

## Contributing

Have an idea for a useful agent? Consider:
- Does it serve a common use case?
- Is it well-documented?
- Does it follow best practices?
- Could others benefit from it?

## See Also

- [Custom Agents Documentation](../../custom-agents.md)
- [AGENTS.md Discovery](../../agents_md.md)
- [Configuration](../../config.md)
