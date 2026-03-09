from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, Awaitable, Callable, ClassVar, Mapping, Sequence

if TYPE_CHECKING:
    from .pending_tool_calls import PendingCommandExecution
    from .pending_tool_calls import PendingFunctionToolCall
    from .pending_tool_calls import ToolDecision

if TYPE_CHECKING:
    BuiltinApprovalPolicy = Callable[
        [PendingCommandExecution],
        Awaitable[ToolDecision | None] | ToolDecision | None,
    ]
else:
    BuiltinApprovalPolicy = Callable[[Any], Awaitable[Any] | Any]


@dataclass(frozen=True, slots=True)
class BuiltinToolSpec:
    tool_name: str


class Tool:
    pass


@dataclass(frozen=True, slots=True)
class ConfiguredBuiltinTool(Tool):
    tool_type: type["BuiltinTool"]
    approval_policy: BuiltinApprovalPolicy | None = None

    def builtin_spec(self) -> BuiltinToolSpec:
        return self.tool_type.builtin_spec()


class BuiltinTool(Tool):
    codex_builtin_tool: ClassVar[str]

    @classmethod
    def builtin_spec(cls) -> BuiltinToolSpec:
        tool_name = getattr(cls, "codex_builtin_tool", None)
        if not tool_name:
            raise TypeError(f"{cls.__name__} must define codex_builtin_tool")
        return BuiltinToolSpec(tool_name=tool_name)

    @classmethod
    def with_approval_policy(
        cls,
        *,
        policy: BuiltinApprovalPolicy,
    ) -> ConfiguredBuiltinTool:
        return ConfiguredBuiltinTool(tool_type=cls, approval_policy=policy)


class FunctionTool(Tool):
    name: ClassVar[str]
    description: ClassVar[str]
    input_schema: ClassVar[dict[str, Any]]

    @classmethod
    def dynamic_tool_spec(cls) -> dict[str, Any]:
        name = getattr(cls, "name", None)
        description = getattr(cls, "description", None)
        input_schema = getattr(cls, "input_schema", None)
        if not name:
            raise TypeError(f"{cls.__name__} must define name")
        if not description:
            raise TypeError(f"{cls.__name__} must define description")
        if not isinstance(input_schema, dict):
            raise TypeError(f"{cls.__name__} must define input_schema as a dict")
        return {
            "name": name,
            "description": description,
            "inputSchema": input_schema,
        }

    async def approve(self, call: PendingFunctionToolCall) -> ToolDecision | None:
        return None

    def instructions(self) -> str | None:
        return None

    async def run(self, arguments: Mapping[str, Any]) -> Any:
        raise NotImplementedError


class ExecCommand(BuiltinTool):
    codex_builtin_tool = "exec_command"


class WriteStdin(BuiltinTool):
    codex_builtin_tool = "write_stdin"


class Shell(BuiltinTool):
    codex_builtin_tool = "shell"


class UpdatePlan(BuiltinTool):
    codex_builtin_tool = "update_plan"


class RequestUserInput(BuiltinTool):
    codex_builtin_tool = "request_user_input"


class ApplyPatch(BuiltinTool):
    codex_builtin_tool = "apply_patch"


class WebSearch(BuiltinTool):
    codex_builtin_tool = "web_search"


class ViewImage(BuiltinTool):
    codex_builtin_tool = "view_image"


class SearchToolBm25(BuiltinTool):
    codex_builtin_tool = "search_tool_bm25"


class ReadFile(BuiltinTool):
    codex_builtin_tool = "read_file"


class ListDir(BuiltinTool):
    codex_builtin_tool = "list_dir"


class GrepFiles(BuiltinTool):
    codex_builtin_tool = "grep_files"


class ListMcpResources(BuiltinTool):
    codex_builtin_tool = "list_mcp_resources"


class ListMcpResourceTemplates(BuiltinTool):
    codex_builtin_tool = "list_mcp_resource_templates"


class ReadMcpResource(BuiltinTool):
    codex_builtin_tool = "read_mcp_resource"


class SpawnAgent(BuiltinTool):
    codex_builtin_tool = "spawn_agent"


