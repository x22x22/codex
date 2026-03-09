from __future__ import annotations

import asyncio
from dataclasses import dataclass, field
import os
from typing import Any

from .app_server_client import JsonRpcNotification, JsonRpcServerRequest
from .capabilities import Capability, DEFAULT_CAPABILITIES
from .bridge import OpenAIResponsesBridge
from .local_backend import LocalBackend, LocalBackendOptions, LocalSession
from .manifest import Manifest
from .task import Task
from .tools import Tool, builtin_tools, function_tools, tool_instruction_fragments


@dataclass(slots=True)
class Agent:
    manifest: Manifest
    model: str = "gpt-5.2-codex"
    # Replaces Codex's composed base instructions for the thread. When set, this
    # bypasses the Rust-side built-in capability prompt composition.
    base_instructions: str | None = None
    # Additive developer-role instructions for the thread. These are composed
    # together with capability- and FunctionTool-contributed instruction
    # fragments and sent via `thread/start.developerInstructions`.
    developer_instructions: str | None = None
    tools: tuple[Tool | type[Tool], ...] = field(default_factory=tuple)
    capabilities: tuple[Capability, ...] = field(default_factory=lambda: DEFAULT_CAPABILITIES)
    backend: LocalBackend = field(default_factory=LocalBackend)
    approval_policy: str | None = None

    async def start(
        self,
        *,
        backend_options: LocalBackendOptions | None = None,
        session: LocalSession | None = None,
    ) -> Task:
        api_key = os.environ.get("OPENAI_API_KEY")
        if not api_key:
            raise RuntimeError("OPENAI_API_KEY must be set for the prototype bridge")
        bridge = OpenAIResponsesBridge(api_key=api_key)
        bridge.start()

        manifest = self.manifest
        for capability in self.capabilities:
            manifest = capability.process_manifest(manifest)

        resolved_tools: tuple[Tool | type[Tool], ...] = (
            *(tool for capability in self.capabilities for tool in capability.tools()),
            *self.tools,
        )
        builtin_tool_names, builtin_tool_policies = builtin_tools(resolved_tools)
        resolved_function_tools = function_tools(resolved_tools)
        tool_fragments = tool_instruction_fragments(resolved_tools)
        capability_fragments = [
            fragment
            for capability in self.capabilities
            if (fragment := capability.instructions()) is not None
        ]
        dynamic_tools = [type(tool).dynamic_tool_spec() for tool in resolved_function_tools]
        function_tool_map = {
            type(tool).dynamic_tool_spec()["name"]: tool for tool in resolved_function_tools
        }
        developer_instructions = self.developer_instructions
        if capability_fragments or tool_fragments:
            sections = [
                fragment
                for fragment in [
                    developer_instructions,
                    *capability_fragments,
                    *tool_fragments,
                ]
                if fragment
            ]
            developer_instructions = "\n\n".join(sections) if sections else None

        if session is None:
            session = await self.backend.create_session(
                manifest=manifest,
                options=backend_options,
            )
        client = await session.start_app_server()
        await client.initialize(
            client_name="codex_sdk_v2",
            client_title="Codex SDK v2 Prototype",
            client_version="0.1.0",
        )
        approval_policy = self.approval_policy
        if approval_policy is None:
            approval_policy = "on-request" if builtin_tool_names else "never"
        thread_start_params: dict[str, Any] = {
            "model": self.model,
            "cwd": str(session.workspace_root),
            "sandbox": "danger-full-access",
            "approvalPolicy": approval_policy,
            "config": {
                "experimental_use_unified_exec_tool": True,
            },
            "baseInstructions": self.base_instructions,
            "developerInstructions": developer_instructions,
            "sdkDelegation": {
                "bridgeUrl": bridge.bridge_url,
            },
            "builtinTools": builtin_tool_names,
            "manualToolExecution": bool(builtin_tool_names),
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
        return Task(
            session=session,
            thread_id=result["thread"]["id"],
            initial_thread_started=thread_started_notification.params,
            function_tools=function_tool_map,
            builtin_tool_policies=builtin_tool_policies,
            _owned_bridge=bridge,
        )
