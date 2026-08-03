use std::{net::IpAddr, path::PathBuf};

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "joc", version, about)]
pub struct Cli {
    /// Override the OpenCode configuration path.
    #[arg(long, env = "JOC_CONFIG")]
    pub config: Option<PathBuf>,

    /// Override the OpenCode authentication path.
    #[arg(long, env = "JOC_AUTH")]
    pub auth: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
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
    /// Print all available provider/model identifiers.
    Models,
    /// Validate configuration discovery and provider loading.
    Doctor,
    /// Add discovered OpenCode models to Codex while retaining built-in OpenAI models.
    CodexInstall {
        /// URL where Codex can reach the local Responses API.
        #[arg(long, default_value = "http://127.0.0.1:10100/v1")]
        base_url: String,
    },
    /// Configure Zed with discovered OpenCode models and start its local proxy.
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
