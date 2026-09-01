mod app;
mod cli;
mod codex;
mod config;
mod error;
mod jetbrains;
mod protocol;
mod provider;
mod sources;
mod upgrade;
mod zed;

use anyhow::Context;
use clap::Parser;
use cli::{Cli, Command};
use provider::Registry;
use sources::SourceSelection;
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
    if cli.all && cli.command.is_some() {
        anyhow::bail!("--all cannot be combined with a subcommand");
    }
    if let Some(Command::Upgrade { version }) = &cli.command {
        return upgrade::run(version.as_deref()).await;
    }

    fn configure_all_desktop_clients(registry: &Registry, base_url: &str) -> anyhow::Result<()> {
        let codex = codex::install(registry, base_url)?;
        let zed_settings = zed::install(registry, base_url)?;

        println!("Configured all supported desktop integrations:");
        println!("  Codex: {}", codex.config.display());
        println!("  Zed:   {}", zed_settings.display());
        println!("  JetBrains: setup values below (stored by the IDE credential store).");
        println!(
            "  Models: {} discovered provider/model entries",
            registry.models().len()
        );
        println!();
        println!("{}", jetbrains::setup_instructions(registry, base_url));
        println!("Restart Codex and Zed if they are already running.");
        Ok(())
    }

    let selection = SourceSelection::new(cli.sources, cli.config, cli.auth)?;
    let registry = Registry::discover(&selection)
        .await
        .context("failed to discover model providers")?;

    if registry.models().is_empty() && !matches!(cli.command, Some(Command::Doctor)) {
        anyhow::bail!("no compatible models found; run `joocode doctor` for source diagnostics");
    }

    if cli.all {
        configure_all_desktop_clients(&registry, &cli.base_url)?;
        return app::serve(cli.host, cli.port, registry).await;
    }

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
            for report in registry.source_reports() {
                println!(
                    "{:<10} {:<8} providers={} models={}{}",
                    report.source,
                    report.status,
                    report.providers,
                    report.models,
                    report
                        .detail
                        .as_deref()
                        .map(|detail| format!(" ({detail})"))
                        .unwrap_or_default()
                );
            }
            println!("providers: {}", registry.provider_count());
            println!("models: {}", registry.models().len());
            Ok(())
        }
        Command::CodexInstall { base_url } => {
            let installed = codex::install(&registry, &base_url)?;
            println!("config:  {}", installed.config.display());
            println!("catalog: {}", installed.catalog.display());
            println!("added:   {} discovered models", installed.added_model_count);
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
            println!("Registered {} discovered models.", registry.models().len());
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
