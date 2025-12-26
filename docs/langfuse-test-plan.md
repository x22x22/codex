# Langfuse Integration Test Plan

This document provides a comprehensive test plan for verifying Langfuse integration with Codex.

## Prerequisites

Before testing, ensure you have:

- [ ] Codex installed and working
- [ ] Langfuse account (Cloud or self-hosted instance)
- [ ] Langfuse API keys (Public Key + Secret Key)
- [ ] Network connectivity to Langfuse endpoint
- [ ] Basic understanding of OpenTelemetry concepts

## Test Environment Setup

### 1. Generate Authorization Header

**Using Bash (Linux/macOS):**
```bash
cd scripts
./generate-langfuse-auth.sh
```

**Using PowerShell (Windows):**
```powershell
cd scripts
.\generate-langfuse-auth.ps1
```

**Manual Generation:**
```bash
echo -n "pk-lf-YOUR-PUBLIC-KEY:sk-lf-YOUR-SECRET-KEY" | base64
```

### 2. Configure Codex

Create or update `~/.codex/config.toml`:

```toml
[otel]
environment = "test"
exporter = "otlp-http"
log_user_prompt = true  # Enable for testing

[otel.exporter."otlp-http"]
endpoint = "https://cloud.langfuse.com/api/public/otel"
protocol = "binary"

[otel.exporter."otlp-http".headers]
"Authorization" = "Basic YOUR_BASE64_AUTH_STRING"
```

### 3. Verify Configuration

Check that the configuration is valid:

```bash
# The config should be loaded without errors
codex --version
```

## Test Cases

### Test 1: Basic Connectivity

**Objective:** Verify that Codex can connect to Langfuse.

**Steps:**
1. Start Codex: `codex`
2. Type a simple prompt: "Hello, can you hear me?"
3. Wait for response
4. Exit Codex

**Expected Result:**
- No connection errors in Codex logs
- Events are batched and sent to Langfuse

**Verification:**
1. Open Langfuse UI
2. Navigate to "Traces" page
3. Look for a new trace with your conversation

**Status:** [ ] Pass [ ] Fail

**Notes:**
_____________________________________

---

### Test 2: Trace Creation

**Objective:** Verify that each Codex session creates a trace in Langfuse.

**Steps:**
1. Note the current time
2. Start Codex: `codex`
3. Have a simple conversation
4. Exit Codex
5. Check Langfuse UI within 1-2 minutes

**Expected Result:**
- A new trace appears in Langfuse
- Trace timestamp matches test start time
- Trace name indicates Codex session

**Verification:**
```
Langfuse UI → Traces → Filter by time → Find your trace
```

**Status:** [ ] Pass [ ] Fail

**Notes:**
_____________________________________

---

### Test 3: Conversation Start Event

**Objective:** Verify that `conversation_starts` event is captured.

**Steps:**
1. Start Codex with specific model: `codex --model gpt-5.1`
2. Check the first trace event

**Expected Result:**
Event should include:
- `event_type`: "codex.conversation_starts"
- `model`: "gpt-5.1"
- `provider_name`: Provider information
- `approval_policy`: Current policy
- `sandbox_policy`: Current policy
- `environment`: "test"

**Status:** [ ] Pass [ ] Fail

**Notes:**
_____________________________________

---

### Test 4: User Prompt Logging

**Objective:** Verify user prompts are logged (when enabled).

**Steps:**
1. Ensure `log_user_prompt = true` in config
2. Start Codex
3. Type: "Write a hello world in Python"
4. Wait for response
5. Check trace in Langfuse

**Expected Result:**
- Event type: "codex.user_prompt"
- Prompt text: "Write a hello world in Python"
- Prompt length: ~30 characters

**Status:** [ ] Pass [ ] Fail

**Notes:**
_____________________________________

---

### Test 5: API Request Tracking

**Objective:** Verify API requests to OpenAI are tracked.

**Steps:**
1. Start Codex
2. Give a prompt that requires API call
3. Wait for response
4. Check trace

**Expected Result:**
- Event type: "codex.api_request"
- Attributes include:
  - `duration_ms`: Response time
  - `http.response.status_code`: 200
  - `attempt`: Request attempt number

**Status:** [ ] Pass [ ] Fail

**Notes:**
_____________________________________

---

### Test 6: Token Usage Tracking

**Objective:** Verify token counts are captured.

**Steps:**
1. Start Codex
2. Ask a question: "Explain quantum computing in 50 words"
3. Wait for response
4. Check trace

**Expected Result:**
- Event type: "codex.sse_event"
- Token counts present:
  - `input_token_count`: > 0
  - `output_token_count`: > 0
  - `tool_token_count`: May be 0

**Status:** [ ] Pass [ ] Fail

**Notes:**
_____________________________________

---

### Test 7: Tool Decision Tracking

**Objective:** Verify tool calls are tracked.

**Steps:**
1. Start Codex
2. Give prompt requiring tool use: "Create a file named test.txt with 'Hello World'"
3. Approve the tool use
4. Check trace

