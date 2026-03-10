# Add Native ACP Support to Codex

This ExecPlan is a living document. Keep the sections Progress, Surprises & Discoveries, Decision Log, and Outcomes & Retrospective up to date as work proceeds.

No repo-specific `docs/PLANS.md` policy exists as of 2026-03-10, so this plan follows the shared ExecPlan template.

## Purpose / Big Picture

Add native Agent Client Protocol (ACP) support to Codex so ACP-aware editors and clients can drive Codex without a bespoke integration. The intended end state is a thin ACP adapter over the existing app-server v2/runtime stack, preserving current thread/turn/item semantics, approvals, and streaming behavior instead of creating a second direct integration into `codex-core`.

Someone can see this working when an ACP client can:

- `initialize` successfully and negotiate capabilities.
- create or load a session backed by a Codex thread.
- send `session/prompt` and receive streamed `session/update` notifications.
- handle permission/interactivity flows without losing parity with current app-server behavior.
- cancel or resume sessions without hanging or duplicating state.

## Progress

- [x] (2026-03-10 14:23Z) Completed initial discovery across ACP docs, app-server/app-server-protocol/mcp-server, and likely stakeholder surfaces.
- [x] (2026-03-10 20:31Z) Prototyped a native ACP adapter crate and CLI entrypoint as `codex acp-server`.
- [x] (2026-03-10 23:18Z) Added ACP session modes/config options, `session/set_mode`, stdio+HTTP `mcpServers` passthrough via request config overrides, text/image resource prompt support, and a distinct `SessionSource::Acp`.
- [x] (2026-03-10 23:18Z) Added local ACP transcript coverage for `initialize`, `session/new`, and `session/set_mode`.
- [ ] Flesh out the ACP-to-app-server v2 mapping table in the plan doc with the concrete implementation decisions now in code.
- [x] Add focused ACP integration tests that exercise stdio JSON-RPC flows against the in-process runtime.
- [x] Implement `session/set_mode` support instead of returning not implemented.
- [ ] Replace the current `mcpServers` stdio+HTTP passthrough with MCP-over-ACP session wiring for ACP-transport servers or make the unsupported capability boundary explicit in initialize.
- [x] Consider adding a distinct `SessionSource::Acp` once we are ready to pay the shared-protocol validation cost.
- [ ] Draft the ACP-to-app-server v2 mapping table and record protocol gaps, including `session/load`, `session/request_permission`, `session/set_mode`, and MCP-over-ACP.
- [ ] Decide whether to adopt the official ACP Rust SDK or a smaller transport-only adapter.
- [ ] Implement and validate stable-core ACP compliance.
- [ ] Implement and validate extended ACP support for session replay, modes, and MCP-over-ACP.
- [ ] Build interop coverage and documentation against official ACP SDKs/examples.

## Surprises & Discoveries

- Observation: `codex-app-server` already exposes nearly all of the interaction primitives ACP needs: lifecycle, streaming notifications, server-initiated approval requests, tool user-input requests, and MCP elicitation requests. Evidence: `codex-rs/app-server/README.md`, `codex-rs/app-server/src/bespoke_event_handling.rs`, `codex-rs/app-server-protocol/src/protocol/common.rs`.
- Observation: local embedders already reuse app-server semantics through the in-process runtime and `codex-app-server-client` instead of reaching into `core` directly. Evidence: `codex-rs/app-server/src/in_process.rs`, `codex-rs/app-server-client/src/lib.rs`.
- Observation: `codex-mcp-server` is prior art for an adapter layer, but it constructs its own `ThreadManager`, which is probably the wrong long-term pattern for a new native rich-client protocol. Evidence: `codex-rs/mcp-server/src/message_processor.rs`.
- Observation: ACP's stable core is JSON-RPC 2.0 with `initialize`, `session/new`, `session/prompt`, `session/cancel`, and `session/update`; `session/load` and `session/set_mode` are optional capabilities. Evidence: ACP docs in `initialization`, `session-setup`, and `session-modes` from `/agentclientprotocol/agent-client-protocol` via Context7.
- Observation: ACP includes client-advertised filesystem and terminal capabilities plus agent-driven `session/request_permission`, which overlap strongly with Codex approvals and `tool/requestUserInput`, but not one-to-one. Evidence: ACP docs in `initialization` and `tool-calls` from `/agentclientprotocol/agent-client-protocol` via Context7.
- Observation: ACP session loading is stronger than a simple resume. If Codex advertises `loadSession`, ACP expects replay via `session/update` before returning. Evidence: ACP `session-setup` docs from `/agentclientprotocol/agent-client-protocol` via Context7.
- Observation: I did not find an official ACP-wide conformance suite in the official docs I queried. The closest official validation assets are the Rust and TypeScript SDKs, their examples, and SDK test helpers. This is an inference from the available docs, not a guarantee that no such suite exists elsewhere. Evidence: ACP docs in `libraries/rust`, `libraries/typescript`, and `rust-sdk-v1` from `/agentclientprotocol/agent-client-protocol` via Context7.
- Observation: the current app-server API collapses `CoreSessionSource::Mcp` into API `SessionSource::AppServer`, so ACP likely needs its own session source if we want clean analytics/compliance attribution. Evidence: `codex-rs/app-server-protocol/src/protocol/v2.rs`.
- Observation: ACP richer elicitation and several session-management capabilities are still evolving via RFDs, so the implementation should separate stable compliance from draft/extended support. Evidence: ACP `elicitation`, `session-stop`, `session-info-update`, and `mcp-over-acp` RFDs from `/agentclientprotocol/agent-client-protocol` and subagent research.

