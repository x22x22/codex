from __future__ import annotations

import asyncio
import os
from pathlib import Path

from codex_sdk_v2 import Agent, Dir, LocalBackendOptions, LocalFile, Manifest

DATA_PATH = Path(__file__).resolve().parent / "data"
W2_PATH = DATA_PATH / "sample_w2.pdf"

INSTRUCTIONS = """
You are a federal tax filing agent. Compute year-end taxes and produce a filled Form 1040 for the current filing year using only the supplied files.
Save final outputs under the output directory in the workspace and provide a short summary of key amounts.

This is a demo. Assume:
1. filing status single
2. ssn 123-45-6789
3. dob 1991-01-01
4. no other income docs
5. if other info is needed, make up a test value
""".strip()


async def main() -> None:
    manifest = Manifest(
        root="/workspace",
        entries={
            "taxpayer_data": Dir(children={"w2.pdf": LocalFile(src=W2_PATH)}),
            "output": Dir(),
        },
    )
    agent = Agent(manifest=manifest, user_instructions=INSTRUCTIONS)
    task = await agent.start(backend_options=LocalBackendOptions())
    try:
        async for notification in task.run(
            "Please generate a 1040 for the current filing year using the supplied W-2 and save the result under the output directory."
        ):
            if notification.method == "item/agentMessage/delta":
                delta = notification.params.get("delta")
                if isinstance(delta, str):
                    print(delta, end="", flush=True)
        print()
    finally:
        bridge = getattr(task, "_owned_bridge", None)
        await task.session.stop()
        if bridge is not None:
            bridge.shutdown()


if __name__ == "__main__":
    if not os.environ.get("OPENAI_API_KEY"):
        raise RuntimeError("OPENAI_API_KEY must be set")
    asyncio.run(main())
