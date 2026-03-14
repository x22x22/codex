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

- Make the generic permissions-popup helper recognize `Read Only`, which is a real preset on Windows.
- Replace Windows-specific hard-coded navigation counts with label-driven movement in the permission history snapshot tests.
- Replace the remaining Windows-only extra navigation in the full-access confirmation/history test, and use the same label-driven movement in the adjacent Smart Approvals popup tests.
- Follow-up on `87941e5d7`: the helper compared the entire selected row with `contains(label)`, which made `Smart Approvals` look like `Default` because its description text says "Same workspace-write permissions as Default...". Tighten the helper to match the selected preset name at the start of the row instead of any substring in the rendered line.
- Follow-up on `13c9d91b0`: the cross-platform helper bug is fixed, but the default unelevated Windows popup still opens on `Read Only` instead of `Default`. The remaining Windows-only failures are in tests that added exact initial-selection assertions while assuming the popup would start on `Default`.
- Rationale: commit `e96d895c9` still failed on both Windows `Tests` jobs in run `23080608881`. The remaining common pattern is Windows-only popup navigation that assumes a fixed number of `Up`/`Down` presses even though the Windows menu includes `Read Only` and selection wraps.

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
| `5dbb9c004` | Second full-suite flaky-test fix | full pass | Full PR CI passed on run `23079410130`, including both Windows test jobs. `permissions_selection_can_disable_smart_approvals` now seeds Smart Approvals mode explicitly and asserts the popup selection before and after navigation. This is pass 2 of 5. |
| `dc8d5d46d` | Harden history-cell permission selection assertions | superseded | `rust-ci` kept running, but `Bazel (experimental)` was cancelled by workflow concurrency after later commits landed on the PR branch. This SHA cannot be counted until its cancelled Bazel run is backfilled. |
| `1b6e21ccc` | Pin permission history snapshots to concrete presets | superseded | Same state as `dc8d5d46d`: `Bazel (experimental)` was cancelled by PR-level workflow concurrency, so this SHA is not countable yet. |
| `fc98d21ad` | Select Smart Approvals in session-configured popup tests | failed | `Bazel (experimental)` failed on run `23079852646` across macOS and Linux. Investigation narrowed the likely issue to the overly broad `selected_popup_line()` helper introduced in this commit. |
| `a30e8e2ec` | Narrow permissions popup selection helper | failed | `Bazel (experimental)` still failed on Linux in run `23080049381`. The helper narrowing was necessary, but Linux snapshots showed the generic `(current)` assertions were still too strict for the default/full-access permission history tests. |
| `8bc3d489a` | Relax popup current-label assertions | failed | `rust-ci` failed on run `23080137075` in both Windows test jobs after Linux and Bazel turned green. The remaining Windows-only issue is likely the `Read Only` preset and wrapping popup navigation in the generic permission history tests. |
| `e96d895c9` | Stabilize Windows permissions popup navigation | failed | `rust-ci` failed on run `23080608881` in both Windows test jobs again. The history snapshot tests were fixed, but another Windows-only extra `Down` remained in `permissions_full_access_history_cell_emitted_only_after_confirmation`, and adjacent popup-selection tests still relied on fixed-step navigation. |
| `87941e5d7` | Harden remaining Windows permissions popup navigation | failed | `rust-ci` and Bazel both regressed on run `23081011779`. The new generic navigation helper matched the entire selected row with `contains(label)`, so the Smart Approvals description text incorrectly satisfied `Default`. |
| `13c9d91b0` | Avoid false matches in permissions popup helper | failed | Run `23081377233` fixed the Linux/macOS/Bazel regression, but both Windows `Tests` jobs still failed. Signed log downloads from both `results-receiver.actions.githubusercontent.com` and the Azure blob log URL hit TLS EOFs in this environment, so the remaining diagnosis comes from CI history plus the test diff: the default unelevated Windows popup still opens on `Read Only`, and the new exact-selection assertions were assuming `Default`. |
| `c3b8a0ebf` | Correct Windows popup selection assertions | full pass | Full PR CI passed on run `23081835533`, including both Windows `Tests` jobs and Bazel. The Windows-only initial-selection assertions now expect `Read Only` when the unelevated popup opens on Windows. This restarts the passing streak at 1 of 5 after the intervening failed commits. |
| `9835ec89d` | Record validation pass 1 | full pass | Full PR CI passed on run `23082212165`, including the rerun of `Tests — windows-arm64 - aarch64-pc-windows-msvc` on job `67054619213`. The branch now has 2 consecutive full-suite green commits after `c3b8a0ebf`. |
