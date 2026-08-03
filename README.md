# Joc — JustOpenCode

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

Override either path with `--config`, `--auth`, `JOC_CONFIG`, or `JOC_AUTH`.

## Build and run

### Install a release

On macOS or Linux:

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://raw.githubusercontent.com/yan-ad/joc/main/install.sh | sh
```

The installer detects the host target, verifies the release checksum, and
installs `joc` to `~/.local/bin` by default. Install a specific version or
directory with:

```bash
JOC_VERSION=0.1.0 JOC_INSTALL_DIR=/usr/local/bin sh install.sh
```

Windows ZIP archives are attached to each GitHub release.

### Build from source

```bash
cargo build --release
./target/release/joc doctor
./target/release/joc models
./target/release/joc codex-install
./target/release/joc zed
./target/release/joc serve
```

The server listens on `127.0.0.1:10100` by default. Change it with `serve --host 0.0.0.0 --port 10100`. Binding beyond loopback can expose access to every configured provider, so only do this behind trusted network controls.

## Codex configuration

Add every discovered OpenCode model to the Codex model picker:

```bash
./target/release/joc codex-install
```

This preserves the existing Codex login and default model, merges Codex's
built-in OpenAI models with OpenCode's `provider/model` entries, registers the
local aggregate Responses provider, and writes
`~/.codex/joc-models.json`. Native OpenAI requests pass through with the
authorization already managed by Codex; OpenCode models use credentials from
OpenCode. Restart the Codex CLI or desktop app after synchronization.

For manual setup, configure a custom model provider in `~/.codex/config.toml`:

```toml
model_provider = "joc"

[model_providers.joc]
name = "JustOpenCode"
base_url = "http://127.0.0.1:10100/v1"
wire_api = "responses"
requires_openai_auth = true
```

Keep using your existing Codex/OpenAI login. JustOpenCode does not replace or
store it; Codex supplies it only when native OpenAI models are selected.

## Zed integration

Configure Zed and start the local proxy in one command:

```bash
joc zed
```

This automatically adds a `joc` OpenAI-compatible provider to Zed’s
settings and exposes every OpenCode model as `provider/model`. It preserves all
other Zed settings and never copies credentials into Zed; the local bridge
continues reading them from OpenCode. Restart Zed once if it was already open.
Use `ZED_SETTINGS_PATH=/path/to/settings.json joc zed` for a nonstandard
settings location.

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
joc doctor
joc models
joc codex-install [--base-url URL]
joc zed [--base-url URL] [--host HOST] [--port PORT]
joc serve [--host HOST] [--port PORT]
joc upgrade [--version VERSION]
```

Set `RUST_LOG=joc=debug,tower_http=debug` for request diagnostics. Secrets and request authorization headers are not logged by the application.

### Upgrade

Upgrade an installed Linux or macOS binary to the latest checksummed GitHub
release:

```bash
joc upgrade
```

Install a specific release when needed:

```bash
joc upgrade --version 0.2.0
```

The command downloads the matching platform archive and `SHA256SUMS`, verifies
the archive, and atomically replaces the running executable. The executable's
install directory must be writable. Windows users should download the release
ZIP because in-place self-upgrade is not currently supported there.

## Architecture

- `config`: XDG discovery and OpenCode JSONC/auth parsing.
- `provider`: filtered provider registry, model lookup, headers, and credentials.
- `protocol`: Responses ↔ Chat Completions conversion and streaming state.
- `app`: Axum HTTP routes and upstream transport.

This separation keeps the core small while allowing native provider adapters to be introduced later.

## Releasing

1. Update the version in `Cargo.toml` and `Cargo.lock`.
2. Push the changes to `main` and wait for CI to pass.
3. Create and push a matching tag, for example `git tag v0.1.0 && git push origin v0.1.0`.

The release workflow builds Linux, macOS, and Windows archives, generates
`SHA256SUMS`, publishes a checksum-pinned `joc.rb` Homebrew formula,
and publishes a GitHub release with generated release notes.

## Homebrew

Install directly from the latest GitHub release:

```bash
brew install https://github.com/yan-ad/joc/releases/latest/download/joc.rb
```

Or install from the maintainer tap:

```bash
brew tap yan-ad/tap
brew install yan-ad/tap/joc
```

The release workflow can update `yan-ad/homebrew-tap` automatically when the
repository has a `HOMEBREW_TAP_TOKEN` secret. The token must be allowed to
write to the tap repository. Set the optional `HOMEBREW_TAP_REPOSITORY`
repository variable to publish to a different tap.
