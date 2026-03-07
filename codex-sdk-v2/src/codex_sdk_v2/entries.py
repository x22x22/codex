from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
import shutil


@dataclass(slots=True)
class Entry:
    def materialize(self, destination: Path) -> None:
        raise NotImplementedError


@dataclass(slots=True)
class Dir(Entry):
    children: dict[str | Path, Entry] = field(default_factory=dict)
    description: str | None = None

    def materialize(self, destination: Path) -> None:
        destination.mkdir(parents=True, exist_ok=True)
        for name, entry in self.children.items():
            entry.materialize(destination / Path(name))


@dataclass(slots=True)
class LocalFile(Entry):
    src: Path
    mode: int = 0o644

    def materialize(self, destination: Path) -> None:
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(self.src, destination)
        destination.chmod(self.mode)


@dataclass(slots=True)
class LocalDir(Entry):
    src: Path

    def materialize(self, destination: Path) -> None:
        if destination.exists():
            shutil.rmtree(destination)
        shutil.copytree(self.src, destination)
