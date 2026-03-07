from __future__ import annotations

import asyncio
from dataclasses import dataclass, field
import os
from typing import Any

from .app_server_client import JsonRpcNotification, JsonRpcServerRequest
from .bridge import OpenAIResponsesBridge
from .local_backend import LocalBackend, LocalBackendOptions, LocalSession
from .manifest import Manifest
from .task import Task
from .tools import DEFAULT_TOOLS, Tool, builtin_tool_names, function_tools


@dataclass(slots=True)
class Agent:
    manifest: Manifest
    model: str = "gpt-5.2-codex"
    base_instructions: str | None = None
    developer_instructions: str | None = None
    user_instructions: str | None = None
    tools: tuple[Tool | type[Tool], ...] = field(default_factory=lambda: DEFAULT_TOOLS)
    backend: LocalBackend = field(default_factory=LocalBackend)

    async def start(
        self,
        *,
        backend_options: LocalBackendOptions | None = None,
        session: LocalSession | None = None,
        bridge: OpenAIResponsesBridge | None = None,
    ) -> Task:
        own_bridge = False
        if bridge is None:
            api_key = os.environ.get("OPENAI_API_KEY")
            if not api_key:
                raise RuntimeError("OPENAI_API_KEY must be set for the prototype bridge")
            bridge = OpenAIResponsesBridge(api_key=api_key)
            bridge.start()
            own_bridge = True

        builtin_tools = builtin_tool_names(self.tools)
        resolved_function_tools = function_tools(self.tools)
        dynamic_tools = [type(tool).dynamic_tool_spec() for tool in resolved_function_tools]
        function_tool_map = {
            type(tool).dynamic_tool_spec()["name"]: tool for tool in resolved_function_tools
        }

        if session is None:
            session = await self.backend.create_session(
                manifest=self.manifest,
                options=backend_options,
                delegation_bridge_url=bridge.bridge_url,
            )
        client = await session.start_app_server()
        await client.initialize(
            client_name="codex_sdk_v2",
            client_title="Codex SDK v2 Prototype",
            client_version="0.1.0",
        )
        thread_start_params: dict[str, Any] = {
            "model": self.model,
            "cwd": str(session.workspace_root),
            "sandbox": "danger-full-access",
            "approvalPolicy": "never",
            "config": {
                "experimental_use_unified_exec_tool": True,
            },
            "baseInstructions": self.base_instructions,
            "developerInstructions": self.developer_instructions,
            "sdkDelegation": {
                "bridgeUrl": bridge.bridge_url,
            },
            "builtinTools": builtin_tools,
        }
        if dynamic_tools:
            thread_start_params["dynamicTools"] = dynamic_tools
        result = await client.request("thread/start", thread_start_params)
        thread_started_notification: JsonRpcNotification | None = None
        deferred_messages: list[JsonRpcNotification | JsonRpcServerRequest] = []
        seen_message_methods: list[str] = []
        while thread_started_notification is None:
            message = await asyncio.wait_for(client.next_message(), timeout=5)
            seen_message_methods.append(message.method)
            if isinstance(message, JsonRpcNotification) and message.method == "thread/started":
                thread_started_notification = message
            else:
                deferred_messages.append(message)

        delegation_notification: JsonRpcNotification | None = None
        while delegation_notification is None:
            try:
                message = await asyncio.wait_for(client.next_message(), timeout=5)
            except TimeoutError as exc:
                raise RuntimeError(
                    "did not receive codexSdk/delegationConfigured after thread/started; "
                    "if you intended to use the repo changes, make sure the example is launching "
                    "the locally built app-server binary instead of the installed Codex binary; "
                    f"seen={seen_message_methods}"
                ) from exc
            seen_message_methods.append(message.method)
            if isinstance(message, JsonRpcNotification) and message.method == "codexSdk/delegationConfigured":
                delegation_notification = message
            else:
                deferred_messages.append(message)
        client.prepend_messages(deferred_messages)
        task = Task(
            session=session,
            thread_id=result["thread"]["id"],
            initial_thread_started=thread_started_notification.params,
            prefix_user_instructions=self.user_instructions,
            function_tools=function_tool_map,
            _owned_bridge=bridge if own_bridge else None,
        )
        return task
