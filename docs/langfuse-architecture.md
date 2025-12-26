# Langfuse Integration Architecture

This document provides visual representations of how Codex integrates with Langfuse.

## High-Level Architecture

```mermaid
graph TB
    User[User] -->|codex command| Codex[Codex CLI]
    Codex -->|OpenAI API| OpenAI[OpenAI Service]
    OpenAI -->|Stream Response| Codex
    
    Codex -->|OTLP Events| OtelManager[OpenTelemetry Manager]
    OtelManager -->|HTTP POST| LangfuseEndpoint[Langfuse OTLP Endpoint]
    LangfuseEndpoint -->|Store| LangfuseDB[(Langfuse Database)]
    
    LangfuseDB -->|Query| LangfuseUI[Langfuse Web UI]
    Team[Team Members] -->|View/Analyze| LangfuseUI
    
    style Codex fill:#4a9eff
    style LangfuseEndpoint fill:#ff6b6b
    style LangfuseUI fill:#ff6b6b
    style LangfuseDB fill:#ff6b6b
```

## Data Flow

```mermaid
sequenceDiagram
    participant User
    participant Codex
    participant OpenAI
    participant Langfuse
    
    User->>Codex: Run command
    Note over Codex: Start session
    Codex->>Langfuse: conversation_starts event
    
    User->>Codex: Provide prompt
    Codex->>Langfuse: user_prompt event
    
    Codex->>OpenAI: API Request
    Codex->>Langfuse: api_request event
    OpenAI-->>Codex: Stream response
    Codex->>Langfuse: sse_event (tokens)
    
    Note over Codex: Execute tool
    Codex->>Langfuse: tool_decision event
    Codex->>Langfuse: tool_result event
    
    Note over Codex: Session complete
    Codex->>Langfuse: Final events
```

## Component Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                         Codex CLI                           │
│  ┌───────────────────────────────────────────────────────┐  │
│  │                   Core Application                     │  │
│  │  • User interaction                                    │  │
│  │  • OpenAI API calls                                    │  │
│  │  • Tool execution                                      │  │
│  └──────────────────┬────────────────────────────────────┘  │
│                     │                                        │
│  ┌──────────────────▼─────────────────────────────────────┐ │
│  │              OpenTelemetry Layer                       │ │
│  │  ┌──────────────────────────────────────────────────┐ │ │
│  │  │ OtelManager (codex-rs/otel)                      │ │ │
│  │  │  • Event collection                               │ │ │
│  │  │  • Event enrichment (metadata)                    │ │ │
│  │  │  • Event batching                                 │ │ │
│  │  └──────────────────┬───────────────────────────────┘ │ │
│  │                     │                                  │ │
│  │  ┌──────────────────▼───────────────────────────────┐ │ │
│  │  │ OTLP Exporter (HTTP)                             │ │ │
│  │  │  • Protocol: HTTP/protobuf or HTTP/JSON          │ │ │
│  │  │  • Authentication: Basic Auth                     │ │ │
│  │  │  • Batching & retry logic                        │ │ │
│  │  └──────────────────┬───────────────────────────────┘ │ │
│  └────────────────────┼─────────────────────────────────┘ │
└────────────────────────┼───────────────────────────────────┘
                         │
                         │ HTTPS POST
                         │ /api/public/otel
                         │
