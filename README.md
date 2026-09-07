<h1 align="center">Joocode</h1>
<h3 align="center">your AI configs, everywhere.</h3>

<p align="center"><b>The OCX idea, supercharged as one fast native Rust binary.</b><br>
Reuse OpenCode, CrabCode, OCX, Hermes, Copilot, Antigravity Gemini, and OpenAI-compatible providers inside Codex, GitHub Copilot App, Antigravity, Zed, JetBrains, Claude Code, and Grok Build.</p>

<p align="center">
  <img src="assets/joocode-icon.png" alt="Joocode palm and crab logo" width="180">
</p>

<p align="center">
  <a href="https://github.com/yan-ad/joocode/releases/latest"><img src="https://img.shields.io/github/v/release/yan-ad/joocode?color=6f42c1&label=release" alt="Latest release"></a>
  <a href="https://github.com/yan-ad/joocode/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/yan-ad/joocode/ci.yml?branch=main&label=build" alt="Build status"></a>
  <a href="https://github.com/yan-ad/joocode/blob/main/LICENSE"><img src="https://img.shields.io/github/license/yan-ad/joocode?color=blue" alt="MIT license"></a>
  <img src="https://img.shields.io/badge/Rust-native-dea584?logo=rust" alt="Native Rust">
  <img src="https://img.shields.io/badge/macOS%20%7C%20Linux%20%7C%20Windows-supported-2ea44f" alt="Supported platforms">
</p>

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://raw.githubusercontent.com/yan-ad/joocode/main/install.bash | bash
jcx
```

> **`jcx` is the flagship command.** `joocode` ships as a fully compatible alias for existing scripts and installations.

```text
◈ Joocode

◆ Config: OpenCode, Joocode
⌘ IDE Target: Codex, GitHub Copilot App, Antigravity, Zed, JetBrains, Claude Code, Grok Build
● Listening: http://127.0.0.1:10100
● OpenAI Compatible: http://127.0.0.1:10100/v1

◉ Models: 30    ◇ Providers: 5

Esc to exit  ·  Tab Providers  ·  / Config
```

Joocode takes the universal local-proxy idea behind projects such as OCX and
pushes it in a different direction: **reuse the provider configurations you
already have, wire installed desktop clients automatically, and keep the whole
runtime small enough to disappear into the background.**

No second provider dashboard is required. No upstream key needs to be copied
into Codex, GitHub Copilot App, Antigravity, Zed, JetBrains, Claude Code, or Grok Build. Start `jcx` and keep using the native client UI.

## Why Joocode?

- **Config-first** — discovers OpenCode, OCX profiles, Hermes Agent, GitHub
  Copilot, Antigravity Gemini API-key mode, and Joocode's own flat
  OpenAI-compatible provider file.
- **Desktop-aware** — detects installed Codex, GitHub Copilot App, Zed,
  JetBrains, Claude Code, and Grok Build clients and
  prepares only the integrations present on the machine.
- **One native process** — a single Rust binary, one shared proxy, no Node/Bun
  runtime and no browser dashboard required.
- **Provider manager** — press `Tab` to browse saved providers, add a new
  OpenAI-compatible endpoint in a modal, or remove the selected provider. The
  catalog reloads without restarting Joocode.
- **Optional background proxy** — press `/` and toggle **Run in background** to
  choose whether closing the dashboard hands the proxy to a supervised service or
  releases the port immediately. **Auto-start after login/restart** independently
  controls whether that service returns after signing in or restarting the
  device. Use `jcx start` and `jcx stop` to control the current background session.
- **Desktop app launcher** — release installers include the Joocode palm/crab
  icon and install a platform launcher while retaining the `jcx` flagship CLI (`joocode` remains an alias).
- **Protocol bridge** — Responses API and Chat Completions, JSON and SSE,
  images, tool calls, streamed arguments, and function results.
- **Local OpenRouter-style gateway** — point any OpenAI-compatible application
  at `http://127.0.0.1:10100/v1` and use every model discovered by Joocode from
  one fully local endpoint.
