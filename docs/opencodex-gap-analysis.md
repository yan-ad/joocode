# OpenCodex Gap Analysis

> A living comparison between Joocode and [OpenCodex](https://github.com/lidge-jun/opencodex), the original source of inspiration for this project.

## Snapshot

- **Reviewed:** September 2026
- **OpenCodex reference:** v2.46.0 (`bba6322`)
- **Joocode role:** lightweight native Rust provider gateway and desktop integration control plane
- **Purpose:** identify useful capabilities to adopt without turning Joocode into a full agent runtime or duplicating every OpenCodex feature

## Product boundary

Joocode should remain focused on:

- provider and credential discovery;
- model catalog aggregation;
- protocol translation;
- routing and reliability;
- local desktop-client integrations;
- lightweight local observability.

Joocode should not become:

- a coding-agent runtime;
- a workflow execution engine;
- a full browser management application;
- an unrestricted remote gateway without authentication;
- a clone of every provider-specific internal transport supported by OpenCodex.

## Capability comparison

Status legend: **Complete**, **Partial**, **Planned**, or **Intentional**.

| Area | Capability | OpenCodex | Joocode | Status / next work |
|---|---|---:|---:|---|
| Discovery | Multi-source provider/model discovery | Yes | OpenCode, CrabCode, OpenCodex, Hermes, Copilot, Antigravity, local providers | **Complete** |
| Discovery | Selectively enable detected sources | Yes | Persistent `/ Config` toggles and explicit `--source` override | **Complete** |
| Discovery | Hot registry reload | Yes | `jcx reload` and `POST /api/reload` with atomic swap | **Complete** |
| Gateway | OpenAI Chat Completions | Yes | `/v1/chat/completions` | **Complete** |
| Gateway | OpenAI Responses | Yes | Responses-to-Chat translation plus native OpenAI passthrough | **Partial** — routed native Responses transport pending |
| Gateway | Anthropic Messages | Yes | Messages/SSE/tool translation through Chat Completions | **Partial** — native upstream pending |
| Gateway | Gemini/Cloud Code bridge | Yes | Source-specific and Antigravity bridge support | **Partial** — generic native Gemini upstream pending |
| Gateway | Responses compaction | Yes | Native OpenAI/ChatGPT passthrough | **Partial** — routed providers pending |
| Gateway | Responses WebSocket | Yes | No | **Planned P3** |
| Gateway | Image generation/editing | Yes | No standalone endpoints | **Planned P3** |
| Gateway | Realtime/Live API | Yes | No | **Planned P5** |
| Tools | Function tools | Yes | Yes | **Complete** |
| Tools | Namespace/MCP tools | Yes | Reversible namespace flattening for routed models | **Complete** |
| Tools | Undeclared tool rejection | Yes | Non-streaming and streaming validation | **Complete** |
| Tools | Structured MCP output preservation | Yes | Yes | **Complete** |
| Routing | Ordered combo failover | Yes | `combo/name` | **Complete** |
| Routing | Weighted round-robin | Yes | Deterministic weighted selection | **Complete** |
| Routing | Generic retries/backoff | Yes | Retryable transport/status classification and `Retry-After` | **Complete** |
| Routing | Provider cooldown | Yes | Fixed cooldown and combo skip | **Complete** |
| Routing | Per-provider concurrency budgets | Yes | Stream-lifetime permits | **Complete** |
| Routing | Request pacing | Yes | Configurable minimum interval per provider | **Complete** |
| Routing | Adaptive quota/error cooldown | Yes | Retry-After plus bounded exponential error-streak cooldown | **Complete** |
| Routing | Lowest-latency/health-aware selection | Yes | Passive EWMA latency and health-aware combo routing | **Complete** |
| Credentials | Source-owned credentials | Yes | Reuses each source's credential store without copying secrets | **Complete** |
| Credentials | Generic API-key pools | Yes | One credential route per discovered provider | **Planned P2** |
| Credentials | Multi-account OAuth pools | Yes | Limited source-specific support | **Planned P2** |
| Security | Non-loopback admission auth | Yes | Mandatory `JOOCODE_API_AUTH_TOKEN` | **Complete** |
| Security | Restrictive remote CORS | Yes | Explicit remote origin allowlist | **Complete** |
| Security | Request body limits | Yes | Configurable, default 16 MiB | **Complete** |
| Security | SSE event/tool argument limits | Yes | Configurable, default 1 MiB each | **Complete** |
| Security | Stream idle timeout/incomplete semantics | Yes | Configurable timeout and explicit protocol errors | **Complete** |
| Security | Downstream cancellation propagation | Yes | Upstream stream dropped on client disconnect | **Complete** |
| Security | Remote rate limiting | Yes | Configurable token-bucket admission limiter | **Complete** |
| Security | Separate management-plane credential | Yes | Dedicated management token required remotely | **Complete** |
| Integrations | Desktop auto-configuration | Yes | Codex, Zed, Claude Code, Grok Build, GitHub Copilot App; JetBrains endpoint guidance | **Complete/Partial by client** |
| Integrations | Configuration ownership journal | Yes | Managed-resource conflict detection for Codex, Zed, Claude Code, Grok Build, Copilot App and service definitions | **Complete** |
| Integrations | Background service and auto-start | Yes | macOS, Linux, Windows lifecycle controls | **Complete** |
| Integrations | Self-update | Yes | Check, checksum, replace/relaunch on Unix and Windows | **Complete** |
| Catalog | Model capability metadata | Yes | Context/output/reasoning and direct-tool compatibility | **Partial** — deeper provider metadata pending |
| Catalog | Subagent model controls | Yes | Basic catalog metadata only | **Planned P2** |
| Observability | Liveness/readiness | Yes | `/healthz` and `/readyz` | **Complete** |
| Observability | Runtime status | Yes | `jcx stats` and `/api/status` | **Complete** |
| Observability | Provider/catalog status | Yes | `/api/providers`, active requests and cooldown state | **Complete** |
| Observability | Prometheus metrics | Yes | `/api/metrics` | **Complete** |
| Observability | Latency and provider health | Yes | Provider EWMA latency, failure streak, cooldown and saturation | **Complete** |
| Observability | Codex browser/computer tool calls | No/limited | Privacy-safe total and per-tool Prometheus metrics | **Complete — Joocode advantage** |
| Observability | Token/retry/failover breakdown | Yes | Tool and request counters shipped; token/retry/failover counters pending | **Partial P4** |
| UI | Browser management dashboard | Yes | Compact Ratatui dashboard and modals | **Intentional difference** |
| Ecosystem | Remote authenticated hub | Yes | No | **Planned P5** |
| Ecosystem | Web-search/vision sidecars | Yes | No | **Planned P5** |
| Ecosystem | On-demand Codex shim | Yes | No | **Planned P5** |

## P0 — Security and correctness

These should be addressed before encouraging LAN or remote use.

### Non-loopback authentication

**Status: implemented.** Non-loopback listeners require
`JOOCODE_API_AUTH_TOKEN`. Clients may authenticate with Bearer auth or
`x-joocode-api-key`; Joocode removes the admission credential before forwarding
the request upstream.

Loopback should continue accepting a non-empty placeholder key. Binding to a non-loopback address must require a real Joocode access token.

Suggested behavior:

```text
127.0.0.1 / ::1
  Any non-empty local API key is accepted.

0.0.0.0 / LAN address
  JOOCODE_API_AUTH_TOKEN is mandatory.
```

Suggested startup guard:

```rust
if !address.ip().is_loopback() && auth_token.is_none() {
    bail!("Non-loopback binding requires JOOCODE_API_AUTH_TOKEN");
}
```

### Restrictive CORS

**Status: implemented.** Loopback mirrors local request origins for desktop
compatibility. Remote listeners allow only origins listed in
`JOOCODE_ALLOWED_ORIGINS`; an empty list emits no cross-origin grant.

- Do not use permissive CORS for non-loopback listeners.
- Allow an explicit origin list.
- Keep management routes more restricted than model data-plane routes.

### Readiness endpoint

**Status: implemented.** `/readyz` returns provider/model counts and reports
`ready`, `degraded`, or `failed` independently from `/healthz` liveness.

Separate process liveness from registry readiness:

```text
GET /healthz  -> process is alive
GET /readyz   -> registry is loaded and can route requests
```

Suggested readiness states:

- `pending`
- `ready`
- `degraded`
- `failed`

### Request body limits

**Status: implemented.** JSON and bridge request bodies are limited to 16 MiB
by default. `JOOCODE_MAX_REQUEST_BYTES` can set a different positive byte limit.

### Bounded streaming

**Status: implemented.** Upstream streams default to a 90-second idle timeout,
1 MiB maximum SSE event size, and 1 MiB accumulated argument limit per tool
call. The limits are configurable through
`JOOCODE_STREAM_IDLE_TIMEOUT_SECONDS`, `JOOCODE_MAX_SSE_EVENT_BYTES`, and
`JOOCODE_MAX_TOOL_ARGUMENT_BYTES`. Responses clients receive
`response.incomplete`; Anthropic clients receive an explicit error event.
Dropping a downstream response body drops and cancels the upstream stream.

Add:

- [x] stream idle timeout;
- [x] maximum SSE event size;
- [x] maximum accumulated tool-call arguments;
- [x] cancellation propagation through stream drop;
- [x] explicit incomplete-response events.

An upstream stream that ends before its terminal event must not be reported as successfully completed.

### Tool-call authorization

Status: **shipped**. Upstream models may only call tools that appeared in the
client request. Invented or undeclared tool names produce an explicit protocol
error; streaming responses never finish successfully with an undeclared call.

### Integration ownership journal

**Status: implemented.** Joocode fingerprints only the configuration subtree,
catalog, database rows, or service definition it owns. Unrelated user settings
remain editable; externally modified managed values are rejected rather than
silently overwritten.

Joocode increasingly edits external configuration and state:

- Codex configuration/catalog;
- Zed settings and credentials;
- Claude Code settings;
- Grok Build settings;
- GitHub Copilot App database;
- OS background-service files.

Create a journal containing:

```json
{
  "client": "zed",
  "resource": "~/.config/zed/settings.json",
  "owned_paths": [
    "language_models.openai_compatible.joocode",
    "agent.commit_message_instructions"
  ],
  "before_hash": "...",
  "managed_hash": "...",
  "snapshot": "..."
}
```

Joocode refuses destructive overwrites when a managed field was changed externally and ownership is ambiguous.

## P1 — Routing superpowers

This is the highest-value feature group for Joocode's product direction.

### Model combos

Status: **complete**. Ordered failover, weighted round-robin, cooldown-aware
fallback, and passive lowest-latency selection are shipped.

Expose virtual models such as:

```text
combo/coding
combo/fast
combo/reliable
```

Example configuration:

```json
{
  "name": "coding",
  "strategy": "failover",
  "models": [
    "crabcode/clip/claude-sonnet",
    "opencode/openai/gpt-5",
    "hermes/openrouter/gemini-pro"
  ]
}
```

Supported strategies include:

- ordered failover;
- weighted round-robin;
- lowest-latency healthy destination;
- quota/cooldown-aware selection.

### Retry policy

Classify upstream failures before retrying:

Status: **complete**. Generic retries, failover classification, proactive pacing,
bounded exponential error-streak cooldown, `Retry-After`, stream-lifetime
concurrency permits, and passive lowest-latency routing are shipped.

```rust
enum UpstreamFailure {
    Authentication,
    RateLimited { retry_after: Option<Duration> },
    Capacity,
    Timeout,
    Transport,
    InvalidRequest,
    ProviderError,
}
```

Retry only safe categories such as transport failures, timeouts, rate limits, and temporary upstream capacity errors.

### Provider cooldown and concurrency limits

Maintain runtime state per provider route:

- active request count;
- last failure;
- cooldown deadline;
- consecutive failures;
- recent latency;
- recent rate-limit status.

## P2 — Credential and catalog controls

### API-key pools

Start with generic key pools before attempting full ChatGPT account management:

```json
{
  "provider": "openrouter",
  "strategy": "failover",
  "keys": [
    { "id": "primary", "enabled": true, "api_key": "..." },
    { "id": "backup", "enabled": true, "api_key": "..." }
  ]
}
```

### Subagent catalog controls

Add configuration for:

- featured subagent models;
- fallback models;
- reasoning-effort caps;
- disabled models;
- maximum advertised subagent entries;
- provider/model health tests.

## P3 — Protocol depth

Joocode currently works best when upstream providers expose OpenAI-compatible Chat Completions. Native transports would preserve more provider-specific capabilities.

Recommended order:

1. Native OpenAI Responses upstream.
2. `/v1/responses/compact`.
3. Native Anthropic Messages upstream.
4. Native Gemini upstream.
5. Image generation and editing endpoints.
6. Responses WebSocket.

Native adapters should preserve:

- reasoning blocks;
- prompt-cache metadata;
- encrypted reasoning fields;
- native tool semantics;
- image/media payloads;
- provider-specific usage and service-tier metadata.

## P4 — Local observability

Add bounded, privacy-safe statistics without storing prompt or response content.

**Status: management foundation shipped.** `jcx stats` and `GET /api/status`
report uptime, provider/model counts, total and active requests, successes,
failures, provider concurrency usage, and cooldown state. `jcx reload` and
`POST /api/reload` atomically refresh enabled sources while retaining the old
registry on failure. `GET /api/providers` exposes the non-secret logical
catalog, source reports, and passive runtime availability. Token, latency,
provider distribution, retry, and failover metrics remain pending.

Suggested CLI:

```bash
jcx stats
```

Suggested metrics:

- request count;
- errors by category;
- input/output token totals;
- average and percentile latency;
- active streams;
- provider/model distribution;
- retries and failovers;
- provider cooldown state.

Suggested management endpoints:

```text
GET  /api/status
GET  /api/providers
GET  /api/metrics
POST /api/reload
```

`/api/status`, `/api/providers`, `/api/metrics`, and `/api/reload` are shipped.
The Prometheus endpoint contains only process, registry, request-counter, and
provider-runtime gauges. Token, latency, retry, failover, and model-distribution
metrics remain pending.

## P5 — Optional ecosystem work

Only pursue these after P0–P3 are stable:

- authenticated remote hub;
- Responses Realtime/Live relay;
- web-search sidecars;
- vision sidecars for text-only models;
- on-demand Codex shim;
- additional desktop clients;
- web dashboard.

## Features intentionally not copied yet

### Full browser management dashboard

Joocode's TUI is lightweight and distinctive. A web dashboard would introduce a frontend build chain, static assets, session authentication, CSRF concerns, and a significantly larger management surface.

### Full ChatGPT account pool

This is powerful but has high maintenance and policy risk. Users needing advanced account pools can already use OCX/OpenCodex as an upstream source.

### Internal provider transports

Cursor protobuf, Kiro, MiMo, and other private transports change frequently. They should only be adopted when there is concrete demand and sufficient maintenance capacity.

### Remote access before security hardening

Joocode should remain local-first until non-loopback authentication, CORS restrictions, rate limits, body limits, and management-plane isolation are complete.

## Recommended next three initiatives

### 1. Add generic credential pools

Support multiple API keys and source-owned OAuth accounts with explicit
active/failover/round-robin policies, without copying credentials into desktop
clients.

### 2. Deepen native provider protocols

Add native Responses, Anthropic Messages, and Gemini upstream transports before
Responses WebSocket and standalone image endpoints.

### 3. Complete request economics observability

Record privacy-safe token counts plus retry/failover/model-distribution counters
and latency histograms. Never store prompts, tool arguments, or response bodies.

## Strategic direction

Joocode should not compete with OpenCodex by matching every feature count. It should adopt the strongest reliability and routing ideas while preserving its differentiators:

- native Rust binary;
- low idle memory;
- fast startup;
- config-first multi-source discovery;
- local desktop-client automation;
- compact TUI;
- minimal operational footprint.

The intended positioning remains:

> A fast, native, local AI provider gateway with routing superpowers—not a full agent runtime or a direct OpenCodex clone.
