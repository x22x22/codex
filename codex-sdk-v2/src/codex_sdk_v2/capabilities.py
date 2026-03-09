from __future__ import annotations

from dataclasses import dataclass

from .manifest import Manifest
from .tools import ExecCommand
from .tools import Tool
from .tools import WriteStdin


class Capability:
    def tools(self) -> tuple[Tool | type[Tool], ...]:
        return ()

    def instructions(self) -> str | None:
        return None

    def process_manifest(self, manifest: Manifest) -> Manifest:
        return manifest


@dataclass(frozen=True, slots=True)
class UnifiedExecCapability(Capability):
    def tools(self) -> tuple[Tool | type[Tool], ...]:
        return (ExecCommand, WriteStdin)


DEFAULT_CAPABILITIES: tuple[Capability, ...] = (UnifiedExecCapability(),)