- **Credential-safe** — upstream credentials stay in their original source;
  Joocode never prints authorization values.

## Performance

Joocode is intentionally narrower than OpenCodex/OCX: it focuses on provider
discovery, desktop integration, and protocol proxying rather than OCX's full web
dashboard, account pooling, and service-management feature set. The comparison
below measures the **CLI-only runtime footprint**, not feature parity.

| Metric | Joocode `0.1.9` | OpenCodex / `ocx` `2.39.0` | Difference |
| --- | ---: | ---: | ---: |
| Startup time | **5.8 ms** | 289.1 ms | **~49.8× faster** |
| Memory usage, idle | **5.8 MB** | 15.3 MB | **~62% less memory** |
| Installed bundle footprint | **8.2 MiB** | 110 MiB | **~13.4× smaller** |

**Test environment:** MacBook Pro with Apple M1, arm64, macOS 27 Golden Gate,
measured September 1, 2026. Startup is the median of 100 warm process launches
using `<command> --version`,
with output redirected; this isolates CLI/runtime startup from provider network
latency. Memory is idle RSS in CLI-only proxy mode. Bundle footprint compares
the optimized standalone Joocode binary with the locally installed OCX npm
package and its runtime dependencies. Results vary by version, build flags, and
host environment.

## Quick start

### macOS, Linux, WSL, or Git Bash (recommended)

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://raw.githubusercontent.com/yan-ad/joocode/main/install.bash | bash
jcx
```

The installer detects the platform, verifies `SHA256SUMS`, and installs to
`~/.local/bin`. Install a specific version or directory with:

```bash
curl -LsSf https://raw.githubusercontent.com/yan-ad/joocode/main/install.bash -o install.bash
JOOCODE_VERSION=0.1.9 JOOCODE_INSTALL_DIR=/usr/local/bin bash install.bash
```

If `jcx` is not found after installation, add the default installation directory
to your shell `PATH` and open a new terminal:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

### Homebrew status

The `yan-ad/tap` Homebrew tap is **not published yet**, so these commands do not
currently work:

```bash
brew tap yan-ad/tap
brew install joocode
```

Use the verified installer above until the tap repository is available. Release
artifacts and checksums are published at
[GitHub Releases](https://github.com/yan-ad/joocode/releases).

### Windows PowerShell

```powershell
irm https://raw.githubusercontent.com/yan-ad/joocode/main/install.ps1 | iex
```

Open a new PowerShell window, then run:

```powershell
jcx --version
jcx doctor
jcx
```

The installer selects x64 or ARM64, verifies the SHA-256 checksum, installs
`joocode.exe` to `~/.local/bin`, and adds that directory to the user `PATH`.
Install a specific version with `./install.ps1 -Version 0.1.9`.

### Build from source

```bash
git clone https://github.com/yan-ad/joocode.git
cd joocode
cargo build --release --locked
./target/release/jcx
```

## How it works

```text
OpenCode ────────┐
OpenCodex / OCX ─┤
OCX profiles ────┤
Hermes ──────────┤                    ┌─ Codex Responses API
GitHub Copilot ──┤                    ├─ GitHub Copilot App BYOK
Antigravity Gemini┼─► Joocode proxy ───┼─ Zed / JetBrains OpenAI-compatible
providers.json ──┘                    ├─ Claude Code Messages API
                                      └─ Grok Build custom models