class SendInput(BuiltinTool):
    codex_builtin_tool = "send_input"


class ResumeAgent(BuiltinTool):
    codex_builtin_tool = "resume_agent"


class Wait(BuiltinTool):
    codex_builtin_tool = "wait"


class CloseAgent(BuiltinTool):
    codex_builtin_tool = "close_agent"


class SpawnAgentsOnCsv(BuiltinTool):
    codex_builtin_tool = "spawn_agents_on_csv"


class JsRepl(BuiltinTool):
    codex_builtin_tool = "js_repl"


class JsReplReset(BuiltinTool):
    codex_builtin_tool = "js_repl_reset"


class Artifacts(BuiltinTool):
    codex_builtin_tool = "artifacts"


class ReportAgentJobResult(BuiltinTool):
    codex_builtin_tool = "report_agent_job_result"


class TestSyncTool(BuiltinTool):
    codex_builtin_tool = "test_sync_tool"


ALL_BUILTIN_TOOLS: tuple[type[BuiltinTool], ...] = (
    ExecCommand,
    WriteStdin,
    Shell,
    UpdatePlan,
    RequestUserInput,
    ApplyPatch,
    WebSearch,
    ViewImage,
    SearchToolBm25,
    ReadFile,
    ListDir,
    GrepFiles,
    ListMcpResources,
    ListMcpResourceTemplates,
    ReadMcpResource,
    SpawnAgent,
    SendInput,
    ResumeAgent,
    Wait,
    CloseAgent,
    SpawnAgentsOnCsv,
    JsRepl,
    JsReplReset,
    Artifacts,
    ReportAgentJobResult,
    TestSyncTool,
)


def builtin_tools(tools: Sequence[Tool | type[Tool]]) -> tuple[list[str], dict[str, BuiltinApprovalPolicy]]:
    names: list[str] = []
    policies: dict[str, BuiltinApprovalPolicy] = {}
    seen: set[str] = set()
    for tool in tools:
        resolved = tool if isinstance(tool, ConfiguredBuiltinTool) else None
        tool_type = resolved.tool_type if resolved is not None else tool if isinstance(tool, type) else type(tool)
        if not isinstance(tool_type, type) or not issubclass(tool_type, BuiltinTool):
            continue
        name = tool_type.builtin_spec().tool_name
        if name not in seen:
            seen.add(name)
            names.append(name)
        if resolved is not None and resolved.approval_policy is not None:
            policies[name] = resolved.approval_policy
    return names, policies


def function_tools(tools: Sequence[Tool | type[Tool]]) -> list[FunctionTool]:
    resolved: list[FunctionTool] = []
    seen: set[str] = set()
    builtin_names = {tool.builtin_spec().tool_name for tool in ALL_BUILTIN_TOOLS}
    for tool in tools:
        if isinstance(tool, ConfiguredBuiltinTool):
            continue
        tool_instance = tool() if isinstance(tool, type) else tool
        if not isinstance(tool_instance, FunctionTool):
            continue
        tool_name = type(tool_instance).dynamic_tool_spec()["name"]
        if tool_name in builtin_names:
            raise ValueError(f"function tool name collides with codex built-in: {tool_name}")
        if tool_name in seen:
            raise ValueError(f"duplicate function tool name: {tool_name}")
        seen.add(tool_name)
        resolved.append(tool_instance)
    return resolved


def tool_instruction_fragments(tools: Sequence[Tool | type[Tool]]) -> list[str]:
    fragments: list[str] = []
    seen: set[str] = set()

    for tool in tools:
        if isinstance(tool, ConfiguredBuiltinTool):
            continue

        tool_instance = tool() if isinstance(tool, type) else tool
        if isinstance(tool_instance, FunctionTool):
            fragment = tool_instance.instructions()
            key = f"function:{type(tool_instance).dynamic_tool_spec()['name']}"
            if fragment and key not in seen:
                seen.add(key)
                fragments.append(fragment)

    return fragments


DEFAULT_TOOLS: tuple[type[BuiltinTool], ...] = (
    ExecCommand,
    WriteStdin,
)