## Decision Log

- Decision: plan around an ACP adapter layered on top of app-server v2 rather than a new direct `core` integration. Rationale: app-server already owns the stable rich-client contract, state management, approvals, and backpressure. Date/Author: 2026-03-10 / Codex.
- Decision: treat `codex-app-server-client` as the preferred internal seam for a native ACP implementation. Rationale: it preserves app-server semantics while avoiding a second process boundary and already supports typed requests plus server-request resolution. Date/Author: 2026-03-10 / Codex.
- Decision: engage app-server owners first, especially Owen Lin, with Michael Bolin and Celia Chen as key follow-ups for runtime-contract stability and interactivity. Rationale: git history and doc ownership point there more strongly than any formal `CODEOWNERS` file. Date/Author: 2026-03-10 / Codex.
- Decision: explicitly split delivery into stable-core ACP compliance and extended ACP support for evolving features. Rationale: this lets Codex make a credible compliance claim for the stable protocol while still planning for draft features like richer elicitation and broader session management. Date/Author: 2026-03-10 / Codex.

## Outcomes & Retrospective

The first implementation slice is now in place:

- a new `codex-rs/acp-server` crate starts the in-process app-server runtime through `codex-app-server-client`.
- `codex acp-server` is wired as a new CLI entrypoint.
- the adapter handles ACP `initialize`, `session/new`, `session/load`, `session/prompt`, `session/cancel`, and `session/set_mode`.
- app-server notifications are translated into ACP `session/update` events for agent text, plans, command/file tool calls, and tool call completion updates.
- app-server approval requests for command execution and file changes are translated into ACP `session/request_permission`.
- ACP session setup now returns modes plus config options for mode/model/thought level derived from app-server collaboration mode and model APIs.
- ACP `mcpServers` declarations are now translated into Codex config overrides for stdio and streamable HTTP MCP servers.
- ACP text resources and image resources can now be converted into Codex input items, so the adapter advertises embedded-context and image prompt support.
- ACP sessions are now recorded with a distinct `SessionSource::Acp`, and app-server schema fixtures were regenerated accordingly.
- local transcript coverage now exercises `initialize`, `session/new`, and `session/set_mode` against the real stdio server binary.

Current known gaps after the first slice:

- ACP transport MCP servers from the MCP-over-ACP RFD are still rejected; only stdio and HTTP declarations are currently translated.
- richer elicitation remains unsupported because ACP `session/request_permission` cannot carry MCP form/URL elicitation content; unsupported server requests are still rejected back to app-server.
- compliance is still demonstrated only with local crate/unit/protocol/CLI tests, not official ACP SDK counterpart interop.

The main remaining unknowns are whether the official ACP Rust SDK fits cleanly into Codex's runtime architecture, how far to go on MCP-over-ACP in the first user-facing milestone, and whether a distinct ACP session source should be introduced in the shared protocol soon or deferred until the adapter semantics settle.

## Context and Orientation

The most relevant local layers are:

- `codex-rs/app-server-protocol/src/protocol/common.rs`
  Defines the JSON-RPC request, server request, and notification surface for app-server v2, including approval, tool-input, and elicitation requests.
