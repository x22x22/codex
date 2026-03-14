# Flaky Test Triage

## Objective

Eliminate flaky tests without disabling, quarantining, or skipping them, then reach five consecutive commits on this PR where the full GitHub CI test suite passes.

## Method

1. Sample the latest 30 merged PRs on `main`.
2. Inspect GitHub Actions history for failures in those PRs.
3. Treat tests that fail in isolated PR runs, but not broadly across the full suite, as flaky candidates.
4. Make the smallest deterministic fix possible.
5. Use PR CI as the source of truth for verification.

## Initial CI Evidence

Recent `rust-ci` failures on unrelated merged PRs repeatedly clustered in `Tests` jobs rather than lint or build jobs. The strongest pattern so far is Windows test failures:

| Run ID | PR | Result | Failing job(s) observed |
| --- | --- | --- | --- |
| `23078147759` | `#14645` | failure | `Tests — windows-x64 - x86_64-pc-windows-msvc` |
| `23078085247` | `#14639` | failure | `Tests — windows-x64 - x86_64-pc-windows-msvc` |
| `23075863238` | `#14633` | failure | `Tests — windows-x64 - x86_64-pc-windows-msvc`, `Tests — windows-arm64 - aarch64-pc-windows-msvc` |
| `23074360184` | `#14631` | failure | `Tests — windows-x64 - x86_64-pc-windows-msvc` |
| `23074136776` | `#14622` | failure | `Tests — windows-x64 - x86_64-pc-windows-msvc` |
| `23073664636` | `#14618` | failure | `Tests — windows-x64 - x86_64-pc-windows-msvc` |

Older failures also appeared on Linux, but the repeated cross-PR signal is strongest on Windows test jobs.

## Known Related History

- Merged PR `#14518` (`Add diagnostics for read_only_unless_trusted timeout flake`) targeted `codex-core::all suite::approvals::approval_matrix_covers_all_modes`.
- That change increased the timeout used by some approval-related shell events to `5_000ms`, which is the maximum allowed for this effort.
- The test still remains a likely candidate because the earlier change addressed a symptom instead of making the behavior deterministic.

## Current Hypotheses

1. Approval-related tests in `codex-rs/core/tests/suite/approvals.rs` still have timing-sensitive behavior, especially in cross-platform CI.
2. Windows-specific approval UI tests in `codex-rs/tui/src/chatwidget/tests.rs` may depend on partially implicit sandbox state and can fail intermittently on Windows runners.

## First Fix Landed

- Replace the approval-matrix write-file command from shell redirection (`printf > file && cat file`) with a deterministic `python3 -c` file write/readback command.
- Keep targeted scenario diagnostics in the matrix so CI logs include the exact command, exit code, and stdout when a scenario fails again.
- Rationale: the known `read_only_unless_trusted_requires_approval` flake was previously "fixed" by increasing timeout budget. This change removes shell-redirection timing sensitivity instead of stretching the timeout further.

## Current Fix In Progress

- Harden `permissions_selection_can_disable_smart_approvals` in `codex-rs/tui/src/chatwidget/tests.rs`.
- Seed the popup into Smart Approvals mode explicitly, then assert the selected row before and after navigation instead of assuming the initial cursor position.
- Rationale: recent merged PR `#14645` fixed another Smart Approvals popup test that was implicitly relying on selection state. This remaining test still used the same brittle pattern.

## Constraints

- Do not run tests locally.
- Do not increase timeouts beyond `5_000ms`.
- Do not disable, quarantine, or skip tests.
- Keep fixes minimal and deterministic.

## Commit CI Ledger

| Commit | Purpose | PR CI result | Notes |
| --- | --- | --- | --- |
| `60f44b4d7` | PR bootstrap | partial pass | PR opened and non-Rust checks passed after rebasing to current `main`, but `rust-ci` skipped because only the tracking doc changed. This commit does not count toward the five full-suite green commits. |
| `b9c655ad4` | First full-suite flaky-test fix | full pass | Full PR CI passed on run `23078933382`, including `Tests — windows-x64 - x86_64-pc-windows-msvc` and `Tests — windows-arm64 - aarch64-pc-windows-msvc`. The approvals matrix write-file command now uses deterministic Python I/O instead of shell redirection. This is pass 1 of 5. |
