# Codex Auth Migration Plan

## Goal

Move auth and model-provider ownership into a new `codex-auth` crate while keeping `codex-core` as the thin public surface for the rest of the workspace.

Non-`core` crates should not end up in a state where they need to import `codex-auth` directly.

## Invariants

- `codex-auth` is the implementation owner for auth and provider concepts.
- `codex-core` is the public facade for normal consumers.
- Normal consumers keep importing auth/provider concepts from `codex-core`.
- Only boundary crates may depend on `codex-auth` directly.
  - Allowed: `codex-core`, `codex-login`, `codex-auth`
  - Not allowed: `tui`, `exec`, `app-server`, `chatgpt`, `cli`
- Avoid duplicating logic between `codex-core` and `codex-auth`.
- Prefer moving code first and redesigning APIs later.

## Ownership Boundary

### `codex-auth` owns

- auth state and persistence
- token parsing and refresh
- auth manager lifecycle
- auth storage backends
- model provider definitions
- built-in provider registry
- auth/provider request glue
- provider-aware remote model catalog behavior if it is truly auth/provider infrastructure

### `codex-core` owns

- config loading and orchestration
- app/session/thread behavior
- product policy
- model presentation and presets
- thin re-exports over `codex-auth`

## PR 1

### Scope

Create `codex-auth`, move foundational auth/provider code into it, and add `codex-core` re-exports in the same PR.

### Expected moves

- `core/src/token_data.rs`
- `core/src/model_provider_info.rs`

These should become `codex-auth` modules with minimal behavioral change.

Note:
`auth/storage` and the heavier auth manager implementation may stay in `codex-core` for PR 1 if moving them would drag `Config` or other core-only seams into `codex-auth` too early.

### Required facade work in the same PR

- Re-export moved items from `codex-core`
- Preserve existing import paths where practical
- Keep module-path compatibility for callers that currently use:
  - `codex_core::auth::...`
  - `codex_core::model_provider_info::...`

### Acceptance criteria

- `codex-auth` exists and owns the moved implementation
- `codex-core` still exposes the same public auth/provider concepts
- No normal consumer crate imports `codex-auth`
- No behavior change intended

### Checkpoints

- [ ] `core/src/lib.rs` re-exports moved auth/provider symbols
- [ ] compatibility module exists for `codex_core::model_provider_info::*`
- [ ] `core/Cargo.toml` depends on `codex-auth`
- [ ] no direct `codex-auth` dependency added to `tui`, `exec`, `app-server`, `chatgpt`, or `cli`
- [ ] moved code does not pull `codex-core` back into `codex-auth`

## PR 2

### Scope

Move implementation-boundary consumers and auth/provider glue to the new owner while keeping `codex-core` as the facade.

### Expected changes

- move `codex-login` to depend on `codex-auth` directly
- move auth/provider request glue that belongs with auth ownership
- update `codex-core` internals to consume `codex-auth` directly under the facade

### Likely files to review

- `login/src/lib.rs`
- `login/Cargo.toml`
- `core/src/api_bridge.rs`
- `core/src/client.rs`
- `core/src/thread_manager.rs`

### Acceptance criteria

- `codex-login` no longer depends on `codex-core` for auth ownership
- auth/provider glue lives with `codex-auth`
- public consumption for normal crates still goes through `codex-core`

### Checkpoints

- [ ] `login` depends on `codex-auth`, not `codex-core`, for auth concepts
- [ ] `core` still re-exports the same public surface
- [ ] no normal consumer crate was switched to importing `codex-auth`
- [ ] auth header/account-id/provider glue no longer has split ownership
- [ ] dependency direction is improved, not inverted again

## PR 3

### Scope

Move the auth/provider-owned model-management pieces and finish cleanup without dragging product policy out of `codex-core`.

### Expected changes

- move only the provider/auth-coupled parts of model management
- keep model presentation, presets, and app policy in `codex-core`
- remove leftover shims that are no longer needed, while keeping the public `codex-core` facade stable

### Likely files to review

- `core/src/models_manager/manager.rs`
- `core/src/models_manager/mod.rs`
- `core/src/models_manager/model_info.rs`

### Acceptance criteria

- provider/auth infrastructure no longer lives in `codex-core`
- `codex-core` still owns product policy and presentation
- normal consumer crates still do not need `codex-auth`

### Checkpoints

- [ ] only provider/auth infrastructure moved out of `models_manager`
- [ ] model presets and presentation remain in `codex-core`
- [ ] `codex-core` facade still covers auth/provider concepts for consumers
- [ ] no duplicate provider/auth logic remains across crates

## Watchlist

These are the highest-risk seams to review during the migration:

- `core/src/api_bridge.rs`
- `core/src/client.rs`
- `core/src/thread_manager.rs`
- `core/src/config/mod.rs`
- `core/src/models_manager/manager.rs`
- `login/src/lib.rs`
- any `codex_core::auth::...` imports in non-`core` crates

## Quick Rule For Future Work

If code answers one of these questions, it likely belongs in `codex-auth`:

- Who am I authenticated as?
- How are credentials stored or refreshed?
- What provider am I configured to use?
- How does auth interact with provider request construction?

If code answers one of these questions, it likely stays in `codex-core`:

- How should the app behave?
- How should models be presented to users?
- How should config be orchestrated for the product?
- How should threads, sessions, and UI flows be managed?