```

Joocode builds one source-aware model registry, exposes it through
`GET /v1/models`, and routes each request back to the provider that owns the
selected model. Model IDs remain explicit:

```text
provider/model
ocx/provider/model
ocx-profile/provider/model
hermes/provider/model
copilot/model
antigravity/gemini/model
joocode/provider/model
```

## Target support and probability

These probabilities estimate how likely each integration is to remain reliable
across upstream client releases. They are not uptime guarantees.

| Target | Probability | Current behavior |
| --- | ---: | --- |
| Codex | **98%** | Full catalog and Responses API integration |
| GitHub Copilot App | **90%** | Automatic BYOK provider/model sync through its local database and OS credential store |
| Zed | **95%** | Full OpenAI-compatible model catalog and local credential registration |
| Claude Code | **85%** | Anthropic Messages gateway, model discovery, JSON/SSE and tools |
| JetBrains | **80%** | OpenAI-compatible endpoint; credential stays in the IDE-managed store |
| Grok Build | **90%** | Writes/removes Joocode custom models in `~/.grok/config.toml` |
| Antigravity | **80% on macOS 2.12.x / 70% source** | Official Gemini source plus an opt-in, reversible patched app clone for custom Joocode models |

Press `/` in the dashboard to open the configuration modal:

```text
Setting
  Auto-start after login/restart (On/Off)
  Run in background              (On/Off)

Detected Providers
  OpenCode                    (On/Off)
  CrabCode                    (On/Off)
  OpenCodex                   (On/Off)
  Hermes                      (On/Off)
  GitHub Copilot              (On/Off)
  Antigravity                 (On/Off)

Proxy to
  Codex                      (On/Off)
  GitHub Copilot App         (On/Off)
  JetBrains                  (On/Off)
  Antigravity                (On/Off · patched app)
  Zed                        (On/Off)
  Claude Code                (On/Off · experimental)
  Grok Build                 (On/Off)
```

Navigate with `↑/↓` and press `Space` to toggle. Detected-provider changes reload
the model registry and desktop catalogs immediately. Preferences are stored in
`~/.config/joocode/settings.json`; explicit `--source` flags override the saved
detected-provider choices for scripting.

## Run Joocode

Start the automatic desktop mode:

```bash
jcx
```

Joocode detects installed clients, starts one proxy at
`http://127.0.0.1:10100/v1`, and opens the terminal dashboard. Press `Esc` or
`Ctrl-C` to stop gracefully. In a non-interactive terminal it falls back to
headless logs.

Force every integration even when its application is not detected:

```bash
jcx --all
```

Choose a different listener:

```bash
jcx --host 127.0.0.1 --port 10200 \
  --base-url http://127.0.0.1:10200/v1
```

Binding beyond loopback can expose access to every configured provider. Only do
so behind trusted network controls.

## Manage OpenAI-compatible providers

Press `Tab` in the dashboard:

- Use `↑` and `↓` to select a saved provider.
- Press `Enter` to open the new-provider modal.
- Press `Del` to remove the selected provider.
- Press `\\` to select its default model. Joocode syncs that model to Zed's
  default chat model and commit-message generator. Generated commit messages
  receive scoped [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/)
  instructions; normal Zed chat prompts are unchanged.

The create modal asks for:

1. The OpenAI-compatible base URL, including `/v1` when required.
2. The API key. It remains masked in the terminal.
3. Joocode requests `GET /models`, saves the provider, and reloads the catalog.

The provider is saved with private file permissions to:

```text
~/.config/joocode/providers.json
```

The flat JSON format is deliberately simple:

```json
[
  {
    "name": "openrouter",
    "base_url": "https://openrouter.ai/api/v1",
    "api_key": "...",
    "models": ["anthropic/claude-sonnet-4"]
  }
]
```

The running registry and detected desktop catalogs reload automatically. Override
the path with `JOOCODE_PROVIDERS=/custom/providers.json`.

## Failover combos

Create virtual models that try multiple routed models in order:

```text
~/.config/joocode/combos.json
```

```json
{
  "combos": [
    {
      "name": "coding",
      "models": [
        "crabcode/clip/claude-sonnet-4.6",
        "opencode/openrouter/gpt-5.5"
      ]
    }
  ]
}
```

The virtual model appears as `combo/coding`. Joocode retries transient failures
on each candidate and moves to the next model for authentication, rate-limit,
capacity, timeout, or transport failures. Invalid requests do not fail over.

Weighted round-robin uses object entries while preserving failover to the other
candidates when the selected provider is unavailable:

```json
{
  "combos": [
    {
      "name": "balanced",
      "strategy": "weighted-round-robin",
      "models": [
        { "model": "crabcode/clip/claude-sonnet-4.6", "weight": 3 },
        { "model": "opencode/openrouter/gpt-5.5", "weight": 1 }
      ]
    }
  ]
}
```

Override the file with `JOOCODE_COMBOS=/custom/combos.json`.

## Provider discovery

All detected sources are enabled by default. Disable individual sources from
`/ Config` → **Detected Providers**, or restrict one invocation with repeated or
comma-separated `--source` values:

```bash
jcx --source opencode,crabcode,hermes models
jcx --source copilot doctor
jcx --source ocx serve
```

Supported values include `auto`, `opencode`, `crabcode`, `ocx`, `hermes`,
`copilot`, `antigravity`, and `joocode`.
Joocode's local `providers.json` source remains available for live additions.

### OpenCode

Joocode reads:

```text
$XDG_CONFIG_HOME/opencode/opencode.jsonc
$XDG_DATA_HOME/opencode/auth.json
```

With the usual fallbacks:

```text
~/.config/opencode/opencode.jsonc
~/.local/share/opencode/auth.json
```

Override them with `--config`, `--auth`, `JOOCODE_CONFIG`, or `JOOCODE_AUTH`.
Legacy `JOC_*` and `CRABCODEX_*` variables remain supported for migration.

Compatible OpenCode entries currently use `@ai-sdk/openai-compatible`,
`@ai-sdk/openai`, or no explicit `npm` adapter.

### CrabCode

CrabCode reuses the normal OpenCode provider configuration but keeps its own
credential store. Joocode combines:

```text
$XDG_CONFIG_HOME/opencode/opencode.jsonc
$XDG_STATE_HOME/crabcode/auth.json
```

With the usual fallbacks:

```text
~/.config/opencode/opencode.jsonc
~/.local/state/crabcode/auth.json
```

Override the CrabCode auth path with `CRABCODE_AUTH`. Models use
`crabcode/provider/model`, allowing OpenCode and CrabCode to expose the same
provider with separate credentials without collisions.

### OpenCodex / OCX

Joocode reads the OpenCodex configuration directory directly:

```text
$OPENCODEX_HOME
~/.opencodex
```

Recognized configuration files include:

```text
config.json
catalog-backup.json
catalog-backup-*.json
codex-runtime.json
codex-runtime-clamp.json
runtime-port.json
```

`config.json` supplies providers, model lists, aliases, model limits, selection
rules, and the running OCX port. Catalog backups enrich native Codex model
metadata. Runtime clamp data is applied to affected model capabilities.
When present, `runtime-port.json` supplies the active OCX listener without
loading its private attestation field.

Models use `ocx/provider/model`. Requests are sent through the local OCX proxy,
so OCX remains authoritative for OAuth refresh, API-key pools, routing, adapter
translation, and account failover. Joocode does not copy those managed secrets.

Runtime state, usage logs, quota caches, response history, and token files such
as `admin-api-token` are deliberately not interpreted as model catalogs.

### OCX OpenCode profiles

Every global OpenCode profile under the following path is discovered:

```text
$XDG_CONFIG_HOME/opencode/profiles/*/opencode.jsonc
```

Models are exposed as `ocx-PROFILE/provider/model` and reuse the normal OpenCode
auth store, preventing collisions between profiles.

### Hermes Agent

Joocode reads `~/.hermes/config.yaml` or `$HERMES_HOME/config.yaml`, plus
`~/.hermes/.env`. Modern `providers:` and legacy `custom_providers:` entries are
supported for OpenAI Chat-compatible endpoints. Models become
`hermes/provider/model`.

Inline `api_key`, `${ENV_VAR}`, and `key_env` credentials are supported.
Dynamic `key_cmd` and provider-native Anthropic transports are not yet proxied.

### GitHub Copilot

Joocode checks `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN`, supported
Copilot macOS Keychain entries, Copilot's plaintext fallback, and finally
`gh auth token`. Run `copilot login` when `jcx doctor` reports an auth error.
Classic `ghp_` PATs are ignored because Copilot does not support them.

