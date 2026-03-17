# codex-exec-server

`codex-exec-server` is a small standalone JSON-RPC server for spawning and
controlling subprocesses through `codex-utils-pty`.

It currently provides:

- a standalone binary: `codex-exec-server`
- a transport-agnostic server runtime with stdio and websocket entrypoints
- a Rust client: `ExecServerClient`
- a direct in-process client mode: `ExecServerClient::connect_in_process`
- a separate local launch helper: `spawn_local_exec_server`
- a small protocol module with shared request/response types

This crate is intentionally narrow. It is not wired into the main Codex CLI or
unified-exec in this PR; it is only the standalone transport layer.

The internal shape is intentionally closer to `app-server` than the first cut:

- transport adapters are separate from the per-connection request processor
- JSON-RPC request dispatch is backed by `jsonrpsee` and kept separate from the
  stateful exec handler
- the client only speaks the protocol; it does not spawn a server subprocess
- the client can also bypass the JSON-RPC transport/routing layer in local
  in-process mode and call the typed handler directly
- local child-process launch is handled by a separate helper/factory layer

That split is meant to leave reusable seams if exec-server and app-server later
share transport or JSON-RPC connection utilities. It also keeps the core
handler testable without the RPC server implementation itself.

Design notes for a likely future integration with unified exec, including
rough call flow, buffering, and sandboxing boundaries, live in
[DESIGN.md](./DESIGN.md).

## Transport

The server speaks the same JSON-RPC message shapes over multiple transports.

The standalone binary supports:

- `stdio://` (default)
- `ws://IP:PORT`

Wire framing:

- stdio: one newline-delimited JSON-RPC message per line on stdin/stdout
- websocket: one JSON-RPC message per websocket text frame

Like the app-server transport, messages on the wire omit the `"jsonrpc":"2.0"`
field and use the shared `codex-app-server-protocol` envelope types.

The current protocol version is:

```text
exec-server.v0
```

## Lifecycle

Each connection follows this sequence:

1. Send `initialize`.
2. Wait for the `initialize` response.
3. Send `initialized`.
4. Start and manage processes with `process/start`, `process/read`,
   `process/write`, and `process/terminate`.
5. Read streaming notifications from `process/output` and
   `process/exited`.

If the client sends exec methods before completing the `initialize` /
`initialized` handshake, the server rejects them.

If a connection closes, the server terminates any remaining managed processes
for that connection.

## API

### `initialize`

Initial handshake request.

Request params:

```json
{
  "clientName": "my-client"
}
```

Response:

```json
{
  "protocolVersion": "exec-server.v0"
}
```

### `initialized`

Handshake acknowledgement notification sent by the client after a successful
`initialize` response. Exec methods are rejected until this arrives.

Params are currently ignored. Sending any other client notification method is a
protocol error.

### `process/start`

Starts a new managed process.

Request params:

```json
{
  "processId": "proc-1",
  "argv": ["bash", "-lc", "printf 'hello\\n'"],
  "cwd": "/absolute/working/directory",
  "env": {
    "PATH": "/usr/bin:/bin"
  },
  "tty": true,
  "arg0": null
}
```

Field definitions:

- `argv`: command vector. It must be non-empty.
- `cwd`: absolute working directory used for the child process.
- `env`: environment variables passed to the child process.
- `tty`: when `true`, spawn a PTY-backed interactive process; when `false`,
  spawn a pipe-backed process with closed stdin.
- `arg0`: optional argv0 override forwarded to `codex-utils-pty`.

Response:

```json
{
  "processId": "proc-1"
}
```

Behavior notes:

- `processId` is chosen by the client and must be unique for the connection.
- PTY-backed processes accept later writes through `process/write`.
- Pipe-backed processes are launched with stdin closed and reject writes.
- Output is streamed asynchronously via `process/output`.
- Exit is reported asynchronously via `process/exited`.

### `process/write`

Writes raw bytes to a running PTY-backed process stdin.

Request params:

```json
{
  "processId": "proc-1",
  "chunk": "aGVsbG8K"
}
```

`chunk` is base64-encoded raw bytes. In the example above it is `hello\n`.

Response:

```json
{
  "accepted": true
}
```

Behavior notes:

- Writes to an unknown `processId` are rejected.
- Writes to a non-PTY process are rejected because stdin is already closed.

### `process/read`

Reads retained output from a managed process by sequence number.

Request params:

```json
{
  "processId": "proc-1",
  "afterSeq": 0,
  "maxBytes": 65536,
  "waitMs": 250
}
```

