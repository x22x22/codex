# codex-sdk-v2

`codex-sdk-v2` is an experimental Python prototype that borrows the host/runtime split from Universal Computer but uses `codex app-server` as the execution runtime.

Prototype shape:

- The host SDK owns workspace materialization, Codex process startup, and Responses API transport.
- A host bridge exposes `/v1/responses` to the locally running Codex runtime.
- Codex runs with `codex app-server --listen stdio://`.
- The SDK talks to app-server over stdio.
- Thread startup uses `thread/start.sdkDelegation` to point Codex at the host bridge.
- The prototype uses a local attached-process backend so it can run against the host-installed Codex binary without cross-compiling a Linux container binary.

Tool model:

- Built-in Codex tools are exposed as Python classes such as `ExecCommand`, `WriteStdin`, `ApplyPatch`, `ReadFile`, and `ViewImage`.
- The SDK sends those classes to app-server as an exact `thread/start.builtinTools` allowlist.
- This prototype assumes unified-exec for command execution, so shell access should be modeled with `ExecCommand` plus `WriteStdin`.
- Host-side custom tools subclass `FunctionTool`; the SDK registers them as dynamic tools internally and answers `item/tool/call` requests on the host.
- SDK users do not need to work with raw app-server `dynamicTools` payloads directly.

Example:

```python
from codex_sdk_v2 import Agent, ExecCommand, WriteStdin, FunctionTool, Manifest


class LookupRefundStatus(FunctionTool):
    name = "lookup_refund_status"
    description = "Return a canned refund status for a demo taxpayer id."
    input_schema = {
        "type": "object",
        "properties": {"taxpayer_id": {"type": "string"}},
        "required": ["taxpayer_id"],
        "additionalProperties": False,
    }

    async def run(self, arguments):
        return f"Refund status for {arguments['taxpayer_id']}: approved"


agent = Agent(
    manifest=Manifest(root="/workspace"),
    tools=(ExecCommand, WriteStdin, LookupRefundStatus()),
)
```

Current delegation shape:

1. The SDK starts a local HTTP bridge on the host.
2. `thread/start.sdkDelegation.bridgeUrl` tells Codex to use that host bridge as its Responses base URL for the thread.
3. Codex sends the raw Responses request body to the host bridge.
4. The host bridge adds the upstream `Authorization` header on the host side and forwards the request to OpenAI.
5. The bridge streams the upstream response back to Codex unchanged.

This means the prototype is bridge-based delegation, not the full event-by-event delegated Responses flow from the RFC yet.

Debugging:

- Set `CODEX_SDK_V2_DEBUG=1` to print JSON-RPC traffic and app-server stderr while running an example.
- The local backend prefers a repo-built `codex-rs/target/debug/codex-app-server` binary when present; otherwise it falls back to `codex` on your `PATH`.
