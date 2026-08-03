mod app;
mod cli;
mod codex;
mod config;
mod error;
mod jetbrains;
mod protocol;
mod provider;
mod upgrade;
mod zed;

use anyhow::Context;
use clap::Parser;
use cli::{Cli, Command};
use config::ConfigPaths;
use provider::Registry;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "joocode=info".into()),
        )
        .compact()
        .init();

    let cli = Cli::parse();
    if let Some(Command::Upgrade { version }) = &cli.command {
        return upgrade::run(version.as_deref()).await;
    }

    let paths = ConfigPaths::resolve(cli.config, cli.auth)?;
    let registry = Registry::load(&paths).context("failed to load OpenCode providers")?;

    match cli.command.unwrap_or(Command::Serve {
        host: "127.0.0.1".parse()?,
        port: 10100,
    }) {
        Command::Serve { host, port } => app::serve(host, port, registry).await,
        Command::Models => {
            for model in registry.models() {
                println!("{}\t{}", model.id, model.name);
            }
            Ok(())
        }
        Command::Doctor => {
            println!("config: {}", paths.config.display());
            println!("auth:   {}", paths.auth.display());
            println!("providers: {}", registry.provider_count());
            println!("models: {}", registry.models().len());
            Ok(())
        }
        Command::CodexInstall { base_url } => {
            let installed = codex::install(&registry, &base_url)?;
            println!("config:  {}", installed.config.display());
            println!("catalog: {}", installed.catalog.display());
            println!("added:   {} OpenCode models", installed.added_model_count);
            println!("total:   {} models", installed.total_model_count);
            println!("Restart Codex to reload the model picker.");
            Ok(())
        }
        Command::Zed {
            base_url,
            host,
            port,
        } => {
            let settings = zed::install(&registry, &base_url)?;
            println!("Zed settings: {}", settings.display());
            println!("Registered {} OpenCode models.", registry.models().len());
            println!("Restart Zed once if it is already running.");
            app::serve(host, port, registry).await
        }
        Command::Jetbrains {
            base_url,
            host,
            port,
        } => {
            println!("{}", jetbrains::setup_instructions(&registry, &base_url));
            app::serve(host, port, registry).await
        }
        Command::Upgrade { .. } => unreachable!("upgrade is handled before config discovery"),
    }
}