┌────────────────────────▼───────────────────────────────────┐
│                  Langfuse Platform                         │
│  ┌─────────────────────────────────────────────────────┐   │
│  │              OTLP Ingestion API                     │   │
│  │  • Endpoint: /api/public/otel                       │   │
│  │  • Authentication: HTTP Basic Auth                  │   │
│  │  • Protocol: HTTP/protobuf, HTTP/JSON               │   │
│  └──────────────────┬──────────────────────────────────┘   │
│                     │                                       │
│  ┌──────────────────▼──────────────────────────────────┐   │
│  │         OpenTelemetry to Langfuse Mapper           │   │
│  │  • Map OTLP spans → Langfuse traces                │   │
│  │  • Extract GenAI attributes                         │   │
│  │  • Calculate costs from token usage                 │   │
│  │  • Parse metadata                                   │   │
│  └──────────────────┬──────────────────────────────────┘   │
│                     │                                       │
│  ┌──────────────────▼──────────────────────────────────┐   │
│  │              Langfuse Database                      │   │
│  │  • Traces & observations                            │   │
│  │  • Prompts & versions                               │   │
│  │  • Evaluations & scores                             │   │
│  │  • Datasets & experiments                           │   │
│  └──────────────────┬──────────────────────────────────┘   │
│                     │                                       │
│  ┌──────────────────▼──────────────────────────────────┐   │
│  │              Langfuse Web UI                        │   │
│  │  • Trace visualization                              │   │
│  │  • Cost analytics                                   │   │
│  │  • Prompt management                                │   │
│  │  • Evaluation tools                                 │   │
│  └─────────────────────────────────────────────────────┘   │
└────────────────────────────────────────────────────────────┘
```

## Event Types and Mappings

```mermaid
graph LR
    subgraph "Codex Events"
        CE1[conversation_starts]
        CE2[api_request]
        CE3[sse_event]
        CE4[user_prompt]
        CE5[tool_decision]
        CE6[tool_result]
    end
    
    subgraph "OpenTelemetry"
        OT1[Log Events]
        OT2[Span Attributes]
        OT3[Resource Attributes]
    end
    
    subgraph "Langfuse Data Model"
        LF1[Trace]
        LF2[Observation: Span]
        LF3[Observation: Generation]
        LF4[Observation: Event]
        LF5[Metadata]
        LF6[Usage & Cost]
    end
    
    CE1 --> OT1
    CE2 --> OT1
    CE3 --> OT1
    CE4 --> OT1
    CE5 --> OT1
    CE6 --> OT1
    
    OT1 --> OT2
    OT2 --> OT3
    
    OT2 --> LF1
    OT2 --> LF2
    OT2 --> LF3
    OT2 --> LF4
    OT3 --> LF5
    CE3 --> LF6
```

## Configuration Flow

```
┌──────────────────────────────────────────────────────────┐
│                User Configuration                        │
│           ~/.codex/config.toml                          │
│                                                          │
│  [otel]                                                  │
│  environment = "production"                              │
│  exporter = "otlp-http"                                  │
│                                                          │
│  [otel.exporter."otlp-http"]                            │
│  endpoint = "https://cloud.langfuse.com/..."            │
│  protocol = "binary"                                     │
│                                                          │
│  [otel.exporter."otlp-http".headers]                    │
│  "Authorization" = "Basic <base64>"                      │
└──────────────────┬───────────────────────────────────────┘
                   │
                   │ Load & Parse
                   ▼
┌──────────────────────────────────────────────────────────┐
│              Config Loader (core/src/config)             │
│  • Parse TOML                                            │
│  • Apply defaults                                        │
│  • Validate settings                                     │
│  • Expand environment variables                          │
└──────────────────┬───────────────────────────────────────┘
                   │
                   │ OtelConfig
                   ▼
┌──────────────────────────────────────────────────────────┐
│          OTEL Provider Builder (core/src/otel_init.rs)   │
│  • Convert config types                                  │
│  • Create OTLP exporter                                  │
│  • Setup resource attributes                             │
│  • Initialize tracer provider                            │
└──────────────────┬───────────────────────────────────────┘
                   │
                   │ OtelProvider
                   ▼
┌──────────────────────────────────────────────────────────┐
│         OTLP HTTP Exporter (otel/src/otel_provider.rs)   │
│  • HTTP client with authentication                       │
│  • Protocol serialization (protobuf/json)                │
│  • Batch processing                                      │
│  • Retry logic                                           │
│  • TLS configuration                                     │
└──────────────────┬───────────────────────────────────────┘
                   │
                   │ Export events
                   ▼
              [Langfuse API]
