from __future__ import annotations

import asyncio
import os

from codex_sdk_v2 import Agent, LocalBackend, LocalBackendOptions, Manifest, OpenAIResponsesBridge


async def main() -> None:
    bridge = OpenAIResponsesBridge(api_key=os.environ["OPENAI_API_KEY"], bind_host="127.0.0.1")
    bridge.start()
    backend = LocalBackend()
    manifest = Manifest(root="/workspace")
    session = await backend.create_session(
        manifest=manifest,
        options=LocalBackendOptions(),
        delegation_bridge_url=bridge.bridge_url,
    )

    agent = Agent(manifest=manifest, backend=backend)
    try:
        task = await agent.start(session=session, bridge=bridge)
        text = await task.collect_text("Reply with a short hello from the delegated Codex runtime.")
        print(text)
    finally:
        await session.stop()
        bridge.shutdown()


if __name__ == "__main__":
    asyncio.run(main())
