# `/loop` Backed by Thread Jobs

## Summary

Implement `/loop` as a TUI command that creates and manages per-thread jobs.

Defaults locked in:

- `/loop <spec>` creates a job.
- Bare `/loop` lists jobs for the current thread and offers delete actions.
- The entire feature is gated behind the experimental feature flag `job_scheduler`.
- Natural-language parsing happens in the caller, not in core or app-server.
- `JobCreate` accepts structured inputs: `cron_expression`, `prompt`, and optional `run_once`.
- `thread/job/create` also accepts the optional `run_once` field, exposed on the v2 wire as `runOnce`.
- Each thread has a hard limit of 256 active jobs to prevent resource exhaustion.
- Jobs are runtime-only and do not persist across restart, resume, or fork.
- Conditions like “until tests pass” stay inside `prompt`; the model stops the job by calling `JobDelete`.

## Key Changes

- Add a job scheduler module in core, preferably `codex-rs/core/src/jobs.rs`, to own:
  - `Job { id, cron_expression, prompt, run_once, created_at, next_run_at, last_run_at, pending_run }`
  - due-run tracking, idle-thread dispatch, and `next_run_at` computation
- Gate all core, app-server, and TUI job functionality behind `Feature::JobScheduler` / the experimental feature key `job_scheduler`.
- Add three built-in tools in core:
  - `JobCreate { cron_expression: String, prompt: String, run_once: Option<bool> }`
  - `JobDelete { id: String }`
  - `JobList {}`
- Tool semantics:
  - `JobCreate` validates the structured job and registers it on the current thread.
  - `JobCreate` rejects creation when the thread already has 256 active jobs.
  - `JobDelete` removes one job by id on the current thread.
  - `JobList` returns all jobs on the current thread.
  - `run_once=true` means the job is deleted immediately after its execution is claimed, removed from `JobList` results, and the thread emits `thread/job/updated` with the new list.
- Job execution behavior in core:
  - due jobs run only when the owning thread is idle
  - if a job becomes due while busy, keep at most one pending firing
  - manual user turns and queued user prompts beat job runs
  - job-triggered turns inherit the current thread context, including plan mode, sandbox, approvals, and model settings
  - interrupts stop the current turn but do not implicitly delete jobs
- Job-triggered prompt execution stays off-transcript as a normal user-authored message. Clients should show explicit runtime/info events instead.
- Add app-server v2 RPCs in `codex-rs/app-server-protocol/src/protocol/v2.rs`:
  - `thread/job/create { threadId, cronExpression, prompt, runOnce? }`
  - `thread/job/delete { threadId, id }`
  - `thread/job/list { threadId }`
  - `thread/job/updated` notification with `threadId` and the full current list
  - when `job_scheduler` is disabled, job RPCs fail with a clear unsupported-feature error instead of silently no-oping
  - `thread/job/create` fails with a clear invalid-request or limit-exceeded error when the thread already has 256 active jobs

## Natural-Language Parsing

- The caller converts `/loop <spec>` into structured job params by issuing a small model parse request before calling `thread/job/create`.
- Clients should use the existing app-server side-prompt path for that parse request rather than inventing a separate parser-specific backend surface.
- Define a shared parse contract for clients:
  - input: raw `spec: String`
  - output: `{ cronExpression: String, prompt: String, runOnce?: bool }`
- Parsing prompt should be minimal and deterministic:
  - extract only schedule timing into `cronExpression`
  - keep all stop conditions and task intent inside `prompt`
  - set `runOnce=true` only when the spec clearly describes a one-shot run
  - if no explicit wall-clock schedule is present, return the special immediate-follow-up expression used by the scheduler
  - return structured JSON only, no prose
- TUI uses this parse step internally for `/loop <spec>`.
- Other clients can reuse the same prompt/contract, but core and app-server only validate the structured fields they receive.
- Parse failures or invalid structured output fail closed: do not create a job.

## Job Identity And Subagents

- Every job-triggered turn carries `currentJobId` in hidden turn context. The model uses that id when it decides the job should stop and calls `JobDelete(currentJobId)`.
- `currentJobId` is present only for turns fired by a job on that same thread. It must not be written into visible prompt text.
- Jobs are strictly thread-local:
  - parent and child threads have separate job lists
  - spawning, resuming, or forking a child does not copy the parent’s jobs
  - a child spawned from a parent job turn does not inherit the parent’s `currentJobId`
- Built-in job tools are available on any thread, including subagents, but always operate on that thread’s own jobs only. No cross-thread job mutation is allowed in v1.
- If a parent job spawns a child and the child concludes the parent’s stop condition is met, the child must report that back to the parent thread; it cannot directly delete the parent’s job.
- Closing a child thread implicitly drops that child’s runtime jobs with the thread.
- Multi-agent clients must treat `thread/job/updated` and job-fire events as thread-scoped so background child-thread jobs are not mistaken for parent-thread activity.

## TUI Behavior

- Add `SlashCommand::Loop` in `codex-rs/tui/src/slash_command.rs`.
- `/loop <spec>`:
  - runs the parse prompt
  - maps the structured result into `thread/job/create`
  - shows a confirmation/info event with the normalized job
- Bare `/loop` lists current jobs for the active thread and supports deletion.
- While a turn is running, `/loop <spec>` still works so the next job can be armed immediately.
- In multi-agent views, job events and lists should remain attached to the owning thread so users can tell which agent armed or fired a job.
- TUI only shows `/loop` when `job_scheduler` is enabled for the current session/config.

## Test Plan

- Core:
  - create, list, and delete multiple jobs on one thread
  - reject the 257th active job on a thread
  - validate `cron_expression` and `run_once`
  - ensure due jobs fire once when idle and do not backlog endlessly while busy
  - ensure a `run_once` job is deleted after execution and disappears from subsequent list results
  - ensure job-triggered turns include hidden `currentJobId`
  - ensure non-job turns do not include `currentJobId`
  - ensure parent `currentJobId` is not inherited by spawned child threads
  - ensure child-thread jobs are isolated from parent-thread jobs
  - ensure closing a child thread drops its runtime jobs
  - ensure restart, resume, and fork clear runtime jobs
- App-server:
  - create/list/delete RPC round-trips, including omitted and explicit `runOnce`
  - `thread/job/create` returns the expected error once the thread reaches 256 active jobs
  - `thread/job/updated` emits the full current job set after creates, deletes, and run-once deletion after execution
  - thread-scoped notifications stay associated with the correct parent or child thread
  - disabled-feature behavior rejects job RPCs cleanly
- TUI:
  - `/loop <spec>` issues the parse request and converts the structured result into the expected create RPC
  - parser snapshots cover recurring, immediate-follow-up, and one-shot examples
  - bare `/loop` lists jobs for the active thread only
  - delete actions call the delete RPC on the active thread
  - job-fire info events render with the owning thread identity in multi-agent scenarios
  - `/loop` is hidden or rejected when `job_scheduler` is disabled

## Assumptions

- `job` is the public product and API term; the underlying module/file can still be named `scheduled.rs` if that keeps implementation boundaries cleaner.
- Core and app-server do not parse freeform schedule text.
- The parser uses a model call rather than handwritten NLP rules.
- The scheduler supports a special non-cron expression for immediate post-turn execution, because plain cron is a bad fit for “run again right after this turn.”
- Cron expressions are evaluated in the local timezone of the process hosting the thread unless the existing scheduler infrastructure already defines a better thread-level timezone source.
- The experimental gate is named exactly `job_scheduler`.
