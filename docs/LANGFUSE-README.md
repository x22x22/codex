# Langfuse Integration Documentation

This directory contains comprehensive documentation for integrating Codex with [Langfuse](https://langfuse.com/), an open-source LLM observability platform.

## 📚 Documentation Index

### Quick Start (Start Here!)
- **[Quick Start Guide](./langfuse-quickstart.md)** (English) - Get started in 3 steps
- **[Integration Summary](./langfuse-integration-summary-cn.md)** (中文) - 完整分析总结

### Comprehensive Guides
- **[Complete Integration Guide](./langfuse-integration.md)** (中文) - 详细的集成说明
  - Langfuse introduction and features
  - Step-by-step configuration
  - Multiple deployment scenarios
  - Privacy considerations
  - Troubleshooting guide

### Technical Documentation
- **[Architecture Documentation](./langfuse-architecture.md)** - Visual diagrams and data flow
  - High-level architecture (Mermaid diagrams)
  - Component interactions (ASCII diagrams)
  - Data flow and event mappings
  - Security architecture
  - Deployment options

- **[Test Plan](./langfuse-test-plan.md)** - Comprehensive testing guide
  - 20 detailed test cases
  - Prerequisites and setup
  - Verification steps
  - Troubleshooting checklist
  - Test report template

### Configuration
- **[Example Configuration](./example-langfuse-config.toml)** - Complete config file
  - 5 deployment scenarios
  - Inline documentation
  - Authorization generation instructions

## 🚀 Quick Start

### 1. Get API Keys
Sign up at [cloud.langfuse.com](https://cloud.langfuse.com) and get your API keys.

### 2. Generate Authorization
Run the helper script:
```bash
# Unix/Linux/macOS
./scripts/generate-langfuse-auth.sh

# Windows
.\scripts\generate-langfuse-auth.ps1
```

### 3. Configure Codex
Add to `~/.codex/config.toml`:
```toml
[otel]
environment = "production"
exporter = "otlp-http"

[otel.exporter."otlp-http"]
endpoint = "https://cloud.langfuse.com/api/public/otel"
protocol = "binary"

[otel.exporter."otlp-http".headers]
"Authorization" = "Basic YOUR_BASE64_AUTH"
```

### 4. Verify
Run Codex and check traces at https://cloud.langfuse.com

## 📖 Documentation by Use Case

### For First-Time Users
1. Start with [Quick Start Guide](./langfuse-quickstart.md)
2. Review [Example Configuration](./example-langfuse-config.toml)
3. Follow the 3-step setup

### For Production Deployment
1. Read [Complete Integration Guide](./langfuse-integration.md)
2. Review [Architecture Documentation](./langfuse-architecture.md)
3. Consider self-hosting options
4. Run [Test Plan](./langfuse-test-plan.md) to verify

### For Troubleshooting
1. Check [Quick Start Guide - Troubleshooting](./langfuse-quickstart.md#troubleshooting)
2. Review [Integration Guide - 故障排除](./langfuse-integration.md#故障排除)
3. Consult [Test Plan](./langfuse-test-plan.md#troubleshooting-checklist)

### For Understanding the Architecture
1. Read [Architecture Documentation](./langfuse-architecture.md)
2. Review visual diagrams (Mermaid and ASCII)
3. Understand data flow and component interactions

## 🛠️ Helper Scripts

Located in `../scripts/`:

### Bash Script (Unix/Linux/macOS)
```bash
./scripts/generate-langfuse-auth.sh
```
- Interactive key collection
- Automatic base64 encoding
- Config snippet generation

### PowerShell Script (Windows)
```powershell
.\scripts\generate-langfuse-auth.ps1
```
- Same functionality as bash version
- Windows-friendly output
- Color-coded display

## 📊 What Gets Tracked?

Codex sends the following events to Langfuse:

| Event Type | Information Tracked |
|------------|-------------------|
| `conversation_starts` | Model, environment, approval policy |
| `api_request` | Duration, status code, retries |
| `sse_event` | Token usage (input, output, cached, reasoning) |
| `user_prompt` | Prompt length (optional: full text) |
| `tool_decision` | Tool name, approval status |
| `tool_result` | Execution time, success/failure, output |

## 🎯 Key Features

### Zero Code Changes
✅ Configuration-only integration
✅ Uses existing OpenTelemetry infrastructure
✅ No Rust code modifications needed

### LLM-Specific Features
✅ Cost tracking and analytics
✅ Prompt management and versioning
✅ Evaluation tools
✅ Dataset and experiment management

### Flexible Deployment
✅ Langfuse Cloud (EU/US regions)
✅ Self-hosted (Docker/Kubernetes)
✅ Custom TLS/mTLS support

### Privacy Control
✅ Configurable logging levels
✅ Self-hosting option
✅ Data retention policies

## 🔒 Security

### Authentication
- HTTP Basic Auth with base64-encoded API keys
- Environment variable support for credentials
- Optional mutual TLS (mTLS)

### Privacy
- User prompts redacted by default (`log_user_prompt = false`)
- Self-hosting option for complete data control
- Configurable metadata filtering

### Transport Security
- HTTPS encryption
- Custom CA certificate support
- Client certificate authentication

## 🌐 Deployment Options

### Cloud (Recommended for Quick Start)
```toml
endpoint = "https://cloud.langfuse.com/api/public/otel"  # EU
# endpoint = "https://us.cloud.langfuse.com/api/public/otel"  # US
```

### Self-Hosted (Docker Compose)
```bash
git clone https://github.com/langfuse/langfuse.git
cd langfuse
docker compose up
```

```toml
endpoint = "http://localhost:3000/api/public/otel"
```

### Self-Hosted (Kubernetes)
See [Langfuse Helm Charts](https://langfuse.com/docs/deployment/self-host/kubernetes)

## 📈 Verification Steps

1. **Run Codex**: `codex`
2. **Have a conversation**: Ask any question
3. **Check Langfuse**: Open https://cloud.langfuse.com
4. **View traces**: Navigate to "Traces" page
5. **Explore data**: Token usage, costs, timeline

## 🔍 Troubleshooting

### Common Issues

**No traces appearing?**
- Check `exporter = "otlp-http"` (not `"none"`)
- Verify Authorization header
- Confirm network connectivity

**Authentication error (401)?**
- Verify API keys are correct
- Check base64 encoding (no extra spaces)
- Confirm public + secret key pair

**Self-hosted issues?**
- Ensure Langfuse version >= v3.22.0
- Check endpoint URL
- Verify firewall rules

**Important:** Langfuse does not support gRPC. Always use `exporter = "otlp-http"`.

## 📝 Testing

Follow the comprehensive [Test Plan](./langfuse-test-plan.md):
- 20 test cases covering all functionality
- Step-by-step verification procedures
- Expected results for each test
- Test report template

## 🔗 External Resources

### Langfuse
- [Langfuse Website](https://langfuse.com/)
- [OpenTelemetry Integration](https://langfuse.com/docs/integrations/opentelemetry)
- [GitHub Repository](https://github.com/langfuse/langfuse)
- [Discord Community](https://discord.gg/7NXusRtqYU)

### OpenTelemetry
- [OpenTelemetry Specification](https://opentelemetry.io/docs/specs/)
- [OTLP Protocol](https://opentelemetry.io/docs/specs/otlp/)
- [GenAI Semantic Conventions](https://opentelemetry.io/docs/specs/semconv/gen-ai/)

## 💡 Tips

### Best Practices
- Start with Cloud for quick testing
- Use environment variables for credentials
- Enable `log_user_prompt` only when needed
- Set appropriate `environment` tags
- Run test plan before production

### Performance
- Async export (no blocking)
- Batch processing (efficient)
- Minimal overhead (< 5%)

### Privacy
- Default: prompts redacted
- Self-host for data control
- Configure retention policies

## 📊 Comparison with Alternatives

| Feature | Langfuse | Generic OTLP | Cloud Vendors |
|---------|----------|--------------|---------------|
| LLM-specific | ✅ Native | ❌ Custom | 🟡 Partial |
| Prompt management | ✅ Built-in | ❌ None | ❌ None |
| Cost tracking | ✅ Automatic | ❌ Custom | 🟡 Partial |
| Evaluation tools | ✅ Rich | ❌ None | 🟡 Limited |
| Self-hostable | ✅ Yes | ✅ Yes | ❌ No |
| Open source | ✅ MIT | ✅ Varies | ❌ Proprietary |
| Setup complexity | 🟢 Simple | 🟡 Medium | 🟢 Simple |

## 📅 Maintenance

### Documentation Version
- **Version:** 1.0
- **Last Updated:** 2025-12-26
- **Status:** Complete

### What's Included
- ✅ 9 documentation files
- ✅ 2 helper scripts (Bash + PowerShell)
- ✅ Visual diagrams (Mermaid + ASCII)
- ✅ 20 test cases
- ✅ Multiple language support (English + 中文)

### Future Enhancements (Optional)
- Automated testing scripts
- Langfuse-specific CLI commands
- Additional language translations
- Video tutorials

## 🤝 Contributing

Found an issue or have a suggestion?
- Open an issue in the [Codex repository](https://github.com/openai/codex/issues)
- Join the [Langfuse Discord](https://discord.gg/7NXusRtqYU)

## 📄 License

This documentation is part of the Codex project and follows the same license (Apache-2.0).

---

**Get Started Now:** Follow the [Quick Start Guide](./langfuse-quickstart.md) and have your Codex integrated with Langfuse in minutes!
