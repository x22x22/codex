---
name: codex-applied-devbox
description: Sync a local Codex worktree from `~/code/codex-worktrees/` to a mirrored path on a remote host, then run a reproducible remote build or exec command there.
---

# Codex Applied Devbox

Use this skill when you want local file editing/search on your laptop, but want
the actual build or execution to happen on a remote host such as `dev`.

This skill assumes:

- remote host alias: `dev`
- local Codex worktree root: `~/code/codex-worktrees`
- remote mirror root: `/tmp/codex-worktrees`

If the box itself needs to be created, resumed, suspended, or inspected, use
the `applied-devbox` skill first.

## Objective

1. Create or reuse a local worktree under `~/code/codex-worktrees/`.
2. Mirror that worktree to the remote host under `/tmp/codex-worktrees/`.
3. Run one configurable remote Bazel command against the mirrored copy.
4. Keep the flow reproducible by excluding build artifacts and local repo state.

## Operator Defaults

When using this skill interactively, the operator should bias toward immediate
execution over setup-heavy preflights.

Default posture:

- If the user asks for a specific PR or branch, create a fresh worktree first.
- Do not spend time checking whether an equivalent worktree already exists
  unless the user explicitly asked to reuse one.
- Assume `dev` is reachable and run the sync directly; only debug SSH or remote
  prereqs after the real command fails.
- Avoid separate "can I reach the host?" or "does rsync exist remotely?"
  checks unless there is a known problem pattern.
- Prefer one end-to-end attempt over multiple speculative probes.

In practice, that means the operator should usually do this:

1. Fetch the requested PR or ref.
2. Create a new local worktree under `~/code/codex-worktrees/`.
3. Run `sync-worktree-and-run` immediately.
4. Only inspect host reachability, missing tools, or conflicting paths if that
   end-to-end run fails.

### PR Fast Path

For a request like "build PR 16620 on devbox", prefer this shape:

```bash
mkdir -p ~/code/codex-worktrees
git -C ~/code/codex fetch origin pull/16620/head
git -C ~/code/codex worktree add -b pr-16620 \
  ~/code/codex-worktrees/pr-16620 FETCH_HEAD
skills/codex-applied-devbox/scripts/sync-worktree-and-run \
  ~/code/codex-worktrees/pr-16620
```

This is intentionally direct. It skips separate validation steps and lets the
real sync/build path prove whether the environment is healthy.

## Key rule for concurrent builds

- Keep each worktree as its own Bazel workspace path.
- Let Bazel derive a separate `output_base` per worktree automatically.
- Reuse the shared caches from `.bazelrc`:
  - `~/.cache/bazel-disk-cache`
  - `~/.cache/bazel-repo-cache`
  - `~/.cache/bazel-repo-contents-cache`
- Do not force a shared `--output_base` across two live worktrees.

On `dev`, this has already been validated with two mirrored worktrees:

- both builds started at the same second
- each worktree got its own Bazel server and `output_base`
- both builds reused shared cache state and completed successfully

## Script

The script lives at:

`skills/codex-applied-devbox/scripts/sync-worktree-and-run`

Default behavior:

- host: `dev`
- local worktree root: `~/code/codex-worktrees`
- remote mirror root: `/tmp/codex-worktrees`
- remote command:
  `cd codex-rs && export PATH=$HOME/code/openai/project/dotslash-gen/bin:$HOME/.local/bin:$PATH && bazel build --bes_backend= --bes_results_url= //codex-rs/cli:cli`
- prints the exact copy-paste remote Codex Bazel run command for the mirrored checkout
- the helper command intentionally stays on the mirrored-worktree Bazel path and uses `//codex-rs/cli:codex`

Example:

```bash
skills/codex-applied-devbox/scripts/sync-worktree-and-run \
  ~/code/codex-worktrees/my-feature
```

This will mirror:

- local: `~/code/codex-worktrees/my-feature`
- remote: `/tmp/codex-worktrees/my-feature`

It will print:

```bash
ssh -t dev 'cd /tmp/codex-worktrees/my-feature/codex-rs && export PATH=$HOME/code/openai/project/dotslash-gen/bin:$HOME/.local/bin:$PATH && bazel run --bes_backend= --bes_results_url= //codex-rs/cli:codex --'
```

Custom host, remote root, and command:

```bash
skills/codex-applied-devbox/scripts/sync-worktree-and-run \
  --host dev \
  --remote-root /tmp/codex-worktrees \
  --command 'cd codex-rs && export PATH=$HOME/code/openai/project/dotslash-gen/bin:$HOME/.local/bin:$PATH && bazel build --bes_backend= --bes_results_url= //codex-rs/tui:tui' \
  ~/code/codex-worktrees/my-feature
```

## Recommended setup

1. Create the local worktree from your main Codex checkout.

```bash
mkdir -p ~/code/codex-worktrees
git -C ~/code/codex worktree add -b my-feature \
  ~/code/codex-worktrees/my-feature origin/main
```

2. Edit locally or fetch the PR/ref you want to test.

3. Sync and build remotely immediately:

```bash
skills/codex-applied-devbox/scripts/sync-worktree-and-run \
  ~/code/codex-worktrees/my-feature
```

4. Repeat sync/build as needed after local edits.

## Retrospective Notes

The main friction in a real run was not rsync itself. It was operator delay
before the first real attempt:

- checking whether a matching worktree already existed before simply creating
  the one needed for the task
- verifying host reachability before letting the real sync prove it
- remembering the mirrored remote path after the sync
- hand-writing the SSH command needed to run Codex in that mirrored checkout
- waiting through a cold Bazel build with no simple "jump in here yourself"
  command printed by the script

