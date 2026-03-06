# Apps tool discovery

Searches over apps metadata with BM25 and exposes matching installed tools for the next model call, or installable apps when requested.

MCP tools of the apps ({{app_names}}) are hidden until you search for them with this tool (`search_tool_bm25`).

Follow this workflow:

1. Call `search_tool_bm25` with:
   - `query` (required): focused terms that describe the capability you need.
   - `limit` (optional): maximum number of tools to return (default `8`).
   - `mode` (optional): `available` or `installable`. Default is `available`.
2. Always search `mode: "available"` first.
3. If `available` finds the right tool, use the returned `tools` list to decide which Apps tools are relevant.
4. Matching available tools are added to available `tools` for the remainder of the current session/thread.
5. If `available` does not find the right tool and the user strongly wants a specific app, search again with `mode: "installable"`.
6. If `installable` finds the right app, call `tool_suggest` with the returned `connector_id` to prompt the user to install it.

Notes:
- Core tools remain available without searching.
- If you are unsure, start with `limit` between 5 and 10 to see a broader set of tools.
- In `available` mode, `query` is matched against Apps tool metadata fields:
  - `name`
  - `tool_name`
  - `server_name`
  - `title`
  - `description`
  - `connector_name`
  - input schema property keys (`input_keys`)
- In `installable` mode, `query` is matched against app metadata such as connector id, name, description, labels, categories, and plugin display names.
- If the needed app is already explicit in the prompt (for example `[$app-name](app://{connector_id})`) or already present in the current `tools` list, you can call that tool directly.
- Do not call app MCP tools for apps returned only by `installable` mode until the user installs them.
- Do not use `search_tool_bm25` for non-apps/local tasks (filesystem, repo search, or shell-only workflows) or anything not related to {{app_names}}.
