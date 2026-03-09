from __future__ import annotations

import asyncio
import os
from pathlib import Path

from codex_sdk_v2 import Agent, Dir, LocalBackendOptions, LocalDir, Manifest, ReadFile, ListDir


async def main() -> None:
    examples_dir = Path(__file__).resolve().parents[2] / "examples"
    manifest = Manifest(
        root="/workspace",
        entries={
            "examples": LocalDir(src=examples_dir),
            "notes": Dir(),
        },
    )
    agent = Agent(manifest=manifest, tools=(ListDir, ReadFile))
    task = await agent.start(backend_options=LocalBackendOptions())
    try:
        async for notification in task.run(
            "List the top-level files under the examples directory, then read the workspace description if you can find one."
        ):
            if notification.method == "item/agentMessage/delta":
                delta = notification.params.get("delta")
                if isinstance(delta, str):
                    print(delta, end="", flush=True)
        print()
    finally:
        await task.close()


if __name__ == "__main__":
    if not os.environ.get("OPENAI_API_KEY"):
        raise RuntimeError("OPENAI_API_KEY must be set")
    asyncio.run(main())
