# Cross-Platform Screen Recording Service in `codex-app-server`

## Summary

- Add a process-scoped, in-process `ScreenRecordingManager` to `codex-app-server`, owned by `MessageProcessor` rather than a thread.
- Make `recording.screen.enabled` the single persistent on/off control. If app-server starts and `recording.screen.enabled = true`, it begins recording automatically without any extra app-server command.
- Keep v1 capture-only: capture all active displays at `1 fps`, store frames locally under `$CODEX_HOME/recording/screen_ephemeral`, prune anything older than `6 hours`.
- Keep `recording/screen/pause` and `recording/screen/resume` as runtime-only controls. They temporarily stop or resume capture but do not change `recording.screen.enabled`.
- Use one app-server API across macOS, Windows, and Linux; unsupported or unpermissioned hosts must report explicit status instead of silently degrading.

## Key Changes
- Add a typed `[recording.screen]` config block in core/app-server config with `enabled`, and expose it through `config/read` plus config schema updates.
- Add `ScreenRecordingManager` under a new `app-server` module (for example `app-server/src/recording/`) with its own lifecycle, reconcile it at startup, on relevant config writes, and during app-server shutdown.
- Start capture automatically when the effective config has `recording.screen.enabled = true`; stop capture and purge local artifacts when config changes flip it to `false`.
- Use `xcap` to do screen capture.
- Capture each display separately once per second, offload grab/encode/write work off the request path, write JPEG files plus a timestamped manifest, and prune on startup and continuously.
- Keep privacy boundaries hard in v1: no audio, no keylogging, no content redaction, and no RPC that returns frame content.

## Public APIs / Types
- Add experimental app-server requests:
  - `recording/screen/read`
  - `recording/screen/pause`
  - `recording/screen/resume`
- Do not add `recording/screen/enable` or `recording/screen/disable`. Persistent enablement is handled through existing config write APIs by changing `recording.screen.enabled`.
- Add experimental notification `recording/screen/status/updated`, broadcast to initialized clients when service state changes.
- Add typed models:
  - `ScreenRecordingConfig` with `enabled`
  - `ScreenRecordingStatus` with `state`, `paused`, `platform`, `backend`, `permission`, `capture_fps`, `retention_hours`, `storage_path`, `captured_display_count`, `newest_frame_at`, and `last_error`
- Expose `recording` in `config/read` behind experimental field gating until the feature exits internal rollout.
- Regenerate app-server schemas and update `codex-rs/app-server/README.md` plus `codex-rs/core/config.schema.json`.

## Test Plan
- Protocol tests for request/notification serialization and experimental gating.
- Config tests for parsing, defaults, schema generation, and `config/read` exposure of `[recording.screen]`.
- Manager unit tests with a fake backend covering disabled startup, enabled startup auto-begins capture, pause/resume, permission-denied, runtime backend failure, prune-after-6h, disable-via-config stops and purges, and shutdown drain.
- App-server integration tests covering startup autostart from config, `recording/screen/read`, `pause`/`resume`, `recording/screen/status/updated`, config-write reconciliation, and notification opt-out behavior.
- Platform smoke tests that compile each backend on its target OS; CI runtime tests use the fake backend rather than real capture permissions.

## Assumptions
- The service is process-scoped, not thread-scoped.
- `paused` is runtime-only and does not persist across restart.
- Future quick-reference and memory-generation work will consume the manifest/artifact layout added here, but that pipeline is intentionally out of scope for this first implementation.