**Expected Result:**
- Event type: "codex.tool_decision"
- Attributes:
  - `tool_name`: Name of tool used
  - `call_id`: Unique call identifier
  - `decision`: "approved" or similar
  - `source`: "user" or "config"

**Status:** [ ] Pass [ ] Fail

**Notes:**
_____________________________________

---

### Test 8: Tool Result Tracking

**Objective:** Verify tool execution results are captured.

**Steps:**
1. Continue from Test 7
2. Let the tool execute
3. Check trace

**Expected Result:**
- Event type: "codex.tool_result"
- Attributes:
  - `tool_name`: Same as decision
  - `duration_ms`: Execution time
  - `success`: "true"
  - `output`: Result of tool execution

**Status:** [ ] Pass [ ] Fail

**Notes:**
_____________________________________

---

### Test 9: Multiple Conversations

**Objective:** Verify multiple sessions create separate traces.

**Steps:**
1. Run Codex session 1: "What is 2+2?"
2. Exit
3. Run Codex session 2: "What is 3+3?"
4. Exit
5. Check Langfuse

**Expected Result:**
- Two separate traces appear
- Each has unique `conversation.id`
- Each contains their respective prompts

**Status:** [ ] Pass [ ] Fail

**Notes:**
_____________________________________

---

### Test 10: Privacy - Redacted Prompts

**Objective:** Verify prompts are redacted when disabled.

**Steps:**
1. Set `log_user_prompt = false` in config
2. Restart Codex
3. Type: "This is a secret message"
4. Check trace

**Expected Result:**
- Event type: "codex.user_prompt"
- `prompt`: Should be empty or "[REDACTED]"
- `prompt_length`: Should show character count

**Status:** [ ] Pass [ ] Fail

**Notes:**
_____________________________________

---

### Test 11: Cost Calculation

**Objective:** Verify Langfuse calculates costs correctly.

**Steps:**
1. Have a conversation with multiple API calls
2. Check trace in Langfuse
3. Look for cost information

**Expected Result:**
- Cost is calculated automatically
- Based on model used and token counts
- Displayed in USD

**Status:** [ ] Pass [ ] Fail

**Notes:**
_____________________________________

---

### Test 12: Session Metadata

**Objective:** Verify session metadata is present.

**Steps:**
1. Start Codex
2. Have any conversation
3. Check trace metadata in Langfuse

**Expected Result:**
Metadata should include:
- `conversation.id`: UUID
- `app.version`: Codex version
- `terminal.type`: Terminal info
- `auth_mode`: Auth method used
- `environment`: "test"

**Status:** [ ] Pass [ ] Fail

**Notes:**
_____________________________________

---

### Test 13: Error Handling

**Objective:** Verify errors are tracked properly.

**Steps:**
1. Temporarily set invalid API key for OpenAI
2. Try to use Codex
3. Check trace in Langfuse

**Expected Result:**
- Error events appear in trace
- `error.message`: Contains error description
- Status indicates failure

**Status:** [ ] Pass [ ] Fail

**Notes:**
_____________________________________

---

### Test 14: Self-Hosted Langfuse

**Objective:** Verify integration works with self-hosted instance.

**Prerequisites:**
- Running self-hosted Langfuse (Docker or K8s)

**Steps:**
1. Update config to point to self-hosted endpoint
   ```toml
   endpoint = "http://localhost:3000/api/public/otel"
   ```
2. Run test conversations
3. Check self-hosted UI

**Expected Result:**
- Same functionality as cloud version
- Traces appear in self-hosted UI

**Status:** [ ] Pass [ ] Fail [ ] N/A

**Notes:**
_____________________________________

---

### Test 15: TLS/SSL Configuration

**Objective:** Verify custom TLS configuration works.

**Prerequisites:**
- Self-hosted Langfuse with custom certificates

**Steps:**
1. Add TLS config:
   ```toml
   [otel.exporter."otlp-http".tls]
   ca-certificate = "path/to/ca.pem"
   ```
2. Test connection

**Expected Result:**
- Connection succeeds with custom CA
- No SSL errors in logs

**Status:** [ ] Pass [ ] Fail [ ] N/A

**Notes:**
_____________________________________

---

### Test 16: Environment Variables

**Objective:** Verify environment variable substitution works.

**Steps:**
1. Set environment variable:
   ```bash
   export LANGFUSE_AUTH="Basic YOUR_AUTH_STRING"
   ```
2. Update config:
   ```toml
   "Authorization" = "${LANGFUSE_AUTH}"
   ```
3. Test connection

**Expected Result:**
- Authentication succeeds
- No credential errors

**Status:** [ ] Pass [ ] Fail

**Notes:**
_____________________________________

---

### Test 17: JSON Protocol

**Objective:** Verify JSON protocol works (alternative to binary).

**Steps:**
1. Change protocol in config:
   ```toml
   protocol = "json"
   ```
2. Run test conversation
3. Check traces appear

**Expected Result:**
- Events sent as JSON
- Same functionality as binary protocol
- May be slower/larger payloads

**Status:** [ ] Pass [ ] Fail

**Notes:**
_____________________________________

---

