# Langfuse Integration Quick Start

This guide provides a quick start for integrating Codex with [Langfuse](https://langfuse.com/), an open-source LLM observability platform.

> 📖 **For comprehensive documentation**, see [langfuse-integration.md](./langfuse-integration.md) (Chinese) or refer to the [Langfuse OpenTelemetry documentation](https://langfuse.com/docs/integrations/opentelemetry).

## What is Langfuse?

Langfuse is an open-source LLM engineering platform that helps teams:
- 📊 **Observe** LLM applications with detailed traces
- 💰 **Track costs** automatically based on token usage
- 🔍 **Evaluate** outputs with manual labeling or LLM-as-a-judge
- 🧪 **Experiment** with datasets and benchmarks
- 🎯 **Manage prompts** with versioning and collaboration

## Why Integrate with Codex?

Codex already supports OpenTelemetry (OTEL) for telemetry export. Langfuse provides an OTEL-compatible endpoint, allowing you to:
- Get LLM-specific insights without code changes
- View traces in a purpose-built UI for AI applications
- Analyze cost, performance, and quality metrics
- Self-host for complete data control

## Quick Setup (3 Steps)

### Step 1: Get Langfuse API Keys

**Option A - Langfuse Cloud (Recommended)**
1. Sign up at [cloud.langfuse.com](https://cloud.langfuse.com)
2. Create a project
3. Get your `Public Key` (pk-lf-...) and `Secret Key` (sk-lf-...)

**Option B - Self-Host**
```bash
git clone https://github.com/langfuse/langfuse.git
cd langfuse
docker compose up
```

### Step 2: Configure Codex

Add to `~/.codex/config.toml`:

```toml
[otel]
environment = "production"
exporter = "otlp-http"
log_user_prompt = false  # Set true to log full prompts

[otel.exporter."otlp-http"]
endpoint = "https://cloud.langfuse.com/api/public/otel"
protocol = "binary"

[otel.exporter."otlp-http".headers]
"Authorization" = "Basic YOUR_BASE64_AUTH"
```

**Generate YOUR_BASE64_AUTH:**
```bash
# Replace with your actual keys
echo -n "pk-lf-YOUR-PUBLIC-KEY:sk-lf-YOUR-SECRET-KEY" | base64
```

### Step 3: Verify

1. Run Codex: `codex`
2. Have a conversation with the agent
3. Open Langfuse UI and navigate to "Traces"
4. See your Codex sessions appear as traces!

## Configuration Options

### Langfuse Cloud (EU Region)
```toml
[otel.exporter."otlp-http"]
endpoint = "https://cloud.langfuse.com/api/public/otel"
protocol = "binary"

[otel.exporter."otlp-http".headers]
"Authorization" = "Basic YOUR_AUTH"
```

### Langfuse Cloud (US Region)
```toml
[otel.exporter."otlp-http"]
endpoint = "https://us.cloud.langfuse.com/api/public/otel"
protocol = "binary"

[otel.exporter."otlp-http".headers]
"Authorization" = "Basic YOUR_AUTH"
```

### Self-Hosted Langfuse
```toml
[otel.exporter."otlp-http"]
endpoint = "http://localhost:3000/api/public/otel"
protocol = "binary"

[otel.exporter."otlp-http".headers]
"Authorization" = "Basic YOUR_AUTH"
```

### Using Environment Variables (Recommended)
```toml
[otel.exporter."otlp-http"]
endpoint = "https://cloud.langfuse.com/api/public/otel"
protocol = "binary"

[otel.exporter."otlp-http".headers]
"Authorization" = "${LANGFUSE_AUTH}"
```

Then in your shell:
```bash
export LANGFUSE_AUTH="Basic $(echo -n 'pk-lf-...:sk-lf-...' | base64)"
```

## What Gets Tracked?

Codex sends these events to Langfuse:
- **Conversation starts** - Model, environment, approval policy
- **API requests** - Duration, status, retries
- **SSE events** - Token usage (input, output, cached, reasoning)
- **User prompts** - Length (full text if `log_user_prompt = true`)
- **Tool decisions** - Tool name, approval status
- **Tool results** - Execution time, success/failure, output

## Viewing Data in Langfuse

In the Langfuse UI, you'll see:
- **Traces**: Each Codex session as a trace
- **Timeline**: Sequence of events (API calls, tool executions)
- **Metadata**: Model, user ID, session ID, environment
- **Metrics**: Total tokens, cost (auto-calculated), duration
- **Hierarchy**: Parent-child relationships between operations

## Privacy Considerations

- By default, `log_user_prompt = false` hides prompt content
- Only metadata (length, timestamp) is logged
- Set to `true` if you need full prompts for debugging
- Self-host Langfuse for complete data control

## Troubleshooting

### No traces appearing?
- ✅ Check `exporter = "otlp-http"` (not `"none"`)
- ✅ Verify Authorization header is correct
- ✅ Confirm network connectivity to Langfuse
- ✅ Check Codex logs for errors

### Authentication error (401)?
- ✅ Verify API keys are correct
- ✅ Ensure base64 has no extra spaces/newlines
- ✅ Use correct public + secret key pair

### Self-hosted issues?
- ✅ Ensure Langfuse version >= v3.22.0
- ✅ Check endpoint URL is correct
- ✅ Verify firewall rules allow HTTPS

### Important: No gRPC Support
Langfuse **does not support** gRPC. Always use HTTP:
```toml
exporter = "otlp-http"  # ✅ Supported
# exporter = "otlp-grpc"  # ❌ Not supported by Langfuse
```

## Complete Example

See [example-langfuse-config.toml](./example-langfuse-config.toml) for a complete, commented configuration file.

## Advanced Features

### Cost Tracking
Langfuse automatically calculates costs based on:
- Model used
- Token counts (input/output)
- Current pricing for each model

### Prompt Management
- Version control your prompts in Langfuse
- Link prompts to traces
- A/B test different prompts

### Evaluations
- Manual scoring in the UI
- Custom evaluation pipelines via API
- LLM-as-a-judge scoring

### Datasets & Experiments
- Create test sets from production traces
- Run experiments comparing configurations
- Track improvements over time

## Resources

- 📚 [Full Integration Guide](./langfuse-integration.md) (Chinese)
- 📝 [Example Config](./example-langfuse-config.toml)
- 🌐 [Langfuse Documentation](https://langfuse.com/docs)
- 🔧 [Langfuse OpenTelemetry Guide](https://langfuse.com/docs/integrations/opentelemetry)
- 💬 [Langfuse Discord](https://discord.gg/7NXusRtqYU)
- 🐛 [Langfuse GitHub Issues](https://github.com/langfuse/langfuse/issues)

## Comparison with Alternatives

| Feature | Langfuse | Generic OTLP | Cloud Vendors |
|---------|----------|--------------|---------------|
| LLM-specific | ✅ Native | ❌ Custom needed | 🟡 Partial |
| Prompt management | ✅ Built-in | ❌ None | ❌ None |
| Cost tracking | ✅ Automatic | ❌ Custom needed | 🟡 Partial |
| Evaluation tools | ✅ Rich | ❌ None | 🟡 Limited |
| Self-hostable | ✅ Yes | ✅ Yes | ❌ No |
| Open source | ✅ MIT | ✅ Varies | ❌ Proprietary |
| Setup complexity | 🟢 Simple | 🟡 Medium | 🟢 Simple |

## Summary

**Key Benefits:**
- ✅ No code changes - configuration only
- ✅ Built on standard OpenTelemetry
- ✅ Purpose-built for LLM applications
- ✅ Open source with self-hosting option
- ✅ Rich features: cost tracking, evaluations, prompts

**Get Started Now:**
1. Get API keys from [cloud.langfuse.com](https://cloud.langfuse.com)
2. Add config to `~/.codex/config.toml`
3. Run Codex and view traces!

For questions or issues, refer to the [full documentation](./langfuse-integration.md) or open an issue.
