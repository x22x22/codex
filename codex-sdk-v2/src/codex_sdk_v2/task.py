from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import Any, AsyncIterator, Mapping

from .app_server_client import JsonRpcNotification, JsonRpcServerRequest


@dataclass(slots=True)
class Task:
    session: Any
    thread_id: str
    initial_thread_started: dict[str, Any]
    prefix_user_instructions: str | None = None
    function_tools: Mapping[str, Any] = field(default_factory=dict)
    _owned_bridge: Any | None = None

    async def run(self, user_text: str) -> AsyncIterator[JsonRpcNotification]:
        if self.session.app_server_client is None:
            raise RuntimeError("app-server client is not attached")
        input_text = user_text
        if self.prefix_user_instructions:
            input_text = f"{self.prefix_user_instructions}\n\n{user_text}"
        response = await self.session.app_server_client.request(
            "turn/start",
            {
                "threadId": self.thread_id,
                "input": [{"type": "text", "text": input_text}],
            },
        )
        turn_id = response["turn"]["id"]
        while True:
            message = await self.session.app_server_client.next_message()
            if isinstance(message, JsonRpcServerRequest):
                await self._handle_server_request(message)
                continue
            yield message
            params = message.params
            matches_turn = params.get("turnId") == turn_id or params.get("turn", {}).get("id") == turn_id
            if message.method == "turn/completed" and matches_turn:
                break

    async def collect_text(self, user_text: str) -> str:
        text_chunks: list[str] = []
        async for notification in self.run(user_text):
            if notification.method == "item/agentMessage/delta":
                delta = notification.params.get("delta")
                if isinstance(delta, str):
                    text_chunks.append(delta)
        return "".join(text_chunks)

    async def _handle_server_request(self, request: JsonRpcServerRequest) -> None:
        if request.method != "item/tool/call":
            await self.session.app_server_client.send_error(
                request.request_id,
                -32601,
                f"unsupported server request method: {request.method}",
            )
            return

        tool_name = request.params.get("tool")
        arguments = request.params.get("arguments", {})
        if not isinstance(tool_name, str):
            await self.session.app_server_client.send_error(
                request.request_id,
                -32602,
                "tool call is missing a string tool name",
            )
            return
        if not isinstance(arguments, dict):
            await self.session.app_server_client.send_error(
                request.request_id,
                -32602,
                f"tool call arguments for {tool_name} must be an object",
            )
            return

        tool = self.function_tools.get(tool_name)
        if tool is None:
            await self.session.app_server_client.send_result(
                request.request_id,
                {
                    "contentItems": [
                        {
                            "type": "input_text",
                            "text": f"unknown function tool: {tool_name}",
                        }
                    ],
                    "success": False,
                },
            )
            return

        try:
            result = await tool.run(arguments)
        except Exception as exc:
            await self.session.app_server_client.send_result(
                request.request_id,
                {
                    "contentItems": [
                        {
                            "type": "input_text",
                            "text": f"function tool {tool_name} failed: {exc}",
                        }
                    ],
                    "success": False,
                },
            )
            return

        try:
            text = (
                result
                if isinstance(result, str)
                else json.dumps(result, indent=2, sort_keys=True)
            )
        except TypeError as exc:
            await self.session.app_server_client.send_result(
                request.request_id,
                {
                    "contentItems": [
                        {
                            "type": "input_text",
                            "text": (
                                f"function tool {tool_name} returned a non-serializable result: {exc}"
                            ),
                        }
                    ],
                    "success": False,
                },
            )
            return
        await self.session.app_server_client.send_result(
            request.request_id,
            {
                "contentItems": [{"type": "input_text", "text": text}],
                "success": True,
            },
        )
