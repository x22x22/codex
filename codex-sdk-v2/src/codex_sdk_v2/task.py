from __future__ import annotations

import inspect
import json
from dataclasses import dataclass, field
from typing import Any, AsyncIterator, Mapping

from .app_server_client import JsonRpcNotification, JsonRpcServerRequest
from .pending_tool_calls import ApproveDecision
from .pending_tool_calls import DeferDecision
from .pending_tool_calls import PendingCommandExecution
from .pending_tool_calls import PendingFileChange
from .pending_tool_calls import PendingFunctionToolCall
from .pending_tool_calls import PendingToolCall
from .pending_tool_calls import RejectDecision
from .pending_tool_calls import ReplaceCommandDecision
from .pending_tool_calls import RespondDecision
from .pending_tool_calls import RunDecision
from .pending_tool_calls import ToolDecision


@dataclass(slots=True)
class Task:
    session: Any
    thread_id: str
    initial_thread_started: dict[str, Any]
    function_tools: Mapping[str, Any] = field(default_factory=dict)
    builtin_tool_policies: Mapping[str, Any] = field(default_factory=dict)
    _owned_bridge: Any | None = None
    _pending_tool_calls: list[PendingToolCall] = field(default_factory=list)
    _active_turn_id: str | None = None
    _turn_complete: bool = True

    async def close(self) -> None:
        try:
            await self.session.stop()
        finally:
            if self._owned_bridge is not None:
                self._owned_bridge.shutdown()
                self._owned_bridge = None

    async def run(self, user_text: str) -> AsyncIterator[JsonRpcNotification]:
        if self.session.app_server_client is None:
            raise RuntimeError("app-server client is not attached")
        if self.pending_tool_calls():
            raise RuntimeError("cannot start a new turn while tool calls are pending")
        response = await self.session.app_server_client.request(
            "turn/start",
            {
                "threadId": self.thread_id,
                "input": [{"type": "text", "text": user_text}],
            },
        )
        self._active_turn_id = response["turn"]["id"]
        self._turn_complete = False
        while True:
            message = await self.session.app_server_client.next_message()
            if isinstance(message, JsonRpcServerRequest):
                should_pause = await self._handle_server_request(message)
                if should_pause:
                    return
                continue
            yield message
            params = message.params
            matches_turn = params.get("turnId") == self._active_turn_id or params.get("turn", {}).get("id") == self._active_turn_id
            if message.method == "turn/completed" and matches_turn:
                self._turn_complete = True
                self._active_turn_id = None
                return

    async def resume(self) -> AsyncIterator[JsonRpcNotification]:
        if self.session.app_server_client is None:
            raise RuntimeError("app-server client is not attached")
        if self.pending_tool_calls():
            raise RuntimeError("cannot resume while tool calls are pending")
        if self._turn_complete or self._active_turn_id is None:
            return
        while True:
            message = await self.session.app_server_client.next_message()
            if isinstance(message, JsonRpcServerRequest):
                should_pause = await self._handle_server_request(message)
                if should_pause:
                    return
                continue
            yield message
            params = message.params
            matches_turn = params.get("turnId") == self._active_turn_id or params.get("turn", {}).get("id") == self._active_turn_id
            if message.method == "turn/completed" and matches_turn:
                self._turn_complete = True
                self._active_turn_id = None
                return

    async def collect_text(self, user_text: str) -> str:
        text_chunks: list[str] = []
        async for event in self.run(user_text):
            if event.method == "item/agentMessage/delta":
                delta = event.params.get("delta")
                if isinstance(delta, str):
                    text_chunks.append(delta)
        while not self._turn_complete and not self.pending_tool_calls():
            async for event in self.resume():
                if event.method == "item/agentMessage/delta":
                    delta = event.params.get("delta")
                    if isinstance(delta, str):
                        text_chunks.append(delta)
        return "".join(text_chunks)

    def pending_tool_calls(self) -> list[PendingToolCall]:
        return [tool_call for tool_call in self._pending_tool_calls if not tool_call.resolved]

    async def resolve_tool_call(self, tool_call: PendingToolCall) -> None:
        if isinstance(tool_call, PendingCommandExecution):
            await self.approve(tool_call)
            return
        if isinstance(tool_call, PendingFileChange):
            await self.approve(tool_call)
            return
        await self.run_function_tool(tool_call)

    async def apply_tool_decision(self, tool_call: PendingToolCall) -> bool:
        decision = await self._call_tool_approval(tool_call)
        if decision is None or isinstance(decision, DeferDecision):
            return False
        if isinstance(decision, ApproveDecision):
            await self.approve(tool_call, for_session=decision.for_session)
            return True
        if isinstance(decision, RejectDecision):
            await self.reject(tool_call, cancel=decision.cancel)
            return True
        if isinstance(decision, ReplaceCommandDecision):
            if not isinstance(tool_call, PendingCommandExecution):
                raise TypeError(
                    "ReplaceCommandDecision can only be used with built-in command approvals"
                )
            await self.replace_command(tool_call, decision.command)
            return True
        if isinstance(decision, RunDecision):
            if not isinstance(tool_call, PendingFunctionToolCall):
                raise TypeError("RunDecision can only be used with function tools")
            await self.run_function_tool(tool_call, arguments=decision.arguments)
            return True
        if isinstance(decision, RespondDecision):
            if not isinstance(tool_call, PendingFunctionToolCall):
                raise TypeError("RespondDecision is only valid for function tool calls")
            await self.submit_tool_result(tool_call, decision.result, success=decision.success)
            return True
        raise TypeError(f"unsupported tool decision: {type(decision)!r}")

    async def approve(self, tool_call: PendingToolCall, *, for_session: bool = False) -> None:
        if isinstance(tool_call, PendingCommandExecution):
            await self._respond_to_command_approval(tool_call, for_session=for_session)
            tool_call.resolved = True
            return
        if isinstance(tool_call, PendingFileChange):
            await self._respond_to_file_change_approval(tool_call, for_session=for_session)
            tool_call.resolved = True
            return
        await self.run_function_tool(tool_call)

    async def replace_command(
        self,
        tool_call: PendingCommandExecution,
        command: list[str],
    ) -> None:
        await self._respond_to_command_override(tool_call, command=command)
        tool_call.command = " ".join(command)
        tool_call.resolved = True

    async def reject(self, tool_call: PendingToolCall, *, cancel: bool = False) -> None:
        if isinstance(tool_call, PendingCommandExecution):
            await self._respond_to_command_rejection(tool_call, cancel=cancel)
            tool_call.resolved = True
            return
        if isinstance(tool_call, PendingFileChange):
            await self._respond_to_file_change_rejection(tool_call, cancel=cancel)
            tool_call.resolved = True
            return
        await self._reject_function_tool(tool_call, cancel=cancel)

    async def run_function_tool(
        self,
        tool_call: PendingFunctionToolCall,
        *,
        arguments: Mapping[str, Any] | None = None,
    ) -> None:
        tool = self.function_tools.get(tool_call.tool_name)
        if tool is None:
            await self.submit_tool_result(
                tool_call,
                f"unknown function tool: {tool_call.tool_name}",
                success=False,
            )
            return
        call_arguments = dict(arguments) if arguments is not None else tool_call.arguments
        try:
            result = await tool.run(call_arguments)
        except Exception as exc:
            await self.submit_tool_result(
                tool_call,
                f"function tool {tool_call.tool_name} failed: {exc}",
                success=False,
            )
            return
        await self.submit_tool_result(tool_call, result, success=True)

    async def submit_tool_result(
        self,
        tool_call: PendingFunctionToolCall,
        result: Any,
        *,
        success: bool = True,
    ) -> None:
        try:
            text = result if isinstance(result, str) else json.dumps(result, indent=2, sort_keys=True)
        except TypeError as exc:
            text = f"function tool {tool_call.tool_name} returned a non-serializable result: {exc}"
            success = False
        await self.session.app_server_client.send_result(
            tool_call.request_id,
            {
                "contentItems": [{"type": "inputText", "text": text}],
                "success": success,
            },
        )
        tool_call.resolved = True

    async def _handle_server_request(self, request: JsonRpcServerRequest) -> bool:
        tool_call = self._decode_tool_call(request)
        if tool_call is None:
            await self.session.app_server_client.send_error(
                request.request_id,
                -32601,
                f"unsupported server request method: {request.method}",
            )
            return False
        self._pending_tool_calls.append(tool_call)
        handled = await self.apply_tool_decision(tool_call)
        if handled:
            return False
        return not tool_call.resolved

    def _decode_tool_call(self, request: JsonRpcServerRequest) -> PendingToolCall | None:
        if request.method == "item/commandExecution/requestApproval":
            return PendingCommandExecution(
                request_id=request.request_id,
                thread_id=str(request.params.get("threadId", self.thread_id)),
                turn_id=str(request.params.get("turnId", "")),
                item_id=str(request.params.get("itemId", "")),
                approval_id=self._optional_str(request.params.get("approvalId")),
                reason=self._optional_str(request.params.get("reason")),
                command=self._optional_str(request.params.get("command")),
                cwd=self._optional_str(request.params.get("cwd")),
                command_actions=self._command_actions(request.params.get("commandActions")),
            )
        if request.method == "item/fileChange/requestApproval":
            return PendingFileChange(
                request_id=request.request_id,
                thread_id=str(request.params.get("threadId", self.thread_id)),
                turn_id=str(request.params.get("turnId", "")),
                item_id=str(request.params.get("itemId", "")),
                reason=self._optional_str(request.params.get("reason")),
                grant_root=self._optional_str(request.params.get("grantRoot")),
            )
        if request.method != "item/tool/call":
            return None
        tool_name = request.params.get("tool")
        arguments = request.params.get("arguments", {})
        if not isinstance(tool_name, str):
            raise RuntimeError("tool call is missing a string tool name")
        if not isinstance(arguments, dict):
            raise RuntimeError(f"tool call arguments for {tool_name} must be an object")
        return PendingFunctionToolCall(
            request_id=request.request_id,
            thread_id=str(request.params.get("threadId", self.thread_id)),
            turn_id=str(request.params.get("turnId", "")),
            call_id=str(request.params.get("callId", "")),
            tool_name=tool_name,
            arguments=arguments,
        )

    async def _call_tool_approval(self, tool_call: PendingToolCall) -> ToolDecision | None:
        decision: Any = None
        if isinstance(tool_call, PendingFunctionToolCall):
            tool = self.function_tools.get(tool_call.tool_name)
            if tool is None:
                return None
            decision = tool.approve(tool_call)
        elif isinstance(tool_call, PendingCommandExecution):
            command_name = self._command_name(tool_call.command)
            policy = self.builtin_tool_policies.get(command_name)
            if policy is None:
                return None
            decision = policy(tool_call)
        elif isinstance(tool_call, PendingFileChange):
            return None
        if inspect.isawaitable(decision):
            decision = await decision
        if decision is None or isinstance(decision, ToolDecision):
            return decision
        raise TypeError(f"tool approval must return ToolDecision or None, got {type(decision)!r}")

    async def _respond_to_command_approval(
        self,
        tool_call: PendingCommandExecution,
        *,
        for_session: bool,
    ) -> None:
        await self.session.app_server_client.send_result(
            tool_call.request_id,
            {"decision": "acceptForSession" if for_session else "accept"},
        )

    async def _respond_to_command_rejection(
        self,
        tool_call: PendingCommandExecution,
        *,
        cancel: bool,
    ) -> None:
        await self.session.app_server_client.send_result(
            tool_call.request_id,
            {"decision": "cancel" if cancel else "decline"},
        )

    async def _respond_to_command_override(
        self,
        tool_call: PendingCommandExecution,
        *,
        command: list[str],
    ) -> None:
        await self.session.app_server_client.send_result(
            tool_call.request_id,
            {
                "decision": {
                    "acceptWithCommandOverride": {
                        "command": command,
                    }
                }
            },
        )

    async def _respond_to_file_change_approval(
        self,
        tool_call: PendingFileChange,
        *,
        for_session: bool,
    ) -> None:
        await self.session.app_server_client.send_result(
            tool_call.request_id,
            {"decision": "acceptForSession" if for_session else "accept"},
        )

    async def _respond_to_file_change_rejection(
        self,
        tool_call: PendingFileChange,
        *,
        cancel: bool,
    ) -> None:
        await self.session.app_server_client.send_result(
            tool_call.request_id,
            {"decision": "cancel" if cancel else "decline"},
        )

    async def _reject_function_tool(self, tool_call: PendingFunctionToolCall, *, cancel: bool) -> None:
        action = "canceled" if cancel else "rejected"
        await self.submit_tool_result(
            tool_call,
            f"function tool {tool_call.tool_name} was {action} by the host",
            success=False,
        )

    @staticmethod
    def _command_actions(value: Any) -> list[dict[str, Any]] | None:
        if isinstance(value, list) and all(isinstance(action, dict) for action in value):
            return value
        return None

    @staticmethod
    def _optional_str(value: Any) -> str | None:
        return value if isinstance(value, str) else None

    @staticmethod
    def _command_name(command: str | None) -> str:
        if not command:
            return "exec_command"
        return "write_stdin" if command.startswith("write_stdin:") else "exec_command"
