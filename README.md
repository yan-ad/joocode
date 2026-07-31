# crabcodex

A small native Rust bridge that exposes providers configured in OpenCode through an OpenAI Responses-compatible local API for Codex clients.

## Current scope

- Automatically reads OpenCode configuration and credentials.
- Supports providers configured with `@ai-sdk/openai-compatible`, `@ai-sdk/openai`, or no explicit `npm` adapter.
- Routes models with an unambiguous `provider/model` identifier.
- Exposes `GET /v1/models` and `POST /v1/responses`.
- Translates text, image URLs, instructions, function tools, function calls, and function results.
- Supports regular JSON responses and SSE streaming, including streamed tool arguments.
- Never prints credential values.

Provider-native Anthropic, Google, and interactive OAuth adapters are intentionally outside the first compatibility layer. OAuth access tokens stored by OpenCode are usable when their upstream speaks the OpenAI protocol.

## Configuration discovery

By default:

```text
$XDG_CONFIG_HOME/opencode/opencode.jsonc
$XDG_DATA_HOME/opencode/auth.json
```

When the XDG variables are unset:

```text
~/.config/opencode/opencode.jsonc
~/.local/share/opencode/auth.json
```

Override either path with `--config`, `--auth`, `CRABCODEX_CONFIG`, or `CRABCODEX_AUTH`.

## Build and run

```bash
cargo build --release
./target/release/crabcodex doctor
./target/release/crabcodex models
./target/release/crabcodex codex-install
./target/release/crabcodex serve
```

The server listens on `127.0.0.1:10100` by default. Change it with `serve --host 0.0.0.0 --port 10100`. Binding beyond loopback can expose access to every configured provider, so only do this behind trusted network controls.

## Codex configuration

Add every discovered OpenCode model to the Codex model picker:

```bash
./target/release/crabcodex codex-install
```

This preserves the existing Codex login and default model, merges Codex's
built-in OpenAI models with OpenCode's `provider/model` entries, registers the
local aggregate Responses provider, and writes
`~/.codex/crabcodex-models.json`. Native OpenAI requests pass through with the
authorization already managed by Codex; OpenCode models use credentials from
OpenCode. Restart the Codex CLI or desktop app after synchronization.

For manual setup, configure a custom model provider in `~/.codex/config.toml`:

```toml
model_provider = "crabcodex"

[model_providers.crabcodex]
name = "CrabCodex"
base_url = "http://127.0.0.1:10100/v1"
wire_api = "responses"
requires_openai_auth = true
```

Keep using your existing Codex/OpenAI login. CrabCodex does not replace or
store it; Codex supplies it only when native OpenAI models are selected.

## HTTP examples

List models:

```bash
curl http://127.0.0.1:10100/v1/models
```

Create a response:

```bash
curl http://127.0.0.1:10100/v1/responses \
  -H 'content-type: application/json' \
  -d '{
    "model": "your-provider/your-model",
    "input": "Reply with hello",
    "stream": false
  }'
```

Stream a response:

```bash
curl -N http://127.0.0.1:10100/v1/responses \
  -H 'content-type: application/json' \
  -d '{
    "model": "your-provider/your-model",
    "input": "Reply with hello",
    "stream": true
  }'
```

## CLI

```text
crabcodex doctor
crabcodex models
crabcodex codex-install [--base-url URL]
crabcodex serve [--host HOST] [--port PORT]
```

Set `RUST_LOG=crabcodex=debug,tower_http=debug` for request diagnostics. Secrets and request authorization headers are not logged by the application.

## Architecture

- `config`: XDG discovery and OpenCode JSONC/auth parsing.
- `provider`: filtered provider registry, model lookup, headers, and credentials.
- `protocol`: Responses ↔ Chat Completions conversion and streaming state.
- `app`: Axum HTTP routes and upstream transport.

This separation keeps the core small while allowing native provider adapters to be introduced later.
