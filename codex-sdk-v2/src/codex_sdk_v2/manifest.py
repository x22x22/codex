from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
import tempfile

from .entries import Entry


@dataclass(slots=True)
class Manifest:
    root: str = "/workspace"
    entries: dict[str | Path, Entry] = field(default_factory=dict)

    def materialize(self) -> Path:
        tempdir = Path(tempfile.mkdtemp(prefix="codex-sdk-v2-manifest-"))
        for name, entry in self.entries.items():
            entry.materialize(tempdir / Path(name))
        return tempdir
