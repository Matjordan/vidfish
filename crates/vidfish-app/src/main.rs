use clap::Parser;
use std::path::PathBuf;
use vidfish_core::{config::AppConfig, engine::Engine};

#[cfg(feature = "desktop")]
mod desktop;

#[derive(Parser)]
#[command(version, about = "Vidfish scientific camera acquisition")]
struct Args {
    /// Override the platform-specific settings file.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Run the independent camera-manager service until Ctrl+C.
    #[arg(long, conflicts_with = "check")]
    service: bool,
    /// Validate settings and exercise startup and orderly shutdown without a display.
    #[arg(long)]
    check: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let path = match args.config {
        Some(path) => path,
        None => directories::ProjectDirs::from("org", "vidfish", "vidfish")
            .ok_or("cannot determine settings directory; pass --config")?
            .config_dir()
            .join("config.json"),
    };
    let config = AppConfig::load(&path)?;
    let runtime = tokio::runtime::Runtime::new()?;
    if args.service || args.check {
        runtime.block_on(async {
            let engine = Engine::start(config)?;
            if args.service {
                tokio::signal::ctrl_c().await?;
            }
            engine.shutdown().await?;
            Ok::<_, Box<dyn std::error::Error>>(())
        })?;
        return Ok(());
    }
    #[cfg(feature = "desktop")]
    desktop::run(path, config, runtime)?;
    #[cfg(not(feature = "desktop"))]
    return Err("desktop feature disabled; use --service or --check".into());
    #[cfg(feature = "desktop")]
    Ok(())
}
