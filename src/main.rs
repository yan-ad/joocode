mod app;
mod autostart;
mod cli;
mod codex;
mod config;
mod dashboard;
mod desktop;
mod error;
mod local_config;
#[cfg(target_os = "macos")]
mod macos_keychain;
mod protocol;
mod provider;
mod sources;
mod upgrade;
mod zed;

use anyhow::Context;
use clap::Parser;
use cli::{Cli, Command};
use desktop::DesktopTargets;
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

    let selection = SourceSelection::new(cli.sources, cli.config, cli.auth)?;
    let registry = Registry::discover(&selection)
        .await
        .context("failed to discover model providers")?;

    if registry.models().is_empty()
        && cli.command.is_some()
        && !matches!(cli.command, Some(Command::Doctor))
    {
        anyhow::bail!("no compatible models found; run `joocode doctor` for source diagnostics");
    }

    if cli.all {
        return app::serve_dashboard(
            cli.host,
            cli.port,
            registry,
            selection,
            DesktopTargets::all_supported(),
            cli.base_url,
        )
        .await;
    }

    let Some(command) = cli.command else {
        return app::serve_dashboard(
            cli.host,
            cli.port,
            registry,
            selection,
            DesktopTargets::detect(),
            cli.base_url,
        )
        .await;
    };

    match command {
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
        Command::Upgrade { .. } => unreachable!("upgrade is handled before config discovery"),
    }
}
