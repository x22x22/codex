from __future__ import annotations

import asyncio
import os

from codex_sdk_v2 import Agent, FunctionTool, LocalBackendOptions, Manifest


class LookupRefundStatus(FunctionTool):
    name = "lookup_refund_status"
    description = "Return a canned refund status for a demo taxpayer id."
    input_schema = {
        "type": "object",
        "properties": {
            "taxpayer_id": {"type": "string"},
        },
        "required": ["taxpayer_id"],
        "additionalProperties": False,
    }

    async def run(self, arguments: dict[str, object]) -> str:
        taxpayer_id = str(arguments["taxpayer_id"])
        return (
            f"Refund status for {taxpayer_id}: accepted, refund approved, "
            "expected deposit in 5 business days."
        )


async def main() -> None:
    agent = Agent(
        manifest=Manifest(root="/workspace"),
        tools=(LookupRefundStatus(),),
    )
    task = await agent.start(backend_options=LocalBackendOptions())
    try:
        text = await task.collect_text(
            "Use the available refund lookup tool for taxpayer id demo-123 and summarize the result."
        )
        print(text)
    finally:
        bridge = getattr(task, "_owned_bridge", None)
        await task.session.stop()
        if bridge is not None:
            bridge.shutdown()


if __name__ == "__main__":
    if not os.environ.get("OPENAI_API_KEY"):
        raise RuntimeError("OPENAI_API_KEY must be set")
    asyncio.run(main())