- `codex-rs/app-server-protocol/src/protocol/v2.rs`
  Defines the typed v2 payloads and session/thread metadata exposed over app-server.
- `codex-rs/app-server/src/message_processor.rs`
  Owns initialization, capability gating, and dispatch for app-server requests.
- `codex-rs/app-server/src/bespoke_event_handling.rs`
  Translates core runtime events into app-server notifications and server requests. This is where user-input, approval, and MCP elicitation flows are already bridged.
- `codex-rs/app-server/src/in_process.rs`
  Hosts the app-server runtime in-process with bounded queues and server-request reply plumbing.
- `codex-rs/app-server-client/src/lib.rs`
  Provides the reusable in-process client facade that TUI and exec already use.
- `codex-rs/mcp-server/src/message_processor.rs`
  Shows how a protocol adapter can sit above the shared runtime, but also shows the maintenance cost of duplicating lifecycle handling.
- `codex-rs/app-server/tests/suite/v2/`
  Provides the best existing model for request/notification regression coverage.
- `codex-rs/app-server-test-client/src/lib.rs`
  Provides a practical harness for app-server transport tests and scripted interaction flows.
- `codex-rs/cli/src/main.rs`
  Defines top-level CLI entrypoints; ACP support will likely need a new subcommand or server mode here.

Relevant ACP concepts:

- `initialize` negotiates protocol version plus client and agent capabilities.
- `session/new` and optional `session/load` establish or resume a conversation.
- `session/prompt` and `session/cancel` drive one user turn and interrupt it.
- `session/update` streams agent message chunks, plan updates, tool activity, diffs, and mode changes.
- `session/request_permission` asks the client to surface approval options and return a user choice.
- `session/set_mode` is optional and adjacent to Codex collaboration modes.
- MCP-over-ACP allows clients to declare ACP-managed MCP servers and expects the agent to connect to them directly.
- draft session-management and elicitation extensions should remain behind explicit seams until the protocol settles.

## Plan of Work

Requirements driving scope:

- Reuse the existing app-server/runtime contract instead of duplicating stateful logic in a parallel ACP-specific core integration.
- Preserve interaction parity for approvals, request-user-input, cancellation, and streaming updates.
- Keep analytics/compliance attribution explicit so ACP traffic is distinguishable from MCP/app-server traffic.
- Ship with an interop harness because the official ACP docs do not currently point to a dedicated conformance suite.
- Support the stable ACP core well enough to make a concrete compliance claim, then layer in extended support for `session/load`, `session/set_mode`, and MCP-over-ACP.

Sequence of work:

First, produce a protocol mapping document that aligns ACP requests and notifications with app-server v2 methods, notifications, and server requests. This must explicitly cover `initialize`, `session/new`, `session/load`, `session/prompt`, `session/cancel`, `session/update`, `session/request_permission`, `session/set_mode`, and MCP-over-ACP, and it must call out mismatches such as ACP client capabilities (`fs`, `terminal`) and ACP-managed MCP servers from `session/new`.

Next, decide whether to build on the official ACP Rust SDK or implement only the ACP wire protocol/transport glue manually. The default should be to use the official SDK if it cleanly supports Codex's bounded async runtime, server-initiated interactions, and a stable test harness. If the SDK makes the embedding awkward, keep a very thin ACP adapter crate with minimal protocol code and put all business logic behind `codex-app-server-client`.

Then, create a new crate, tentatively `codex-rs/acp-server`, and expose it through the CLI, likely as `codex acp-server`. This crate should start the in-process app-server runtime through `codex-app-server-client`, stamp a distinct `SessionSource::Acp` if approved, and translate ACP messages into typed app-server calls and events. The adapter should own only translation, capability negotiation, and session bookkeeping required by ACP; it should not own core conversation logic.

After the transport skeleton works, implement the stable ACP core:

- `initialize` mapped to ACP protocol negotiation plus a derived Codex capability view.
- `session/new` mapped to `thread/start`.
- `session/prompt` mapped to `turn/start`.
- `session/cancel` mapped to `turn/interrupt`.
- `session/update` synthesized from app-server `ServerNotification` events and, where needed, `ServerRequestResolved`.
- `session/request_permission` mapped from app-server approval and elicitation flows with ACP-native option/outcome semantics.

Then implement the extended ACP slice needed for a stronger compliance story:

- `session/load` mapped to persisted Codex sessions with full ACP-style replay through `session/update` before the response completes.
- `session/set_mode` mapped to collaboration modes or an ACP-specific mode layer only if the mapping remains explicit and user-visible.
- MCP-over-ACP support for ACP-declared MCP servers passed in `session/new`, reusing existing Codex MCP connection machinery where possible.
- Draft-rich interactivity support behind explicit feature gates so ACP elicitation can evolve without destabilizing the stable core.

Finally, add test coverage and docs. Mirror the style of `codex-rs/app-server/tests/suite/v2/`, add ACP interop tests that run against a real adapter instance, and update user-facing docs under `codex-rs/docs/` plus CLI help text in `codex-rs/cli/src/main.rs`. If shared app-server v2 payloads change, update the app-server README and regenerate schema fixtures.

## Concrete Steps

Discovery and design:

1. From repo root, inspect the current rich-client boundary:
   `rg -n "initialize|ServerRequest|ServerNotification|requestUserInput|ElicitationRequest|RequestPermissions" codex-rs/app-server codex-rs/app-server-protocol codex-rs/app-server-client`
2. Review the lifecycle docs:
   `sed -n '1,260p' codex-rs/app-server/README.md`
3. Review the protocol surfaces:
   `sed -n '430,770p' codex-rs/app-server-protocol/src/protocol/common.rs`
   `rg -n "SessionSource" codex-rs/app-server-protocol/src/protocol/v2.rs`
4. Review ACP docs through Context7 before implementation begins:
   resolve `/agentclientprotocol/agent-client-protocol`
   query for `initialization`, `session-setup`, `prompt-turn`, `session-modes`, and `request_permission`
5. Produce an explicit compliance matrix with rows for stable-core ACP, optional ACP features, and draft/RFD features.

Implementation:

1. Create the adapter crate:
   `cargo new -w codex-rs/acp-server --bin`
2. Add workspace wiring in `codex-rs/Cargo.toml` and CLI wiring in `codex-rs/cli/src/main.rs`.
3. Implement ACP connection/session translation on top of `codex-app-server-client`.
4. Add focused tests for the new crate:
   `cargo test -p codex-acp-server`
5. If app-server protocol or docs change, regenerate or refresh the relevant artifacts:
   `just write-app-server-schema`
6. Run ACP SDK/example interop checks for at least one official ACP client or harness implementation.

Expected implementation proof points:

- ACP `initialize` returns negotiated capabilities and agent metadata.
- ACP `session/new` yields a persistent Codex-backed session id.
- ACP `session/prompt` streams incremental `session/update` events and completes cleanly.
- ACP permission prompts round-trip with ACP-native option and outcome shapes and without orphaned pending requests.
- ACP `session/load` replays prior conversation history as `session/update` events before returning when `loadSession` is advertised.
- ACP mode changes behave deterministically and visibly when `session/set_mode` is advertised.
- ACP-declared MCP servers can be connected and used through the ACP session lifecycle when MCP-over-ACP is enabled.

## Validation and Acceptance

Acceptance criteria:

- An ACP client can complete `initialize`, `session/new`, and `session/prompt` against Codex without using MCP as a wrapper transport.
- Streaming updates include enough fidelity to render agent text, plans, tool calls, tool-call updates, and diffs in an ACP client.
- At least one sensitive operation path exercises ACP permission prompting and maps cleanly to existing Codex approval semantics.
- Cancellation ends the active prompt turn and results in a stable terminal state instead of a stuck pending turn.
- A previously created session can be loaded and its prior history replayed in ACP shape when `loadSession` is advertised.
- If `session/set_mode` is advertised, the mode list and mode transitions conform to ACP expectations and remain aligned with Codex policy behavior.
- If MCP-over-ACP is advertised, ACP-provided MCP server declarations connect successfully and do not require a parallel runtime path.
- The implementation ships with a written compliance matrix that marks which stable, optional, and draft ACP features are supported.

Proof artifacts to capture:

- A saved ACP transcript showing `initialize`, `session/new`, `session/prompt`, and streamed `session/update`.
- A saved ACP transcript covering one permission prompt round-trip.
- A saved ACP transcript for `session/load` showing replay before completion.
- A saved ACP transcript for `session/set_mode`, if supported.
- A saved ACP transcript for one MCP-over-ACP session, if supported.
- Test output from `cargo test -p codex-acp-server`.
- Interop logs or transcripts from at least one official ACP SDK/example counterpart.
- If shared app-server types change, a diff to the regenerated schema fixtures and README docs.