The account's live model catalog is exposed as `copilot/model`; exchanged tokens
remain in memory only.

## Desktop integrations

### Codex

Synchronize manually when needed:

```bash
jcx codex-install
```

Joocode preserves the existing Codex login and default model, merges native
OpenAI models with discovered entries, registers the local Responses provider,
and writes `~/.codex/joocode-models.json`. Native OpenAI requests keep using the
authorization managed by Codex.

Joocode also marks routed models for **direct tool exposure**. When Codex's
Browser or Computer Use plugin is installed and enabled, its local MCP tools are
sent to compatible non-OpenAI models as ordinary function tools instead of being
hidden behind OpenAI-only dynamic tool search. Model identities are not spoofed:
the selected `provider/model` remains visible and is routed normally.

Tool availability does not guarantee model quality. The upstream model must
reliably support function calling, and Computer Use works best with a model that
can reason over image inputs. Codex still owns tool execution, permissions,
browser isolation, cursor movement, and user confirmations; Joocode only carries
the model request and tool-call/result payloads.

Upstream models may only call tools declared by the client request. Hallucinated
tool names are rejected; streaming Responses become incomplete and Anthropic
streams emit an error rather than forwarding an unauthorized tool execution.

`POST /v1/responses/compact` is supported for native OpenAI/ChatGPT models and
preserves Codex-managed authentication. Routed Chat Completions models return an
explicit `501` because the official compaction item is opaque/encrypted and
cannot be emulated safely without a native Responses upstream.

Manual provider configuration:

```toml
model_provider = "joocode"

[model_providers.joocode]
name = "Joocode"
base_url = "http://127.0.0.1:10100/v1"
wire_api = "responses"
requires_openai_auth = true
```

### Zed

When Zed is installed, automatic desktop mode writes the complete discovered
catalog into Zed's OpenAI-compatible provider settings. On macOS, the system may
ask for your password to authorize Keychain access for the local tunnel.
On Windows, Joocode updates `%APPDATA%\Zed\settings.json` and registers the same
placeholder in Windows Credential Manager so Zed marks the provider as connected.
Joocode stores only a harmless local placeholder key—never an upstream provider
credential. Existing providers, themes, telemetry settings, and an existing Zed
default model remain unchanged. Restart Zed once after the first install or model
catalog sync. When a Joocode default model is selected, Zed's commit-message
generator is also configured with Conventional Commits 1.0.0 instructions while
preserving any existing custom commit instructions.

Joocode fingerprints the Zed fields it owns in
`~/.local/state/joocode/integrations.json`. Unrelated Zed settings can still be
edited freely. If Joocode's own provider or default-model fields are changed
externally, the next sync reports a conflict instead of overwriting them.

### GitHub Copilot App

When the GitHub Copilot App is installed and has been opened at least once,
automatic desktop mode creates a **Joocode** BYOK provider and synchronizes the
complete discovered model catalog. The app's built-in GitHub Copilot provider,
other custom endpoints, accounts, sessions, and selected model remain untouched.

Joocode stores only the local placeholder `joocode-local` in the operating
system credential store. Upstream provider credentials remain in their original
OpenCode, CrabCode, OCX, Hermes, Copilot, or Joocode provider configuration.
Restart the GitHub Copilot App after the first sync or after adding/removing
models so its in-memory model catalog reloads.

### Antigravity

Antigravity's model picker is backed by Google's internal Cloud Code protocol,
not a public OpenAI-compatible provider setting. Joocode therefore uses an
explicit, reversible integration inspired by Antigravity manager projects:

```bash
jcx antigravity patch
jcx antigravity status
jcx antigravity restore
```

On macOS, `patch` creates `~/Applications/Antigravity Joocode.app` from the
installed Google application, changes only the language-server API endpoints to
the local Joocode bridge, updates ASAR integrity metadata, and ad-hoc signs the
copy. Google's original `/Applications/Antigravity.app` remains untouched.
Open **Antigravity Joocode** after patching; its model picker merges built-in
models with the complete Joocode catalog. Toggle **Antigravity** under
`/ Config → Proxy to` to install or remove the patched app.

