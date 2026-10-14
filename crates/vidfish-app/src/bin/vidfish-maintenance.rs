#[path = "../installation.rs"]
#[allow(dead_code)]
mod installation;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
#[derive(Parser)]
#[command(version = installation::VERSION)]
struct Args {
    #[command(subcommand)]
    command: Operation,
}
#[derive(Subcommand)]
enum Operation {
    Install {
        #[arg(long)]
        prefix: PathBuf,
        #[arg(long)]
        bundle: PathBuf,
    },
    Uninstall {
        #[arg(long)]
        prefix: PathBuf,
    },
    Recover {
        #[arg(long)]
        root: PathBuf,
    },
    Apply {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        bundle: PathBuf,
        #[arg(long)]
        relaunch: bool,
        #[arg(long)]
        opengl: bool,
        #[arg(long)]
        config: Option<PathBuf>,
    },
}
fn main() -> installation::Result<()> {
    match Args::parse().command {
        Operation::Install { prefix, bundle } => installation::install(&prefix, &bundle),
        Operation::Uninstall { prefix } => installation::uninstall(&prefix),
        Operation::Recover { root } => installation::recover(&root),
        Operation::Apply {
            root,
            bundle,
            relaunch,
            opengl,
            config,
        } => installation::apply_with_renderer(&root, &bundle, relaunch, config.as_deref(), opengl),
    }
}
