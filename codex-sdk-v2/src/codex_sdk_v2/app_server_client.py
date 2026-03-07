from __future__ import annotations

import asyncio
import json
from dataclasses import dataclass
import os
import sys
from typing import Any, TypeAlias


@dataclass(slots=True)
class JsonRpcNotification:
    method: str
    params: dict[str, Any]


@dataclass(slots=True)
class JsonRpcServerRequest:
    request_id: int | str
    method: str
    params: dict[str, Any]


IncomingMessage: TypeAlias = JsonRpcNotification | JsonRpcServerRequest


class AppServerClient:
    def __init__(self, process: asyncio.subprocess.Process) -> None:
        if process.stdout is None or process.stdin is None:
            raise RuntimeError("app-server process must be started with stdin/stdout pipes")
        self._process = process
        self._stdout = process.stdout
        self._stdin = process.stdin
        self._request_id = 0
        self._pending_messages: list[IncomingMessage] = []
        self._debug_enabled = os.environ.get("CODEX_SDK_V2_DEBUG") == "1"

    async def initialize(self, *, client_name: str, client_title: str, client_version: str) -> None:
        await self.request(
            "initialize",
            {
                "clientInfo": {
                    "name": client_name,
                    "title": client_title,
                    "version": client_version,
                },
                "capabilities": {"experimentalApi": True},
            },
        )
        await self.notify("initialized", {})

    async def notify(self, method: str, params: dict[str, Any]) -> None:
        await self._write({"method": method, "params": params})

    async def request(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        request_id = self._request_id
        self._request_id += 1
        await self._write({"id": request_id, "method": method, "params": params})
        while True:
            message = await self._read_message()
            if message.get("id") == request_id and "method" not in message:
                if "error" in message:
                    raise RuntimeError(f"app-server {method} failed: {message['error']}")
                return message["result"]
            queued = self._decode_incoming(message)
            if queued is not None:
                self._pending_messages.append(queued)

    async def send_result(self, request_id: int | str, result: dict[str, Any]) -> None:
        await self._write({"id": request_id, "result": result})

    async def send_error(self, request_id: int | str, code: int, message: str) -> None:
        await self._write(
            {
                "id": request_id,
                "error": {
                    "code": code,
                    "message": message,
                },
            }
        )

    async def next_message(self) -> IncomingMessage:
        if self._pending_messages:
            return self._pending_messages.pop(0)
        while True:
            message = await self._read_message()
            incoming = self._decode_incoming(message)
            if incoming is not None:
                return incoming

    async def next_notification(self) -> JsonRpcNotification:
        for index, pending in enumerate(self._pending_messages):
            if isinstance(pending, JsonRpcNotification):
                return self._pending_messages.pop(index)
        while True:
            message = await self._read_message()
            incoming = self._decode_incoming(message)
            if incoming is None:
                continue
            if isinstance(incoming, JsonRpcNotification):
                return incoming
            self._pending_messages.append(incoming)

    def prepend_messages(self, messages: list[IncomingMessage]) -> None:
        if messages:
            self._pending_messages = messages + self._pending_messages

    async def _write(self, payload: dict[str, Any]) -> None:
        if self._debug_enabled:
            print(f"[codex-sdk-v2] -> {payload}", file=sys.stderr)
        data = json.dumps(payload, separators=(",", ":")).encode("utf-8") + b"\n"
        self._stdin.write(data)
        await self._stdin.drain()

    async def _read_message(self) -> dict[str, Any]:
        line = await self._stdout.readline()
        if not line:
            raise RuntimeError("app-server closed the transport")
        message = json.loads(line.decode("utf-8"))
        if self._debug_enabled:
            print(f"[codex-sdk-v2] <- {message}", file=sys.stderr)
        return message

    def _decode_incoming(self, message: dict[str, Any]) -> IncomingMessage | None:
        method = message.get("method")
        if method is None:
            return None
        params = message.get("params", {})
        if "id" in message:
            return JsonRpcServerRequest(
                request_id=message["id"],
                method=method,
                params=params,
            )
        return JsonRpcNotification(method=method, params=params)
