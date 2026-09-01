use std::{net::IpAddr, path::PathBuf};

use clap::{Parser, Subcommand};

use crate::sources::SourceKind;

#[derive(Debug, Parser)]
#[command(
    name = "joocode",
    version,
    about,
    long_about = "Discover configured AI providers, auto-detect installed desktop clients, and run one local proxy. Running without a subcommand opens the Joocode dashboard."
)]
pub struct Cli {
    /// Override the OpenCode configuration path.
    #[arg(long, env = "JOOCODE_CONFIG")]
    pub config: Option<PathBuf>,

    /// Override the OpenCode authentication path.
    #[arg(long, env = "JOOCODE_AUTH")]
    pub auth: Option<PathBuf>,

    /// Provider configuration sources to load (repeat or comma-separate).
    #[arg(
        long = "source",
        value_enum,
        value_delimiter = ',',
        default_value = "auto"
    )]
    pub sources: Vec<SourceKind>,

    /// Force every supported desktop integration and start one shared local proxy.
    #[arg(long)]
    pub all: bool,

    /// URL where desktop applications can reach the local API when using --all.
    #[arg(long, default_value = "http://127.0.0.1:10100/v1", requires = "all")]
    pub base_url: String,

    /// Interface to bind the shared proxy to when using --all.
    #[arg(long, default_value = "127.0.0.1", requires = "all")]
    pub host: IpAddr,

    /// Port for the shared proxy when using --all.
    #[arg(long, default_value_t = 10100, requires = "all")]
    pub port: u16,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_invocation_starts_auto_detect_mode() {
        let cli = Cli::try_parse_from(["joocode"]).unwrap();
        assert!(!cli.all);
        assert!(cli.command.is_none());
        assert_eq!(cli.sources, vec![SourceKind::Auto]);
    }

    #[test]
    fn all_mode_accepts_shared_proxy_options() {
        let cli = Cli::try_parse_from([
            "joocode",
            "--all",
            "--host",
            "0.0.0.0",
            "--port",
            "1234",
            "--base-url",
            "http://localhost:1234/v1",
        ])
        .unwrap();

        assert!(cli.all);
        assert_eq!(cli.host, "0.0.0.0".parse::<IpAddr>().unwrap());
        assert_eq!(cli.port, 1234);
        assert_eq!(cli.base_url, "http://localhost:1234/v1");
        assert_eq!(cli.sources, vec![SourceKind::Auto]);
    }

    #[test]
    fn accepts_multiple_provider_sources() {
        let cli = Cli::try_parse_from(["joocode", "--source", "opencode,hermes,copilot", "models"])
            .unwrap();
        assert_eq!(
            cli.sources,
            vec![
                SourceKind::OpenCode,
                SourceKind::Hermes,
                SourceKind::Copilot
            ]
        );
    }
}

#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// Start the local Codex-compatible HTTP server.
    Serve {
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        #[arg(long, default_value_t = 10100)]
        port: u16,
    },
    /// Start a JetBrains AI Assistant-compatible proxy and print provider setup values.
    Jetbrains {
        /// URL where JetBrains can reach the local OpenAI-compatible API.
        #[arg(long, default_value = "http://127.0.0.1:10100/v1")]
        base_url: String,
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        #[arg(long, default_value_t = 10100)]
        port: u16,
    },
    /// Print all available provider/model identifiers.
    Models,
    /// Validate configuration discovery and provider loading.
    Doctor,
    /// Add discovered models to Codex while retaining built-in OpenAI models.
    CodexInstall {
        /// URL where Codex can reach the local Responses API.
        #[arg(long, default_value = "http://127.0.0.1:10100/v1")]
        base_url: String,
    },
    /// Configure Zed with discovered models and start its local proxy.
    Zed {
        /// URL where Zed can reach the local OpenAI-compatible API.
        #[arg(long, default_value = "http://127.0.0.1:10100/v1")]
        base_url: String,
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        #[arg(long, default_value_t = 10100)]
        port: u16,
    },
    /// Upgrade JustOpenCode from a checksummed GitHub release.
    Upgrade {
        /// Install a specific version instead of the latest release.
        #[arg(long)]
        version: Option<String>,
    },
}
