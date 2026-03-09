from __future__ import annotations

from dataclasses import dataclass
from typing import Any, ClassVar, Literal, Mapping


class ToolDecision:
    pass


@dataclass(frozen=True, slots=True)
class ApproveDecision(ToolDecision):
    for_session: bool = False


@dataclass(frozen=True, slots=True)
class RejectDecision(ToolDecision):
    cancel: bool = False


@dataclass(frozen=True, slots=True)
class DeferDecision(ToolDecision):
    pass


@dataclass(frozen=True, slots=True)
class RunDecision(ToolDecision):
    arguments: Mapping[str, Any] | None = None


@dataclass(frozen=True, slots=True)
class ReplaceCommandDecision(ToolDecision):
    command: list[str]


@dataclass(frozen=True, slots=True)
class RespondDecision(ToolDecision):
    result: Any
    success: bool = True


class PendingToolCall:
    kind: ClassVar[str]

    async def __call__(self, task: Any) -> None:
        await task.resolve_tool_call(self)

    def describe(self) -> str:
        raise NotImplementedError


@dataclass(slots=True)
class PendingCommandExecution(PendingToolCall):
    kind: ClassVar[str] = "command_execution"
    request_id: int | str
    thread_id: str
    turn_id: str
    item_id: str
    approval_id: str | None
    reason: str | None
    command: str | None
    cwd: str | None
    command_actions: list[dict[str, Any]] | None
    resolved: bool = False

    def describe(self) -> str:
        if self.command:
            return f"Approve command: {self.command}"
        if self.reason:
            return f"Approve command execution: {self.reason}"
        return "Approve command execution"


@dataclass(slots=True)
class PendingFileChange(PendingToolCall):
    kind: ClassVar[str] = "file_change"
    request_id: int | str
    thread_id: str
    turn_id: str
    item_id: str
    reason: str | None
    grant_root: str | None
    resolved: bool = False

    def describe(self) -> str:
        if self.reason:
            return f"Approve file changes: {self.reason}"
        return "Approve file changes"


@dataclass(slots=True)
class PendingFunctionToolCall(PendingToolCall):
    kind: ClassVar[str] = "function_tool"
    request_id: int | str
    thread_id: str
    turn_id: str
    call_id: str
    tool_name: str
    arguments: dict[str, Any]
    resolved: bool = False

    def describe(self) -> str:
        return f"Run function tool {self.tool_name}({self.arguments})"
