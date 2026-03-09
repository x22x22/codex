# codex-otel

`codex-otel` is the OpenTelemetry integration crate for Codex. It provides:

- Provider wiring for log/trace/metric exporters (`codex_otel::OtelProvider`,
  `codex_otel::provider`, and the compatibility shim `codex_otel::otel_provider`).
- Session-scoped business event emission via `codex_otel::SessionTelemetry`.
- Low-level metrics APIs via `codex_otel::metrics`.
- Trace-context helpers via `codex_otel::trace_context` and crate-root re-exports.

## Ownership rules

`codex-otel` is intentionally split by telemetry family:

- `SessionTelemetry` owns session-scoped business events that need shared
  metadata and explicit log-only versus trace-safe field selection.
- `metrics` owns low-level counters, histograms, and timers that do not need
  session metadata.
- `trace_context` owns W3C trace propagation and parent-setting helpers.
- Domain-owned audit events may stay with the subsystem that emits them instead
  of being forced through `SessionTelemetry`.

Current example of a domain-owned audit emitter:

- `codex-network-proxy` emits policy audit events on the
  `codex_otel.network_proxy` target. See
  `codex-rs/network-proxy/README.md` for the event family details.

## Tracing and logs

Create an OTEL provider from `OtelSettings`. The provider also configures
metrics (when enabled), then attach its layers to your `tracing_subscriber`
registry:

```rust
use codex_otel::config::OtelExporter;
use codex_otel::config::OtelHttpProtocol;
use codex_otel::config::OtelSettings;
use codex_otel::OtelProvider;
use tracing_subscriber::prelude::*;

let settings = OtelSettings {
    environment: "dev".to_string(),
    service_name: "codex-cli".to_string(),
    service_version: env!("CARGO_PKG_VERSION").to_string(),
    codex_home: std::path::PathBuf::from("/tmp"),
    exporter: OtelExporter::OtlpHttp {
        endpoint: "https://otlp.example.com".to_string(),
        headers: std::collections::HashMap::new(),
        protocol: OtelHttpProtocol::Binary,
        tls: None,
    },
    trace_exporter: OtelExporter::OtlpHttp {
        endpoint: "https://otlp.example.com".to_string(),
        headers: std::collections::HashMap::new(),
        protocol: OtelHttpProtocol::Binary,
        tls: None,
    },
    metrics_exporter: OtelExporter::None,
};

if let Some(provider) = OtelProvider::from(&settings)? {
    let registry = tracing_subscriber::registry()
        .with(provider.logger_layer())
        .with(provider.tracing_layer());
    registry.init();
}
```

## Target families and routing

`codex-otel` reserves the following target families:

- `codex_otel.log_only` for session events whose rich fields should stay on the
  OTEL log lane only.
- `codex_otel.trace_safe` for session events whose fields are safe to attach to
  exported traces.
- `codex_otel.<subsystem>` for approved domain-owned audit emitters such as
  `codex_otel.network_proxy`.

Current routing behavior:

- Log export accepts any `codex_otel.*` target except the
  `codex_otel.trace_safe` family.
- Trace export accepts `codex_otel.trace_safe` events plus all spans.

This means adding a new `codex_otel.<subsystem>` target is immediately
export-visible on the OTEL log lane unless provider filters change. Treat new
domain-owned targets as an explicit review point.

This refactor does not make arbitrary spans trace-safe. Direct spans outside
`SessionTelemetry` are still exported by the trace filter, so any span fields on
those paths still need separate privacy review.

## SessionTelemetry (events)

`SessionTelemetry` adds consistent metadata to tracing events and helps record
Codex-specific session events. Rich session/business events should go through
`SessionTelemetry`; subsystem-owned audit events can stay with the owning subsystem.

```rust
use codex_otel::SessionTelemetry;

let manager = SessionTelemetry::new(
    conversation_id,
    model,
    slug,
    account_id,
    account_email,
    auth_mode,
    originator,
    log_user_prompts,
    terminal_type,
    session_source,
);

manager.user_prompt(&prompt_items);
```

## Metrics (OTLP or in-memory)

Modes:

- OTLP: exports metrics via the OpenTelemetry OTLP exporter (HTTP or gRPC).
- In-memory: records via `opentelemetry_sdk::metrics::InMemoryMetricExporter` for tests/assertions; call `shutdown()` to flush.

`codex-otel` also provides `OtelExporter::Statsig`, a shorthand for exporting OTLP/HTTP JSON metrics
to Statsig using Codex-internal defaults.

Statsig ingestion (OTLP/HTTP JSON) example:

```rust
use codex_otel::config::{OtelExporter, OtelHttpProtocol};

let metrics = MetricsClient::new(MetricsConfig::otlp(
    "dev",
    "codex-cli",
    env!("CARGO_PKG_VERSION"),
    OtelExporter::OtlpHttp {
        endpoint: "https://api.statsig.com/otlp".to_string(),
        headers: std::collections::HashMap::from([(
            "statsig-api-key".to_string(),
            std::env::var("STATSIG_SERVER_SDK_SECRET")?,
        )]),
        protocol: OtelHttpProtocol::Json,
        tls: None,
    },
))?;

metrics.counter("codex.session_started", 1, &[("source", "tui")])?;
metrics.histogram("codex.request_latency", 83, &[("route", "chat")])?;
```

In-memory (tests):

```rust
let exporter = InMemoryMetricExporter::default();
let metrics = MetricsClient::new(MetricsConfig::in_memory(
    "test",
    "codex-cli",
    env!("CARGO_PKG_VERSION"),
    exporter.clone(),
))?;
metrics.counter("codex.turns", 1, &[("model", "gpt-5.1")])?;
metrics.shutdown()?; // flushes in-memory exporter
```

## Trace context

Trace propagation helpers remain separate from the session event emitter:

```rust
use codex_otel::current_span_w3c_trace_context;
use codex_otel::set_parent_from_w3c_trace_context;
```

## Shutdown

- `OtelProvider::shutdown()` stops the OTEL exporter.
- `SessionTelemetry::shutdown_metrics()` flushes and shuts down the metrics provider.

Both are optional because drop performs best-effort shutdown, but calling them
explicitly gives deterministic flushing (or a shutdown error if flushing does
not complete in time).