```

## Metadata Flow

```
User Action → Codex Event
                ↓
        Common Metadata Added:
        • conversation.id
        • app.version
        • auth_mode
        • user.account_id
        • user.email
        • terminal.type
        • model
        • slug
                ↓
        Event-Specific Data:
        • api_request: status_code, duration_ms
        • sse_event: token counts
        • tool_result: success, output
                ↓
        OpenTelemetry Attributes:
        • service.name
        • service.version
        • deployment.environment
                ↓
        Langfuse Processing:
        • Extract trace-level attributes
        • Map to observation types
        • Calculate costs
        • Build hierarchy
                ↓
        Langfuse UI Display:
        • Trace timeline
        • Token usage graphs
        • Cost breakdown
        • Filterable metadata
```

## Deployment Options

```mermaid
graph TB
    subgraph "Option 1: Langfuse Cloud"
        Codex1[Codex CLI]
        Cloud[Langfuse Cloud<br/>EU/US Regions]
        Codex1 -->|HTTPS| Cloud
    end
    
    subgraph "Option 2: Self-Hosted (Docker)"
        Codex2[Codex CLI]
        Docker[Langfuse Container]
        DB1[(PostgreSQL)]
        Codex2 -->|HTTP/HTTPS| Docker
        Docker -->|Store| DB1
    end
    
    subgraph "Option 3: Self-Hosted (Kubernetes)"
        Codex3[Codex CLI]
        K8s[Langfuse Pods]
        DB2[(PostgreSQL)]
        Redis[(Redis)]
        Codex3 -->|HTTPS| K8s
        K8s -->|Store| DB2
        K8s -->|Cache| Redis
    end
    
    style Cloud fill:#ff6b6b
    style Docker fill:#4a9eff
    style K8s fill:#4a9eff
```

## Security Flow

```
┌─────────────────────────────────────────────────┐
│         Credential Management                   │
│                                                 │
│  Option 1: Direct in config.toml               │
│    "Authorization" = "Basic <base64>"           │
│                                                 │
│  Option 2: Environment Variable                 │
│    "Authorization" = "${LANGFUSE_AUTH}"         │
│    export LANGFUSE_AUTH="Basic <base64>"        │
│                                                 │
│  Option 3: Secrets Manager (future)             │
│    Integration with vault/keychain             │
└───────────────────┬─────────────────────────────┘
                    │
                    ▼
┌─────────────────────────────────────────────────┐
│         TLS/SSL Configuration                   │
│                                                 │
│  • CA Certificate (custom trust)                │
│  • Client Certificate (mTLS)                    │
│  • Client Private Key                           │
│                                                 │
│  Paths: Relative to ~/.codex/ or absolute      │
└───────────────────┬─────────────────────────────┘
                    │
                    ▼
┌─────────────────────────────────────────────────┐
│         HTTPS Request                           │
│                                                 │
│  POST /api/public/otel                          │
│  Authorization: Basic <base64>                  │
│  Content-Type: application/x-protobuf           │
│                                                 │
│  [Encrypted OTLP payload]                       │
└───────────────────┬─────────────────────────────┘
                    │
                    ▼
              [Langfuse API]
              • Verify credentials
              • Decrypt payload
              • Process events
```

## Key Advantages

1. **No Code Changes**: Pure configuration
2. **Standard Protocol**: OpenTelemetry (OTLP)
3. **Flexible Deployment**: Cloud or self-hosted
4. **Rich Metadata**: Automatic context propagation
5. **LLM-Specific**: Purpose-built for AI apps
6. **Privacy Control**: Configurable logging levels
7. **Secure**: HTTPS + Authentication + optional mTLS

## References

- [OpenTelemetry Specification](https://opentelemetry.io/docs/specs/)
- [OTLP Protocol](https://opentelemetry.io/docs/specs/otlp/)
- [Langfuse OTLP Endpoint](https://langfuse.com/docs/integrations/opentelemetry)
- [GenAI Semantic Conventions](https://opentelemetry.io/docs/specs/semconv/gen-ai/)