Response:

```json
{
  "chunks": [
    {
      "seq": 1,
      "stream": "pty",
      "chunk": "aGVsbG8K"
    }
  ],
  "nextSeq": 2,
  "exited": false,
  "exitCode": null
}
```

Behavior notes:

- Output is retained in bounded server memory so callers can poll without
  relying only on notifications.
- `afterSeq` is exclusive: `0` reads from the beginning of the retained buffer.
- `waitMs` waits briefly for new output or exit if nothing is currently
  available.
- Once retained output exceeds the per-process cap, oldest chunks are dropped.

### `process/terminate`

Terminates a running managed process.

Request params:

```json
{
  "processId": "proc-1"
}
```

Response:

```json
{
  "running": true
}
```

If the process is already unknown or already removed, the server responds with:

```json
{
  "running": false
}
```

## Notifications

### `process/output`

Streaming output chunk from a running process.

Params:

```json
{
  "processId": "proc-1",
  "stream": "stdout",
  "chunk": "aGVsbG8K"
}
```

Fields:

- `processId`: process identifier
- `stream`: `"stdout"`, `"stderr"`, or `"pty"` for PTY-backed processes
- `chunk`: base64-encoded output bytes

### `process/exited`

Final process exit notification.

Params:

```json
{
  "processId": "proc-1",
  "exitCode": 0
}
```

## Errors

The server returns JSON-RPC errors with these codes:

- `-32600`: invalid request
- `-32602`: invalid params
- `-32603`: internal error

Typical error cases:

- unknown method
- malformed params
- empty `argv`
- duplicate `processId`
- writes to unknown processes
- writes to non-PTY processes

## Rust surface

The crate exports:

- `ExecServerClient`
- `ExecServerClientConnectOptions`
- `RemoteExecServerConnectArgs`
- `ExecServerLaunchCommand`
- `ExecServerEvent`
- `SpawnedExecServer`
- `ExecServerError`
- `ExecServerTransport`
- `spawn_local_exec_server(...)`
- protocol structs such as `ExecParams`, `ExecResponse`,
  `WriteParams`, `TerminateParams`, `ExecOutputDeltaNotification`, and
  `ExecExitedNotification`
- `run_main()` and `run_main_with_transport(...)`

### Binary

Run over stdio:

```text
codex-exec-server
```

Run as a websocket server:

```text
codex-exec-server --listen ws://127.0.0.1:8080
```

### Client

Connect the client to an existing server transport:

- `ExecServerClient::connect_stdio(...)`
- `ExecServerClient::connect_websocket(...)`
- `ExecServerClient::connect_in_process(...)` for a local no-transport mode
  backed directly by the typed handler

Timeout behavior:

- stdio and websocket clients both enforce an initialize-handshake timeout
- websocket clients also enforce a connect timeout before the handshake begins

Events:

- `ExecServerClient::event_receiver()` yields `ExecServerEvent`
- output events include both `stream` (`stdout`, `stderr`, or `pty`) and raw
  bytes
- process lifetime is tracked by server notifications such as
  `process/exited`, not by a client-side process registry

Spawning a local child process is deliberately separate:

- `spawn_local_exec_server(...)`

## Example session

Initialize:

```json
{"id":1,"method":"initialize","params":{"clientName":"example-client"}}
{"id":1,"result":{"protocolVersion":"exec-server.v0"}}
{"method":"initialized","params":{}}
```

Start a process:

```json
{"id":2,"method":"process/start","params":{"processId":"proc-1","argv":["bash","-lc","printf 'ready\\n'; while IFS= read -r line; do printf 'echo:%s\\n' \"$line\"; done"],"cwd":"/tmp","env":{"PATH":"/usr/bin:/bin"},"tty":true,"arg0":null}}
{"id":2,"result":{"processId":"proc-1"}}
{"method":"process/output","params":{"processId":"proc-1","stream":"pty","chunk":"cmVhZHkK"}}
```

Write to the process:

```json
{"id":3,"method":"process/write","params":{"processId":"proc-1","chunk":"aGVsbG8K"}}
{"id":3,"result":{"accepted":true}}
{"method":"process/output","params":{"processId":"proc-1","stream":"pty","chunk":"ZWNobzpoZWxsbwo="}}
```

Terminate it:

```json
{"id":4,"method":"process/terminate","params":{"processId":"proc-1"}}
{"id":4,"result":{"running":true}}
{"method":"process/exited","params":{"processId":"proc-1","exitCode":0}}
```
