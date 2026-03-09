from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Literal, TypeAlias

from .app_server_client import JsonRpcNotification, JsonRpcServerRequest


@dataclass(frozen=True, slots=True)
class ApprovalDecision:
    decision: Literal["approve", "approve_for_session", "reject", "cancel"]


@dataclass(frozen=True, slots=True)
class CommandApprovalRequestEvent:
    request_id: int | str
    thread_id: str
    turn_id: str
    item_id: str
    approval_id: str | None
    reason: str | None
    command: str | None
    cwd: str | None
    command_actions: list[dict[str, Any]] | None
    raw_request: JsonRpcServerRequest


@dataclass(frozen=True, slots=True)
class FileChangeApprovalRequestEvent:
    request_id: int | str
    thread_id: str
    turn_id: str
    item_id: str
    reason: str | None
    grant_root: str | None
    raw_request: JsonRpcServerRequest


@dataclass(frozen=True, slots=True)
class FunctionToolCallEvent:
    request_id: int | str
    thread_id: str
    turn_id: str
    call_id: str
    tool_name: str
    arguments: dict[str, Any]
    raw_request: JsonRpcServerRequest


ApprovalRequestEvent: TypeAlias = (
    CommandApprovalRequestEvent | FileChangeApprovalRequestEvent | FunctionToolCallEvent
)
TaskEvent: TypeAlias = JsonRpcNotification | ApprovalRequestEvent