### Test 18: Performance Impact

**Objective:** Measure performance impact of telemetry.

**Steps:**
1. Run conversation with telemetry disabled
   ```toml
   exporter = "none"
   ```
2. Time the session
3. Enable telemetry
4. Run identical conversation
5. Compare times

**Expected Result:**
- Minimal performance impact
- Difference should be < 5% in most cases
- Batching reduces overhead

**Status:** [ ] Pass [ ] Fail

**Notes:**
_____________________________________

---

### Test 19: Large Conversations

**Objective:** Verify large conversations are handled correctly.

**Steps:**
1. Have a long conversation (10+ exchanges)
2. Use various tools
3. Check complete trace in Langfuse

**Expected Result:**
- All events are captured
- Trace hierarchy is correct
- No missing events
- UI renders correctly

**Status:** [ ] Pass [ ] Fail

**Notes:**
_____________________________________

---

### Test 20: Connection Failures

**Objective:** Verify graceful handling of connection failures.

**Steps:**
1. Set invalid endpoint:
   ```toml
   endpoint = "https://invalid.example.com/otel"
   ```
2. Use Codex normally
3. Observe behavior

**Expected Result:**
- Codex continues to function
- Telemetry fails silently or logs error
- No crashes or hangs

**Status:** [ ] Pass [ ] Fail

**Notes:**
_____________________________________

---

## Test Summary

| Test # | Name | Status | Notes |
|--------|------|--------|-------|
| 1 | Basic Connectivity | [ ] | |
| 2 | Trace Creation | [ ] | |
| 3 | Conversation Start Event | [ ] | |
| 4 | User Prompt Logging | [ ] | |
| 5 | API Request Tracking | [ ] | |
| 6 | Token Usage Tracking | [ ] | |
| 7 | Tool Decision Tracking | [ ] | |
| 8 | Tool Result Tracking | [ ] | |
| 9 | Multiple Conversations | [ ] | |
| 10 | Privacy - Redacted Prompts | [ ] | |
| 11 | Cost Calculation | [ ] | |
| 12 | Session Metadata | [ ] | |
| 13 | Error Handling | [ ] | |
| 14 | Self-Hosted Langfuse | [ ] | |
| 15 | TLS/SSL Configuration | [ ] | |
| 16 | Environment Variables | [ ] | |
| 17 | JSON Protocol | [ ] | |
| 18 | Performance Impact | [ ] | |
| 19 | Large Conversations | [ ] | |
| 20 | Connection Failures | [ ] | |

**Total Tests:** 20
**Passed:** ___
**Failed:** ___
**Skipped:** ___

## Troubleshooting Checklist

If tests fail, verify:

- [ ] Langfuse API keys are correct
- [ ] Base64 encoding is correct (no extra spaces/newlines)
- [ ] Network connectivity to Langfuse endpoint
- [ ] Firewall allows HTTPS to Langfuse
- [ ] Config file syntax is valid TOML
- [ ] `exporter = "otlp-http"` (not "none")
- [ ] Endpoint URL is correct
- [ ] Self-hosted Langfuse version >= v3.22.0

## Logs and Debugging

### Enable Verbose Logging

```bash
# Set environment variable before running Codex
export RUST_LOG=codex_otel=debug
codex
```

### Check OTLP Export Logs

Look for lines like:
```
[codex_otel] Sending OTLP request to https://cloud.langfuse.com/api/public/otel
[codex_otel] OTLP response status: 200 OK
```

### Verify Events in Codex Logs

Check `~/.codex/logs/` for detailed event information.

## Next Steps

After completing tests:

1. **Document Issues:** Note any failing tests and error messages
2. **Review Configuration:** Double-check config for typos
3. **Check Langfuse Status:** Verify Langfuse service is operational
4. **Seek Help:** 
   - Codex: [GitHub Issues](https://github.com/openai/codex/issues)
   - Langfuse: [Discord](https://discord.gg/7NXusRtqYU)

## Test Report Template

```markdown
# Langfuse Integration Test Report

**Date:** YYYY-MM-DD
**Tester:** Your Name
**Environment:** 
- Codex Version: X.Y.Z
- Langfuse: Cloud / Self-Hosted vX.Y.Z
- OS: Linux / macOS / Windows

## Summary
- Total Tests: 20
- Passed: X
- Failed: Y
- Skipped: Z

## Failed Tests
1. Test #X: Reason for failure
2. Test #Y: Reason for failure

## Additional Notes
[Any other observations or issues]

## Conclusion
[ ] Integration works correctly
[ ] Minor issues (not blocking)
[ ] Major issues (requires fixes)
```

## Automated Testing Script

Future enhancement: Create automated test script

```bash
#!/bin/bash
# langfuse-integration-test.sh
# Automated test suite for Langfuse integration

# TODO: Implement automated tests
# - Check config validity
# - Start Codex with test prompts
# - Query Langfuse API to verify traces
# - Report results
```

## Continuous Monitoring

For production use, consider:
- Monitoring trace creation rate
- Alerting on failed exports
- Tracking cost trends
- Setting up dashboards in Langfuse
