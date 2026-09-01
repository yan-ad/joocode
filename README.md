# Joocode — JustOpenCode

A small native Rust bridge that makes models from your existing AI tools
available to **ChatGPT Codex**, **Zed**, and **JetBrains AI Assistant**. It can
discover providers from OpenCode, OCX profiles, Hermes Agent, and GitHub
Copilot, then expose them without duplicating upstream credentials.

## Current scope

- Automatically discovers OpenCode, OCX, Hermes Agent, and GitHub Copilot.
- Makes compatible providers available to ChatGPT Codex, Zed, and JetBrains AI Assistant.
- Supports providers configured with `@ai-sdk/openai-compatible`, `@ai-sdk/openai`, or no explicit `npm` adapter.
- Routes models with source-aware identifiers such as `provider/model`,
  `hermes/provider/model`, `ocx-profile/provider/model`, and `copilot/model`.
- Exposes `GET /v1/models` and `POST /v1/responses`.
- Translates text, image URLs, instructions, function tools, function calls, and function results.
- Supports regular JSON responses and SSE streaming, including streamed tool arguments.
- Never prints credential values.

OpenCode, OCX, and Hermes entries currently need an OpenAI Chat
Completions-compatible upstream. GitHub Copilot uses its official token exchange
and live model catalog; Joocode keeps exchanged tokens in memory only.

## Roadmap

Joocode currently integrates with **ChatGPT Codex**, **Zed**, and **JetBrains
AI Assistant**. Support for additional AI clients is planned, using the same
existing provider configuration and source-qualified model identifiers.

## Provider source discovery

All available sources are enabled by default. Select sources explicitly by
repeating or comma-separating `--source`:

```bash
joocode --source opencode,hermes models
joocode --source copilot doctor
joocode --source ocx zed
```

Supported source values are `auto`, `opencode`, `ocx`, `hermes`, and `copilot`.

### OpenCode

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

Override either path with `--config`, `--auth`, `JOOCODE_CONFIG`, or
`JOOCODE_AUTH`. The prior `JOC_CONFIG` and `JOC_AUTH` names remain supported
for upgrades.

### OCX profiles

Joocode scans every global OCX profile with an OpenCode config at:

```text
$XDG_CONFIG_HOME/opencode/profiles/*/opencode.jsonc
```

Models are exposed as `ocx-PROFILE/provider/model` and use the normal OpenCode
auth store. This avoids collisions between profiles that define the same
provider name.

### Hermes Agent

Joocode reads `~/.hermes/config.yaml` (or `$HERMES_HOME/config.yaml`) and
`~/.hermes/.env`. Modern `providers:` and legacy `custom_providers:` entries
are supported when they declare an OpenAI Chat-compatible endpoint and model
list. Models are exposed as `hermes/provider/model`. Inline `api_key`,
`${ENV_VAR}`, and `key_env` credentials are supported; dynamic `key_cmd` and
provider-native Anthropic transports are not yet proxied.

### GitHub Copilot

Joocode checks environment variables (`COPILOT_GITHUB_TOKEN`, `GH_TOKEN`,
`GITHUB_TOKEN`), the Copilot macOS Keychain entries, Copilot's documented
plaintext fallback, then `gh auth token`. On Linux and Windows, use a supported
token environment variable when Copilot stores its login only in the OS
credential manager. Run `copilot login` if `joocode doctor` reports a Copilot
auth error. Classic `ghp_` PATs are ignored because Copilot does not support them.
Available account models are fetched from Copilot's live catalog and exposed as
`copilot/model`.

## Build and run

### Install a release

On macOS, Linux, WSL, or Git Bash:

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://raw.githubusercontent.com/yan-ad/joocode/main/install.bash | bash
```

The installer detects the host target, verifies the release checksum, and
installs `joocode` to `~/.local/bin` by default. Install a specific version or
directory with:

```bash
curl -LsSf https://raw.githubusercontent.com/yan-ad/joocode/main/install.bash -o install.bash
JOOCODE_VERSION=0.1.6 JOOCODE_INSTALL_DIR=/usr/local/bin bash install.bash
```

`install.sh` remains available as a backward-compatible alias for
`install.bash`.

### Install on Windows

In PowerShell, run:

```powershell
irm https://raw.githubusercontent.com/yan-ad/joocode/main/install.ps1 | iex
```

It selects the x64 or ARM64 release automatically, verifies its SHA-256
checksum, installs `joocode.exe` to `~/.local/bin`, and adds that directory to
your user `PATH`. Open a new PowerShell window, then verify the installation:

```powershell
joocode --version
joocode doctor
joocode --source opencode,hermes,copilot models
```

To install a specific version, download the script first and run
`./install.ps1 -Version 0.1.6`. Windows self-upgrade and uninstall are not yet
available; replace or remove the installed `joocode.exe` manually.

### Uninstall

For standalone installations made with `install.sh`, download and run the
uninstaller:

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://raw.githubusercontent.com/yan-ad/joocode/main/uninstall.sh | sh
```

It removes only the `joocode` binary from `~/.local/bin` by default and keeps
your OpenCode credentials plus Codex and Zed settings. Use
`JOOCODE_INSTALL_DIR=/custom/bin sh uninstall.sh --yes` for a custom install
directory or non-interactive use. If installed with Homebrew, use
`brew uninstall joocode` instead.

### Build from source

```bash
cargo build --release
./target/release/joocode doctor
./target/release/joocode models
./target/release/joocode codex-install
./target/release/joocode zed
./target/release/joocode serve
./target/release/joocode --all
```

