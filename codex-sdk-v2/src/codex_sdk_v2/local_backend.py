from __future__ import annotations

import asyncio
from dataclasses import dataclass
import os
from pathlib import Path
import shutil
import sys

from .app_server_client import AppServerClient
from .manifest import Manifest

APP_SERVER_STREAM_LIMIT = 16 * 1024 * 1024


@dataclass(slots=True)
class LocalBackendOptions:
    workspace_root: Path | None = None
    codex_binary: Path | None = None


class LocalSession:
    def __init__(
        self,
        *,
        workspace_root: Path,
        app_server_binary: Path,
        app_server_args: tuple[str, ...],
        owned_workspace: bool,
    ) -> None:
        self.workspace_root = workspace_root
        self.app_server_binary = app_server_binary
        self.app_server_args = app_server_args
        self.owned_workspace = owned_workspace
        self.app_server_process: asyncio.subprocess.Process | None = None
        self.app_server_client: AppServerClient | None = None
        self._stderr_task: asyncio.Task[None] | None = None

    async def start_app_server(self) -> AppServerClient:
        if self.app_server_client is not None:
            return self.app_server_client
        env = os.environ.copy()
        debug_enabled = env.get("CODEX_SDK_V2_DEBUG") == "1"
        if debug_enabled and "RUST_LOG" not in env:
            env["RUST_LOG"] = "codex_app_server=info"
        process = await asyncio.create_subprocess_exec(
            str(self.app_server_binary),
            *self.app_server_args,
            cwd=str(self.workspace_root),
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env=env,
            limit=APP_SERVER_STREAM_LIMIT,
        )
        self.app_server_process = process
        if debug_enabled and process.stderr is not None:
            self._stderr_task = asyncio.create_task(self._pump_stderr(process.stderr))
        self.app_server_client = AppServerClient(process)
        return self.app_server_client

    async def stop(self) -> None:
        if self.app_server_process is not None:
            self.app_server_process.terminate()
            await self.app_server_process.wait()
            self.app_server_process = None
            self.app_server_client = None
        if self._stderr_task is not None:
            await self._stderr_task
            self._stderr_task = None
        if self.owned_workspace:
            shutil.rmtree(self.workspace_root, ignore_errors=True)

    async def _pump_stderr(self, stream: asyncio.StreamReader) -> None:
        while True:
            line = await stream.readline()
            if not line:
                return
            print(f"[codex-app-server] {line.decode('utf-8', errors='replace').rstrip()}", file=sys.stderr)


class LocalBackend:
    def __init__(self, *, codex_binary: Path | None = None) -> None:
        self.codex_binary = codex_binary or self._default_app_server_binary()

    async def create_session(
        self,
        *,
        manifest: Manifest,
        options: LocalBackendOptions | None = None,
    ) -> LocalSession:
        options = options or LocalBackendOptions()
        codex_binary = options.codex_binary or self.codex_binary
        if not codex_binary.exists():
            raise RuntimeError(f"codex binary not found at {codex_binary}")
        app_server_args = self._app_server_args_for_binary(codex_binary)

        if options.workspace_root is None:
            workspace_root = manifest.materialize()
            owned_workspace = True
        else:
            workspace_root = options.workspace_root
            workspace_root.mkdir(parents=True, exist_ok=True)
            materialized = manifest.materialize()
            try:
                for child in materialized.iterdir():
                    destination = workspace_root / child.name
                    if destination.exists():
                        if destination.is_dir():
                            shutil.rmtree(destination)
                        else:
                            destination.unlink()
                    shutil.move(str(child), str(destination))
            finally:
                shutil.rmtree(materialized, ignore_errors=True)
            owned_workspace = False

        return LocalSession(
            workspace_root=workspace_root,
            app_server_binary=codex_binary,
            app_server_args=app_server_args,
            owned_workspace=owned_workspace,
        )

    @staticmethod
    def _default_app_server_binary() -> Path:
        repo_app_server = Path(__file__).resolve().parents[3] / "codex-rs" / "target" / "debug" / "codex-app-server"
        if repo_app_server.exists():
            return repo_app_server
        return Path(shutil.which("codex") or "/opt/homebrew/bin/codex")

    @staticmethod
    def _app_server_args_for_binary(binary: Path) -> tuple[str, ...]:
        if binary.name == "codex-app-server":
            return ("--listen", "stdio://")
        return ("app-server", "--listen", "stdio://")
