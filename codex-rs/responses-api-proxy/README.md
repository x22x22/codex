# codex-responses-api-proxy

A strict HTTP proxy that only forwards `POST` requests to `/v1/responses` to the OpenAI API (`https://api.openai.com`), injecting the `Authorization: Bearer $OPENAI_API_KEY` header. Everything else is rejected with `403 Forbidden`.

**NEW:** The proxy now supports bridge mode with `--bridge-to-chat`, which allows you to use remote model providers that only support the Chat Completions API (`/v1/chat/completions`) with clients that expect the Responses API (`/v1/responses`).

## Expected Usage

**IMPORTANT:** `codex-responses-api-proxy` is designed to be run by a privileged user with access to `OPENAI_API_KEY` so that an unprivileged user cannot inspect or tamper with the process. Though if `--http-shutdown` is specified, an unprivileged user _can_ make a `GET` request to `/shutdown` to shutdown the server, as an unprivileged user could not send `SIGTERM` to kill the process.

A privileged user (i.e., `root` or a user with `sudo`) who has access to `OPENAI_API_KEY` would run the following to start the server, as `codex-responses-api-proxy` reads the auth token from `stdin`:

```shell
printenv OPENAI_API_KEY | env -u OPENAI_API_KEY codex-responses-api-proxy --http-shutdown --server-info /tmp/server-info.json
```

A non-privileged user would then run Codex as follows, specifying the `model_provider` dynamically:

```shell
PROXY_PORT=$(jq .port /tmp/server-info.json)
PROXY_BASE_URL="http://127.0.0.1:${PROXY_PORT}"
codex exec -c "model_providers.openai-proxy={ name = 'OpenAI Proxy', base_url = '${PROXY_BASE_URL}/v1', wire_api='responses' }" \
    -c model_provider="openai-proxy" \
    'Your prompt here'
```

When the unprivileged user was finished, they could shutdown the server using `curl` (since `kill -SIGTERM` is not an option):

```shell
curl --fail --silent --show-error "${PROXY_BASE_URL}/shutdown"
```

## Bridge Mode: Using Chat Completions API with Responses API Clients

Bridge mode allows you to use remote model providers that only support the OpenAI Chat Completions API (e.g., Alibaba Dashscope, Claude via Amazon Bedrock) with clients that only know how to use the Responses API.

### Example: Using Alibaba Dashscope

```shell
# Start the proxy in bridge mode
echo "sk-98e55d42763e4e2fa9253e35783aba08" | codex-responses-api-proxy \
  --bridge-to-chat \
  --http-shutdown \
  --server-info /tmp/server-info.json \
  --upstream-url "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"

# In another terminal, use Codex with the proxy
PROXY_PORT=$(jq .port /tmp/server-info.json)
PROXY_BASE_URL="http://127.0.0.1:${PROXY_PORT}"
codex exec -c "model_providers.dashscope-proxy={ name = 'Dashscope via Proxy', base_url = '${PROXY_BASE_URL}/v1', wire_api='responses' }" \
    -c model_provider="dashscope-proxy" \
    -c model="qwen-max" \
    'Your prompt here'
```

### How Bridge Mode Works

When `--bridge-to-chat` is enabled:

1. The proxy accepts Responses API requests at `POST /v1/responses`
2. Transforms the request body from Responses API format to Chat Completions format
3. Forwards the transformed request to the upstream Chat Completions endpoint
4. Transforms the streaming SSE response from Chat Completions format back to Responses API format
5. Streams the transformed response back to the client

This allows you to use any OpenAI-compatible Chat Completions endpoint with Codex or other clients that expect the Responses API.

## Behavior

- Reads the API key from `stdin`. All callers should pipe the key in (for example, `printenv OPENAI_API_KEY | codex-responses-api-proxy`).
- Formats the header value as `Bearer <key>` and attempts to `mlock(2)` the memory holding that header so it is not swapped to disk.
- Listens on the provided port or an ephemeral port if `--port` is not specified.
- Accepts exactly `POST /v1/responses` (no query string). The request body is forwarded to `https://api.openai.com/v1/responses` with `Authorization: Bearer <key>` set. All original request headers (except any incoming `Authorization`) are forwarded upstream, with `Host` overridden to `api.openai.com`. For other requests, it responds with `403`.
- Optionally writes a single-line JSON file with server info, currently `{ "port": <u16>, "pid": <u32> }`.
- Optional `--http-shutdown` enables `GET /shutdown` to terminate the process with exit code `0`. This allows one user (e.g., `root`) to start the proxy and another unprivileged user on the host to shut it down.

## CLI

```
codex-responses-api-proxy [--port <PORT>] [--server-info <FILE>] [--http-shutdown] [--upstream-url <URL>] [--bridge-to-chat]
```

- `--port <PORT>`: Port to bind on `127.0.0.1`. If omitted, an ephemeral port is chosen.
- `--server-info <FILE>`: If set, the proxy writes a single line of JSON with `{ "port": <PORT>, "pid": <PID> }` once listening.
- `--http-shutdown`: If set, enables `GET /shutdown` to exit the process with code `0`.
- `--upstream-url <URL>`: Absolute URL to forward requests to. Defaults to `https://api.openai.com/v1/responses`.
- `--bridge-to-chat`: Enable bridge mode to convert Responses API requests to Chat Completions API. When enabled, the upstream URL should be a `/chat/completions` endpoint.
- Authentication is fixed to `Authorization: Bearer <key>` to match the Codex CLI expectations.

For Azure, for example (ensure your deployment accepts `Authorization: Bearer <key>`):

```shell
printenv AZURE_OPENAI_API_KEY | env -u AZURE_OPENAI_API_KEY codex-responses-api-proxy \
  --http-shutdown \
  --server-info /tmp/server-info.json \
  --upstream-url "https://YOUR_PROJECT_NAME.openai.azure.com/openai/deployments/YOUR_DEPLOYMENT/responses?api-version=2025-04-01-preview"
```

## Notes

- Only `POST /v1/responses` is permitted. No query strings are allowed.
- All request headers are forwarded to the upstream call (aside from overriding `Authorization` and `Host`). Response status and content-type are mirrored from upstream.
- In bridge mode, the SSE stream is transformed line-by-line from Chat Completions format to Responses API format.

## Hardening Details

Care is taken to restrict access/copying to the value of `OPENAI_API_KEY` retained in memory:

- We leverage [`codex_process_hardening`](https://github.com/openai/codex/blob/main/codex-rs/process-hardening/README.md) so `codex-responses-api-proxy` is run with standard process-hardening techniques.
- At startup, we allocate a `1024` byte buffer on the stack and copy `"Bearer "` into the start of the buffer.
- We then read from `stdin`, copying the contents into the buffer after `"Bearer "`.
- After verifying the key matches `/^[a-zA-Z0-9_-]+$/` (and does not exceed the buffer), we create a `String` from that buffer (so the data is now on the heap).
- We zero out the stack-allocated buffer using https://crates.io/crates/zeroize so it is not optimized away by the compiler.
- We invoke `.leak()` on the `String` so we can treat its contents as a `&'static str`, as it will live for the rest of the process.
- On UNIX, we `mlock(2)` the memory backing the `&'static str`.
- When using the `&'static str` when building an HTTP request, we use `HeaderValue::from_static()` to avoid copying the `&str`.
- We also invoke `.set_sensitive(true)` on the `HeaderValue`, which in theory indicates to other parts of the HTTP stack that the header should be treated with "special care" to avoid leakage:

https://github.com/hyperium/http/blob/439d1c50d71e3be3204b6c4a1bf2255ed78e1f93/src/header/value.rs#L346-L376