The server listens on `127.0.0.1:10100` by default. Change it with `serve --host 0.0.0.0 --port 10100`. Binding beyond loopback can expose access to every configured provider, so only do this behind trusted network controls.

## Codex configuration

Add every discovered model to the Codex model picker:

```bash
./target/release/joocode codex-install
```

This preserves the existing Codex login and default model, merges Codex's
built-in OpenAI models with Joocode's discovered entries, registers the
local aggregate Responses provider, and writes
`~/.codex/joocode-models.json`. Native OpenAI requests pass through with the
authorization already managed by Codex; routed models use credentials from
their original source. Restart the Codex CLI or desktop app after synchronization.

For manual setup, configure a custom model provider in `~/.codex/config.toml`:

```toml
model_provider = "joocode"

[model_providers.joocode]
name = "JustOpenCode"
base_url = "http://127.0.0.1:10100/v1"
wire_api = "responses"
requires_openai_auth = true
```

Keep using your existing Codex/OpenAI login. JustOpenCode does not replace or
store it; Codex supplies it only when native OpenAI models are selected.

## Configure all desktop clients

Configure supported desktop integrations and start **one shared local proxy**:

```bash
joocode --all
```

This updates Joocode-managed Codex and Zed settings, prints the one-time
JetBrains AI Assistant setup values, and serves all clients at
`http://127.0.0.1:10100/v1`. Every client receives the same discovered
`provider/model` list. Use `--host`, `--port`, and `--base-url` together to
use a different local endpoint.

## Zed integration

Configure Zed and start the local proxy in one command:

```bash
joocode zed
```

This automatically adds a `joocode` OpenAI-compatible provider to Zed’s
settings and exposes every discovered model with its routable ID. It preserves
all other Zed settings and never copies upstream credentials into Zed; the local
bridge continues reading them from their source. On macOS, you may be asked for
your password to authorize Keychain access for the local Zed tunnel. Joocode
stores only a harmless local placeholder key there so Zed shows the provider;
it does not store or expose upstream credentials. Restart Zed once if it
was already open.
Use `ZED_SETTINGS_PATH=/path/to/settings.json joocode zed` for a nonstandard
settings location.

## JetBrains AI Assistant integration

Start the OpenAI-compatible proxy for JetBrains:

```bash
joocode jetbrains
```

The command prints the exact provider values and keeps the proxy running. In
your JetBrains IDE, open **Settings | Tools | AI Assistant | Providers & API
keys**, add an **OpenAI-compatible** provider, and use:

- **Base URL:** `http://127.0.0.1:10100/v1`
- **API key:** any non-empty local value, such as `joocode`
- **Model:** a discovered `provider/model` ID from `joocode models`

JetBrains stores provider keys in its managed credential store, so Joocode
does not write IDE settings or copy upstream credentials into it. The proxy
continues to read credentials from the selected source. Select the configured model
under **Models Assignment** to use it in AI Assistant features.

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
joocode doctor
joocode --all [--base-url URL] [--host HOST] [--port PORT]
joocode models
joocode codex-install [--base-url URL]
joocode zed [--base-url URL] [--host HOST] [--port PORT]
joocode jetbrains [--base-url URL] [--host HOST] [--port PORT]
joocode serve [--host HOST] [--port PORT]
joocode upgrade [--version VERSION]
```

Set `RUST_LOG=joocode=debug,tower_http=debug` for request diagnostics. Secrets and request authorization headers are not logged by the application.

### Upgrade

Upgrade an installed Linux or macOS binary to the latest checksummed GitHub
release:

```bash
joocode upgrade
```

Install a specific release when needed:

```bash
joocode upgrade --version 0.2.0
```

The command downloads the matching platform archive and `SHA256SUMS`, verifies
the archive, and atomically replaces the running executable. The executable's
install directory must be writable. Windows users should download the release
ZIP because in-place self-upgrade is not currently supported there.

## Architecture

- `config`: OpenCode JSONC/auth parsing.
- `sources`: OpenCode, OCX, Hermes, and Copilot discovery adapters.
- `provider`: merged provider registry, route lookup, headers, and credentials.
- `protocol`: Responses ↔ Chat Completions conversion and streaming state.
- `app`: Axum HTTP routes and upstream transport.

This separation keeps the core small while allowing native provider adapters to be introduced later.

## Releasing

Create and publish the next patch release from a clean, up-to-date `main`:

```bash
make release
```

`make release` updates `Cargo.toml` and `Cargo.lock`, runs the locked format,
Clippy, test, and release-build checks, creates a `chore: release vX.Y.Z`
commit, creates an annotated matching tag, and pushes both `main` and the tag.

Choose a version increment or preview the operation without writing changes:

```bash
make release BUMP=minor
make release BUMP=major
make release VERSION=1.2.3
make release DRY_RUN=1
```

The release workflow builds Linux, macOS, and Windows archives, generates
`SHA256SUMS`, publishes a checksum-pinned `joocode.rb` Homebrew formula,
and publishes a GitHub release with generated release notes.

## Homebrew

Install from the Joocode Homebrew tap:

```bash
brew tap yan-ad/tap
brew install joocode
```

The tap adds Joocode's formula to your local Homebrew installation, so the
second command can use the short `joocode` name. To install in one command:

```bash
brew install yan-ad/tap/joocode
```

Maintainers can publish formula updates automatically to
`yan-ad/homebrew-tap` with a `HOMEBREW_TAP_TOKEN` secret. Set the optional
`HOMEBREW_TAP_REPOSITORY` repository variable to use a different tap.
