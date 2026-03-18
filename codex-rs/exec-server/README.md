# Exec-Server JSON-RPC Base Sketch

This is a review-only sketch of the simplified exec-server JSON-RPC base.

It is intentionally scoped to the transport and dispatch shape:

- websocket-only transport
- direct `JSONRPCMessage -> handler -> JSONRPCMessage` processing
- no `"jsonrpc": "2.0"` envelope requirement
- a local bypass path represented by direct handler calls rather than an in-memory transport

It does not include the later `process/*` or filesystem RPC surface.

The goal of this sketch is to show the app-server-like structure without stacking the rest of the exec-server implementation on top of it.
