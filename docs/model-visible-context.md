# Model-visible context fragments

Codex injects model-visible context through two turn-state envelopes:

- the developer envelope, rendered as a single `developer` message
- the contextual-user envelope, rendered as a single `user` message whose contents are contextual state rather than real user intent

Both envelopes are assembled from the single ordered fragment registry in
[`codex-rs/core/src/model_visible_fragments.rs`](/Users/ccunningham/code/codex-worktree-tria/codex-rs/core/src/model_visible_fragments.rs).
Envelope builders normalize text fragment boundaries by inserting `\n\n`
between adjacent text content items, so fragments do not run together in the
model-visible token stream.

## Canonical rules

1. All model-visible injected context must be represented as a typed `ModelVisibleContextFragment`.
2. Turn-state context assembly always produces exactly two envelopes: one developer message and one contextual-user message.
3. There is one blessed place to define and register current fragment types: [`model_visible_fragments.rs`](/Users/ccunningham/code/codex-worktree-tria/codex-rs/core/src/model_visible_fragments.rs).
4. Contextual-user fragments must provide stable detection so history parsing can distinguish contextual state from true user intent.
5. Fragments derived from durable/current turn state that should survive history-mutating flows (resume/fork/compaction/backtracking) via re-diffing should implement `ModelVisibleContextFragment::build(...)`.
6. Do not hand-construct model-visible `ResponseItem::Message` payloads in new code. Use fragment conversion (`into_message` / `into_response_input_item`) and the shared envelope builders.

Rule 2 applies to turn-state context assembly (`build_initial_context` /
`build_settings_update_items`). Runtime or session-prefix events may still
inject standalone fragment messages, but those fragment types still belong in
the central registry so rendering and contextual-user detection remain
standardized.

## Blessed path

When adding new model-visible context:

1. Define a typed fragment type in [`model_visible_fragments.rs`](/Users/ccunningham/code/codex-worktree-tria/codex-rs/core/src/model_visible_fragments.rs).
2. Implement `ModelVisibleContextFragment` for it.
3. Set the fragment `type Role` to the correct developer or contextual-user role.
4. If it is a contextual-user fragment:
   - define `contextual_user_markers()` when marker-based detection/wrapping is sufficient
   - override `matches_contextual_user_text()` only when matching is genuinely custom (for example AGENTS.md)
5. If it is derived from current/persisted turn state and should participate in initial-context assembly and turn-to-turn diffing, implement `build(...)`.
6. Register the fragment exactly once in `REGISTERED_MODEL_VISIBLE_FRAGMENTS`, in the rough order it should appear in model-visible context.

That single registration powers:

- contextual-user history detection
- developer-envelope turn-state assembly
- contextual-user-envelope turn-state assembly

This is intentionally stricter than “implement the trait somewhere.” A fragment
definition is not integrated until it is registered.

## Choosing an envelope

Use the developer envelope for developer-role guidance:

- permissions / approval policy instructions
- collaboration-mode developer guidance
- model switch and realtime notices
- personality guidance
- subagent roster and subagent notifications
- other developer-only instructions

Use `CustomDeveloperInstructions` only for custom developer override text (for
example config/app-server `developer_instructions` values).

For system-generated developer guidance (permissions, collaboration-mode
wrappers, realtime notices, personality wrappers, model-switch notices), use
typed developer fragments whose text comes from the neutral
`developer_*_text` helpers in `codex_protocol::models`.

Use the contextual-user envelope for contextual state that should not count as
real user turns:

- AGENTS / user instructions
- environment context

Use standalone contextual-user fragment messages for runtime contextual state or
markers that also should not count as real user turns:

- plugin instructions
- skill instructions
- user shell command records
- turn-aborted markers

Contextual-user fragments must have stable detection because history parsing
uses it to distinguish contextual state from real user intent.

Use `<environment_context>` only for environment facts derived from turn/session
state (`TurnContext`) that may need turn-to-turn diffing. Today that includes
`cwd`, `shell`, optional `current_date`, optional `timezone`, and optional
network allow/deny domain summaries. Do not put developer policy/instructions
or plugin/skill metadata into `<environment_context>`; those belong in their
own typed fragments.

## Build semantics

`ModelVisibleContextFragment::build(...)` is the canonical hook for turn-state
fragments.

It receives:

- the current `TurnContext`
- an optional `reference_context_item`, which is the last persisted turn-state snapshot already represented in model-visible history
- `TurnContextDiffParams` for shared runtime inputs such as shell rendering, previous-turn bridge state, exec-policy rendering context, and feature gating flags

Turn-state fragments should return:

- `Some(fragment)` when current model-visible state should be injected
- `None` when no model-visible update is needed

Runtime/session-prefix fragments that are not built from turn state should leave
the default `build(...) -> None`.

## History behavior

Developer fragments do not need contextual-user detection because they are
already separable by message role.

Contextual-user fragments do need contextual-user detection because they share
the `user` role with real user turns, and history parsing / truncation must
avoid treating injected context as actual user input.

Current fragment types live in the registry. Historical wrappers that are no
longer current fragments should stay in a tiny separate compatibility shim near
the detection path rather than being added as fake current fragments.
