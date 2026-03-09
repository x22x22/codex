# codex-sdk-v2

`codex-sdk-v2` is an experimental Python prototype that borrows the host/runtime split from Universal Computer but uses `codex app-server` as the execution runtime.

Prototype shape:

- The host SDK owns workspace materialization, Codex process startup, and Responses API transport.
- A host bridge exposes `/v1/responses` to the locally running Codex runtime.
- Codex runs with `codex app-server --listen stdio://`.
- The SDK talks to app-server over stdio.
- Thread startup uses `thread/start.sdkDelegation` to point Codex at the host bridge.
- The SDK owns the bridge lifecycle and `await task.close()` tears down both the app-server session and the bridge.
- The prototype uses a local attached-process backend so it can run against the host-installed Codex binary without cross-compiling a Linux container binary.

Capability model:

- Capabilities are the SDK’s grouping abstraction for UC-style bundles.
- A `Capability` can contribute:
  - `tools()`
  - `instructions()`
  - `process_manifest(manifest)`
- The capability API intentionally uses a single `tools()` method; the built-in vs function-tool split stays internal to the SDK runtime.
- The default capability set is `UnifiedExecCapability()`, which enables `ExecCommand` and `WriteStdin`.

Tool model:

- Built-in Codex tools are exposed as Python classes such as `ExecCommand`, `WriteStdin`, `ApplyPatch`, `ReadFile`, and `ViewImage`.
- The SDK sends those classes to app-server as an exact `thread/start.builtinTools` allowlist.
- Defaults come from `UnifiedExecCapability()`, which enables `ExecCommand` plus `WriteStdin`.
- Host-side custom tools subclass `FunctionTool`; the SDK registers them as dynamic tools internally and answers `item/tool/call` requests on the host.
- SDK users do not need to work with raw app-server `dynamicTools` payloads directly.
- Custom `FunctionTool`s can contribute instruction fragments; the SDK folds those fragments into `developerInstructions`.
- Built-in tool instructions are owned by Codex itself and are composed in Rust from the enabled built-in capability set.

Example capability:

```python
from codex_sdk_v2 import Capability, ExecCommand, WriteStdin


class UnifiedExec(Capability):
    def tools(self):
        return (ExecCommand, WriteStdin)
```

Pending tool call API:

- `task.pending_tool_calls()` returns unresolved tool calls.
- Each pending tool call supports `describe()` and `await tool_call(task)`.
- The pending tool call subclasses are:
  - `PendingCommandExecution`
  - `PendingFileChange`
  - `PendingFunctionToolCall`
- The explicit host helpers are:
  - `task.approve(...)`
  - `task.reject(...)`
  - `task.replace_command(...)`
  - `task.run_function_tool(...)`
  - `task.submit_tool_result(...)`

Decision model:

- `ApproveDecision()`
- `RejectDecision()`
- `DeferDecision()`
- `ReplaceCommandDecision(command=[...])`
- `RunDecision(arguments=...)`
- `RespondDecision(result=...)`

Approval model:

- Manual is the default.
- If a tool does not make a decision, its call stays pending in `task.pending_tool_calls()`.
- `FunctionTool.approve(call)` can resolve or defer a function tool call.
- `BuiltinTool.with_approval_policy(policy=...)` can resolve or defer a built-in call.
- There is no agent-wide global approval policy in the prototype.

Example:

```python
from codex_sdk_v2 import Agent, ApproveDecision, DeferDecision
from codex_sdk_v2 import ExecCommand, FunctionTool, Manifest
from codex_sdk_v2 import PendingCommandExecution, ReplaceCommandDecision, WriteStdin


class LookupRefundStatus(FunctionTool):
    name = "lookup_refund_status"
    description = "Return a canned refund status for a demo taxpayer id."
    input_schema = {
        "type": "object",
        "properties": {"taxpayer_id": {"type": "string"}},
        "required": ["taxpayer_id"],
        "additionalProperties": False,
    }

    async def approve(self, call):
        if call.arguments["taxpayer_id"].startswith("demo_"):
            return ApproveDecision()
        return DeferDecision()

    async def run(self, arguments):
        return f"Refund status for {arguments['taxpayer_id']}: approved"


async def approve_exec(call: PendingCommandExecution):
    if call.command and call.command.startswith("ls"):
        return ApproveDecision()
    if call.command and call.command.startswith("cat"):
        return ReplaceCommandDecision(command=["sed", "-n", "1,20p", "README.md"])
    return DeferDecision()


agent = Agent(
    manifest=Manifest(root="/workspace"),
    tools=(
        ExecCommand.with_approval_policy(policy=approve_exec),
        WriteStdin,
        LookupRefundStatus(),
    ),
)

task = await agent.start()
await stream_turn(task, start_text="Help me with my taxes")
while task.pending_tool_calls():
    for tool_call in task.pending_tool_calls():
        print(tool_call.describe())
        await tool_call(task)
    await stream_turn(task)
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

Current limitation:

- The UC-style pending-tool-call flow is now present in-memory on the SDK task object. Persisting unresolved tool calls cleanly across a full host process restart still depends on replay behavior from app-server for the underlying pending request type.