Suggested test strategy:

- Unit tests for ACP-to-app-server mapping functions.
- Integration tests that run the ACP adapter against the in-process app-server runtime.
- Replay-focused tests for `session/load` so ordering and completion semantics are pinned down.
- Permission-shape tests to verify ACP option IDs, kinds, and selected/cancelled outcomes.
- Mode tests to verify `availableModes`, `currentModeId`, and `session/set_mode` behavior if advertised.
- MCP-over-ACP tests that verify agent-side connection setup from ACP session declarations.
- Interop smoke tests using the official ACP Rust or TypeScript examples/SDKs as counterpart clients where practical.

## Idempotence and Recovery

The implementation should be additive:

- the new ACP adapter must live behind a dedicated crate and CLI entrypoint.
- no existing `app-server`, `mcp-server`, TUI, or exec flows should change behavior unless they share a deliberate supporting abstraction.
- if the ACP prototype proves unstable, disable the CLI entrypoint and keep the internal mapping code out of existing startup paths.

Recovery path:

- revert the ACP crate and CLI entrypoint if the adapter destabilizes startup or transport logic.
- if a shared app-server abstraction becomes too ACP-specific, move that logic back to the adapter boundary and restore the previous app-server surface.

## Artifacts and Notes

Local references:

- `codex-rs/app-server/README.md`
- `codex-rs/app-server-protocol/src/protocol/common.rs`
- `codex-rs/app-server-protocol/src/protocol/v2.rs`
- `codex-rs/app-server/src/bespoke_event_handling.rs`
- `codex-rs/app-server/src/in_process.rs`
- `codex-rs/app-server-client/src/lib.rs`
- `codex-rs/mcp-server/src/message_processor.rs`
- `codex-rs/core/src/connectors.rs`
- `codex-rs/app-server/tests/suite/v2/`
- `codex-rs/app-server-test-client/src/lib.rs`

External ACP references consulted through Context7:

- `/agentclientprotocol/agent-client-protocol` `docs/protocol/initialization.mdx`
- `/agentclientprotocol/agent-client-protocol` `docs/protocol/session-setup.mdx`
- `/agentclientprotocol/agent-client-protocol` `docs/protocol/prompt-turn.mdx`
- `/agentclientprotocol/agent-client-protocol` `docs/protocol/session-modes.mdx`
- `/agentclientprotocol/agent-client-protocol` `docs/protocol/tool-calls.mdx`
- `/agentclientprotocol/agent-client-protocol` `docs/rfds/mcp-over-acp.mdx`
- `/agentclientprotocol/agent-client-protocol` `docs/rfds/elicitation.mdx`
- `/agentclientprotocol/agent-client-protocol` `docs/rfds/session-stop.mdx`
- `/agentclientprotocol/agent-client-protocol` `docs/rfds/session-info-update.mdx`
- `/agentclientprotocol/agent-client-protocol` `docs/libraries/rust.mdx`
- `/agentclientprotocol/agent-client-protocol` `docs/libraries/typescript.mdx`
- `/agentclientprotocol/agent-client-protocol` `docs/rfds/rust-sdk-v1.mdx`

Stakeholder starting point:

- Primary discussion: Owen Lin
- Secondary discussion: Michael Bolin, Celia Chen
- Focus area for initial review: app-server v2 lifecycle, approval semantics, and analytics/compliance identity for ACP sessions

## Interfaces and Dependencies

Expected local modules and crates:

- `codex-app-server-client` as the primary internal runtime seam.
- `codex-app-server` and `codex-app-server-protocol` for any shared lifecycle or schema extensions.
- `codex-cli` for surfacing a user-facing ACP server command.
- `codex-protocol::protocol::SessionSource` if ACP needs a distinct session identity.
- `codex-core` MCP connector plumbing if MCP-over-ACP needs to reuse the existing connector stack.
- `codex-app-server/tests` and `codex-app-server-test-client` for reusable harness patterns.

Expected external dependencies:

- ACP official Rust SDK if it fits Codex's embedding model.
- Otherwise, no large ACP-specific runtime dependency beyond JSON-RPC/transport helpers already compatible with the workspace.
