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

| Capability | OpenCodex | Joocode | Priority |
|---|---:|---:|---:|
| Model/provider discovery | Yes | Yes, multi-source | Complete |
| OpenAI-compatible local gateway | Yes | Yes | Complete |
| Desktop-client auto-configuration | Yes | Codex, Zed, Claude Code, GitHub Copilot App, Grok Build, others | Stronger/different |
| Combo failover routing | Yes | Yes, ordered candidates | Complete |
| Weighted round-robin | Yes | Yes, deterministic weighted selection | Complete |
| Generic request retry/backoff | Yes | Yes, configurable with `Retry-After` | Complete |
| Provider pacing and cooldown | Yes | Cooldown shipped; pacing pending | P1 |
| Non-loopback authentication | Yes | Yes, token required | Complete |
| Restrictive remote CORS | Yes | Yes, explicit origin allowlist | Complete |
| Readiness endpoint | Yes | Yes, `/readyz` | Complete |
| Request body limits | Yes | Yes, configurable | Complete |
| Stream stall/idle timeout | Yes | Yes, configurable incomplete semantics | Complete |
| Integration ownership journal | Yes | Merge-preserving writes, no formal journal | P0 |
| Credential/API-key pools | Yes | One credential route per discovered provider | P2 |
| Multi-account OAuth pools | Yes | Limited source-specific support | P2 |
| Native Anthropic upstream | Yes | Mostly translated through Chat Completions | P3 |
| Native Gemini upstream | Yes | Partial/source-specific | P3 |
| Native Responses upstream | Yes | Partial | P3 |
| Responses compact endpoint | Yes | No | P3 |
| Responses WebSocket | Yes | No | P3 |
| Image generation/edit endpoints | Yes | No | P3 |
| Realtime/Live API | Yes | No | P5 |
| Browser management dashboard | Yes | TUI only | Intentionally different |
| Usage and latency analytics | Yes | Minimal status only | P4 |
| Remote authenticated hub | Yes | No | P5 |
| Web-search/vision sidecars | Yes | No | P5 |
| Subagent catalog controls | Yes | Basic catalog metadata | P2 |
| On-demand Codex shim | Yes | No | P5 |

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

**Status: idle timeout implemented.** Upstream streams default to a 90-second
idle timeout configurable through `JOOCODE_STREAM_IDLE_TIMEOUT_SECONDS`.
Responses clients receive `response.incomplete`; Anthropic clients receive an
explicit error event. Remaining hardening work includes event-size and
accumulated tool-argument limits.

Add:

- stream idle timeout;
- maximum SSE event size;
- maximum accumulated tool-call arguments;
- cancellation propagation;
- explicit incomplete-response events.

An upstream stream that ends before its terminal event must not be reported as successfully completed.

### Tool-call authorization

Only allow upstream models to call tools that appeared in the client request. Reject invented or undeclared tool names with an explicit protocol error.

### Integration ownership journal

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

Joocode should refuse destructive overwrites when a managed field was changed externally and ownership is ambiguous.

## P1 — Routing superpowers

This is the highest-value feature group for Joocode's product direction.

### Model combos

Status: **ordered failover and weighted round-robin shipped**.

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

Supported strategies should eventually include:

- ordered failover;
- round-robin;
- weighted round-robin;
- lowest-latency healthy destination;
- quota/cooldown-aware selection.

### Retry policy

Classify upstream failures before retrying:

Status: **generic retries and failover classification shipped**. Provider pacing,
and adaptive quota-aware cooldown remain pending. Basic provider cooldown and
per-provider concurrency limits are shipped.

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

### 1. Secure non-loopback binding

Implement mandatory access tokens, restrictive CORS, and safe startup guards before promoting LAN access.

### 2. Combo routing with retries and failover

This provides the largest user-facing improvement while remaining within Joocode's provider-gateway responsibility.

### 3. Readiness and hardened streaming

Add `/readyz`, stream deadlines, explicit incomplete-response semantics, and cancellation propagation.

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
