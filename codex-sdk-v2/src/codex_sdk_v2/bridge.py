from __future__ import annotations

from dataclasses import dataclass
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import threading
from typing import Protocol

import httpx


class ResponsesBridge(Protocol):
    def serve_forever(self) -> None: ...
    def shutdown(self) -> None: ...
    @property
    def bridge_url(self) -> str: ...


@dataclass(slots=True)
class _BridgeConfig:
    bind_host: str
    port: int
    upstream_url: str
    auth_header: str


class _ResponsesHandler(BaseHTTPRequestHandler):
    server: "_BridgeServer"

    def do_POST(self) -> None:  # noqa: N802
        config = self.server.config
        if self.path != "/v1/responses":
            self.send_error(HTTPStatus.FORBIDDEN)
            return

        content_length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(content_length)

        upstream_headers = {
            key: value
            for key, value in self.headers.items()
            if key.lower() not in {"authorization", "host", "content-length"}
        }
        upstream_headers["Authorization"] = config.auth_header

        with httpx.stream(
            "POST",
            config.upstream_url,
            headers=upstream_headers,
            content=body,
            timeout=None,
        ) as response:
            self.send_response(response.status_code)
            for key, value in response.headers.items():
                if key.lower() in {"content-length", "transfer-encoding", "connection"}:
                    continue
                self.send_header(key, value)
            self.end_headers()
            for chunk in response.iter_raw():
                self.wfile.write(chunk)
                self.wfile.flush()

    def log_message(self, _format: str, *args: object) -> None:
        _ = args
        return


class _BridgeServer(ThreadingHTTPServer):
    def __init__(self, config: _BridgeConfig) -> None:
        super().__init__((config.bind_host, config.port), _ResponsesHandler)
        self.config = config


class OpenAIResponsesBridge:
    def __init__(
        self,
        *,
        api_key: str,
        bind_host: str = "127.0.0.1",
        port: int = 0,
        upstream_url: str = "https://api.openai.com/v1/responses",
    ) -> None:
        self._config = _BridgeConfig(
            bind_host=bind_host,
            port=port,
            upstream_url=upstream_url,
            auth_header=f"Bearer {api_key}",
        )
        self._server = _BridgeServer(self._config)
        self._thread: threading.Thread | None = None

    @property
    def bridge_url(self) -> str:
        host, port = self._server.server_address
        return f"http://{host}:{port}/v1"

    def serve_forever(self) -> None:
        self._server.serve_forever(poll_interval=0.1)

    def start(self) -> None:
        if self._thread is not None:
            return
        # `ThreadingHTTPServer` is synchronous. Running it on a daemon thread keeps
        # the bridge loop independent from the caller's asyncio event loop.
        self._thread = threading.Thread(target=self.serve_forever, daemon=True)
        self._thread.start()

    def shutdown(self) -> None:
        self._server.shutdown()
        self._server.server_close()
        if self._thread is not None:
            self._thread.join(timeout=1)
            self._thread = None

    def __enter__(self) -> "OpenAIResponsesBridge":
        self.start()
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        _ = (exc_type, exc, tb)
        self.shutdown()
