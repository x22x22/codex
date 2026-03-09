from __future__ import annotations

import asyncio

from codex_sdk_v2 import Agent
from codex_sdk_v2 import ApproveDecision
from codex_sdk_v2 import DeferDecision
from codex_sdk_v2 import ExecCommand
from codex_sdk_v2 import FunctionTool
from codex_sdk_v2 import Manifest
from codex_sdk_v2 import PendingCommandExecution
from codex_sdk_v2 import ReplaceCommandDecision
from codex_sdk_v2 import WriteStdin


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
        taxpayer_id = call.arguments.get("taxpayer_id", "")
        if taxpayer_id.startswith("demo_"):
            return ApproveDecision()
        return DeferDecision()

    async def run(self, arguments):
        return f"Refund status for {arguments['taxpayer_id']}: approved"


async def approve_exec(tool_call: PendingCommandExecution):
    if tool_call.command and tool_call.command.startswith("ls"):
        return ApproveDecision()
    if tool_call.command and tool_call.command.startswith("cat"):
        return ReplaceCommandDecision(command=["sed", "-n", "1,20p", "README.md"])
    return DeferDecision()


async def stream_turn(task, start_text: str | None = None) -> None:
    events = task.run(start_text) if start_text is not None else task.resume()
    async for notification in events:
        if notification.method == "item/agentMessage/delta":
            delta = notification.params.get("delta")
            if isinstance(delta, str):
                print(delta, end="", flush=True)
    print()


async def main() -> None:
    agent = Agent(
        manifest=Manifest(),
        tools=(
            ExecCommand.with_approval_policy(policy=approve_exec),
            WriteStdin,
            LookupRefundStatus(),
        ),
    )
    task = await agent.start()
    await stream_turn(
        task,
        "List the current directory, then read README.md, then look up refund status for demo_123.",
    )
    while task.pending_tool_calls():
        for tool_call in task.pending_tool_calls():
            print(tool_call.describe())
            await tool_call(task)
        await stream_turn(task)


if __name__ == "__main__":
    asyncio.run(main())
