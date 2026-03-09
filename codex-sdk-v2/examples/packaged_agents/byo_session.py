from __future__ import annotations

import asyncio

from codex_sdk_v2 import Agent, LocalBackend, LocalBackendOptions, Manifest


async def main() -> None:
    backend = LocalBackend()
    manifest = Manifest(root="/workspace")
    session = await backend.create_session(
        manifest=manifest,
        options=LocalBackendOptions(),
    )

    agent = Agent(manifest=manifest, backend=backend)
    try:
        task = await agent.start(session=session)
        text = await task.collect_text("Reply with a short hello from the delegated Codex runtime.")
        print(text)
    finally:
        await task.close()


if __name__ == "__main__":
    asyncio.run(main())