The current script update addresses the third and fourth issues by printing the
exact `ssh -t ...` command for running Codex in the mirrored checkout.
This skill update addresses the first two issues by telling the operator to
start the end-to-end flow sooner and only investigate after an actual failure.

The next improvements worth making, if you want this flow to feel faster and
more automatic, are:

- add `--pr <number>` so the script can fetch `pull/<n>/head` and create or
  reuse `~/code/codex-worktrees/pr-<n>` itself
- add `--tmux-window <name>` support so the remote command can start in a named
  tmux session/window and print the exact follow/log command
- add an optional "sync only changed files" mode driven by git status or
  `git diff --name-only` for large worktrees
- add an optional `--bazel-target <label>` shortcut so users do not have to
  remember the common labels

## Validated run paths on `dev`

What has been verified:

- `sync-worktree-and-run` can mirror the local worktree and complete a remote
  Bazel build with:
  `bazel build --bes_backend= --bes_results_url= //codex-rs/cli:cli`
- on current `main`, `bazel run --bes_backend= --bes_results_url=
  //codex-rs/cli:codex --` builds successfully on `dev`

Practical note:

- older pre-`#16634` checkouts could fail on `dev` when launching
  `//codex-rs/cli:codex`; treat current `main` as the baseline before carrying
  that older caveat forward

## Bazel defaults on the devbox

Use this decision rule:

- Default to Bazel for remote builds in mirrored worktrees.
- Keep the existing `.bazelrc` cache settings; they already share the useful
  cache layers across worktrees.
- On `dev`, clear the BES flags for routine builds:
  `--bes_backend= --bes_results_url=`
- Prepend both common Bazel locations to `PATH`:
  `export PATH=$HOME/code/openai/project/dotslash-gen/bin:$HOME/.local/bin:$PATH`
- Prefer labels that have already been validated on the host:
  - `//codex-rs/cli:cli`
  - `//codex-rs/tui:tui`
  - `//codex-rs/utils/absolute-path:absolute-path`

Current practical note:

- older pre-`#16634` checkouts could fail on `dev` when launching
  `//codex-rs/cli:codex`; re-test current `main` before treating that older
  caveat as still active

What is shared versus isolated:

- Shared across worktrees:
  - `~/.cache/bazel-disk-cache`
  - `~/.cache/bazel-repo-cache`
  - `~/.cache/bazel-repo-contents-cache`
  - the Bazel install base under `~/.cache/bazel/_bazel_dev-user/install`
- Still per worktree:
  - each `output_base`
  - each Bazel server
  - mutable workspace-specific state under
    `~/.cache/bazel/_bazel_dev-user/<hash>`

That means this setup saves disk space compared with giving every worktree its
own completely separate Bazel root, but it does not eliminate the large
per-worktree `output_base` directories.

## Fresh default devbox bootstrap

This was validated against a fresh box created with a temporary minimal config
override, not your personal `~/.config/applied-devbox/config.toml`.

Validated sequence:

1. Create a minimal config file locally and point `APPLIED_DEVBOX_CONFIG` at it.
   An empty file is enough if you want the CLI's built-in defaults without your
   personal apt/git/custom-setup additions.

2. Create the box:

```bash
APPLIED_DEVBOX_CONFIG=/tmp/applied-devbox-default-config.toml \
  a devbox new codex-bazel-0402-1800 \
  --sku cpu64 \
  --home-size 2Ti \
  --skip-secret-setup \
  --skip-tool-setup
```

If you expect large Bazel output trees or long-lived mirrored worktrees, prefer
`--sku cpu64 --home-size 2Ti` over the smaller defaults.

3. If the first `a devbox ssh` fails on websocket transport, establish
   connectivity with:

```bash
APPLIED_DEVBOX_CONFIG=/tmp/applied-devbox-default-config.toml \
  a devbox ssh codex-bazel-0402-1800 --no-ws --no-tmux -- bash -lc 'hostname && whoami'
```

After that, direct `ssh codex-bazel-0402-1800` was available on this machine.

4. Install `rsync` once on the new box:

```bash
ssh codex-bazel-0402-1800 'sudo apt-get update && sudo apt-get install -y rsync'
```

5. Run the mirrored Bazel build:

```bash
skills/codex-applied-devbox/scripts/sync-worktree-and-run \
  --host codex-bazel-0402-1800 \
  ~/code/codex-worktrees/my-feature
```

What was validated on the fresh box:

- the box came up from a default-style config override
- the first websocket-based SSH attempt failed, but `--no-ws` succeeded
- plain `ssh <box>` worked after the first successful `--no-ws` connection
- `rsync` was the only package that had to be installed manually
- Bazel was already available from the default OpenAI clone at
  `~/code/openai/project/dotslash-gen/bin`
- the first mirrored `//codex-rs/cli:cli` build completed successfully in
  `68.24s`

## Sync exclusions

The script excludes:

- `.git`
- `.sl`
- `.jj`
- `target`
- `node_modules`
- `.venv`, `venv`
- `dist`, `build`, `.next`
- `.pytest_cache`, `.mypy_cache`, `__pycache__`, `.ruff_cache`
- `.DS_Store`

## Cleanup

Remove a stale remote mirror:

```bash
ssh dev 'rm -rf /tmp/codex-worktrees/my-feature'
```

Remove the local worktree when finished:

```bash
git -C ~/code/codex worktree remove ~/code/codex-worktrees/my-feature
git -C ~/code/codex branch -D my-feature
```

## Guardrails

- Treat the local worktree as the editing source of truth.
- Treat the mirrored remote copy as disposable build state.
- Do not sync `.git` or build outputs.
- Keep the local worktree under `~/code/codex-worktrees/` so the mirror path is
  stable and easy to clean up.