Quit Antigravity before patching. Antigravity updates may require running
`jcx antigravity patch` again. The bridge preserves native Google requests and
only translates models whose IDs belong to the Joocode registry.

### JetBrains AI Assistant

Use the built-in endpoint when JetBrains asks for an OpenAI-compatible provider:

- **Base URL:** `http://127.0.0.1:10100/v1`
- **API key:** any non-empty local value, such as `joocode`
- **Model:** any discovered ID from `jcx models`

JetBrains keeps the local placeholder in its managed credential store; upstream
credentials remain in their original source.

## HTTP API

Joocode can be used outside the built-in desktop integrations as a fully local
OpenAI-compatible gateway. Configure any compatible application with:

```text
Base URL: http://127.0.0.1:10100/v1
API key:  any non-empty local placeholder
Model:    provider/model
```

Joocode ignores the placeholder client key and uses credentials from the
original provider source. It does not expose provider credentials to the client.

```bash
curl http://127.0.0.1:10100/healthz
curl http://127.0.0.1:10100/readyz
curl http://127.0.0.1:10100/api/status
curl http://127.0.0.1:10100/api/providers
curl http://127.0.0.1:10100/api/metrics
curl http://127.0.0.1:10100/v1/models
```

`/healthz` reports process liveness. `/readyz` reports whether the provider
registry can route requests and returns model/provider counts plus `ready`,
`degraded`, or `failed` status. `/api/status` exposes privacy-safe in-memory
uptime and request counters plus passive provider runtime state: active requests,
concurrency capacity, and cooldown time. It never stores prompts, bodies, or
response content.

`/api/providers` exposes the non-secret logical provider/model catalog, source
discovery reports, and passive runtime availability. Provider URLs, headers,
and credentials are never included.

`/api/metrics` exposes the same privacy-safe process, request-counter, registry,
and provider-runtime data in Prometheus text format. It does not contain request
content, provider URLs, headers, or credentials.

```bash
jcx stats
jcx reload
```

`jcx reload` atomically re-discovers enabled provider sources in the running
proxy. If discovery fails or produces an empty registry, the existing registry
remains active. Remote management calls use the same `JOOCODE_API_AUTH_TOKEN`.

Create a response:

```bash
curl http://127.0.0.1:10100/v1/responses \
  -H 'content-type: application/json' \
  -d '{
    "model": "provider/model",
    "input": "Reply with hello",
    "stream": false
  }'
```

Use `POST /v1/chat/completions` for OpenAI-compatible clients. Both endpoints
support SSE streaming and tool calls.

### LAN or remote binding

Loopback keeps the zero-configuration placeholder-key behavior. Binding to a
LAN/non-loopback interface requires a real Joocode admission token:

```bash
export JOOCODE_API_AUTH_TOKEN='replace-with-a-long-random-token'
export JOOCODE_ALLOWED_ORIGINS='https://app.example.com,https://admin.example.com'
jcx serve --host 0.0.0.0 --port 10100
```

Remote clients must send either:

```text
Authorization: Bearer <JOOCODE_API_AUTH_TOKEN>
```

or:

```text
x-joocode-api-key: <JOOCODE_API_AUTH_TOKEN>
```

Joocode rejects non-loopback startup without `JOOCODE_API_AUTH_TOKEN`. Remote
CORS is disabled unless origins are listed in `JOOCODE_ALLOWED_ORIGINS`.
Admission credentials are removed before forwarding requests upstream.

Request bodies are limited to 16 MiB by default. Override the limit in bytes:

```bash
export JOOCODE_MAX_REQUEST_BYTES=33554432
```

Streaming upstreams have a 90-second idle timeout. A stalled Responses stream is
reported as `response.incomplete`; an Anthropic Messages stream receives an
explicit error event instead of a false successful completion. Override it with:

```bash
export JOOCODE_STREAM_IDLE_TIMEOUT_SECONDS=120
export JOOCODE_MAX_SSE_EVENT_BYTES=1048576
export JOOCODE_MAX_TOOL_ARGUMENT_BYTES=1048576
```

Oversized SSE events and accumulated streamed tool arguments are terminated with
explicit incomplete/error semantics. Dropping the client response body also
drops the upstream stream immediately.

Configure upstream request retries when needed:

```bash
export JOOCODE_RETRY_ATTEMPTS=3
export JOOCODE_RETRY_INITIAL_MS=250
export JOOCODE_RETRY_MAX_MS=2000
export JOOCODE_PROVIDER_CONCURRENCY=8
export JOOCODE_PROVIDER_COOLDOWN_MS=1000
```

Joocode retries transport failures and HTTP `408`, `429`, `500`, `502`, `503`,
and `504`, while respecting numeric `Retry-After` headers. Streaming is retried
only before a successful upstream response begins; emitted stream data is never
replayed.

Concurrency is bounded per discovered provider and the permit is held until the
response body or stream finishes. Providers that exhaust retries enter a short
cooldown; combo routes skip cooling candidates while direct model requests wait.

## CLI

```text
jcx [--source SOURCE] [--host HOST] [--port PORT]
jcx --all
jcx doctor
jcx models
jcx codex-install [--base-url URL]
jcx serve [--host HOST] [--port PORT]
jcx upgrade [--version VERSION]
```

Set `RUST_LOG=joocode=debug,tower_http=debug` for diagnostics. Joocode does not
log secrets or request authorization headers.

### Upgrade

```bash
jcx upgrade
jcx upgrade --version 0.2.0
```

All platforms download the matching archive and `SHA256SUMS` before replacing
the executable. Linux and macOS replace the binary atomically. On Windows,
Joocode stages a PowerShell helper that waits for the running process to exit,
replaces both `jcx.exe` and `joocode.exe`, and relaunches the dashboard when the
upgrade was accepted from the live update prompt. The installer remains the
fallback when the current installation directory is not writable.

### Uninstall

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://raw.githubusercontent.com/yan-ad/joocode/main/uninstall.sh | sh
```

The uninstaller removes the binary and preserves provider credentials plus
Codex/Zed settings by default.

## Architecture

- `sources` — OpenCode, OCX, Hermes, Copilot, and flat-file discovery.
- `provider` — merged registry, source-aware routes, headers, and credentials.
- `protocol` — Responses ↔ Chat Completions conversion and streaming state.
- `desktop` — installed-client detection and catalog synchronization.
- `dashboard` — Ratatui status UI, provider manager, and modal provider setup.
- `app` — Axum routes, upstream transport, and hot-reloadable registry.

This separation keeps the proxy core small while allowing additional config
sources and desktop targets to be added independently.

## Roadmap

See [OpenCodex gap analysis](docs/opencodex-gap-analysis.md) for the maintained
capability comparison, security priorities, routing roadmap, and features that
Joocode intentionally does not copy.

## Releasing

From a clean and synchronized `main`:

```bash
cargo xtask release
```

The native Rust release task bumps the patch version by default, updates Cargo metadata, runs
locked format/Clippy/tests/build checks, commits, creates an annotated tag, and
pushes both `main` and the tag.

```bash
cargo xtask release --bump minor
cargo xtask release --bump major
cargo xtask release --version 1.2.3
cargo xtask release --dry-run
```

Other repository tooling is native Rust as well:

```bash
cargo xtask check
cargo xtask release-notes v1.2.3 RELEASE_NOTES.md
cargo xtask homebrew-formula v1.2.3 SHA256SUMS
```

Release CI builds Linux, macOS, and Windows archives and publishes checksums plus
a generated Homebrew formula. Publishing `yan-ad/tap` additionally requires the
`yan-ad/homebrew-tap` repository and a configured `HOMEBREW_TAP_TOKEN`.

## License

[MIT](LICENSE)
