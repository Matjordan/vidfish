//! Headless session management and post-processing helpers.
use clap::{Args, Subcommand};
use std::path::PathBuf;
#[derive(Args)]
pub struct SessionArgs {
    #[arg(long)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    action: Action,
}
#[derive(Subcommand)]
enum Action {
    List,
    Rename {
        #[arg(long)]
        name: String,
    },
    New {
        #[arg(long, default_value = "")]
        name: String,
    },
    Browse {
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        camera: Option<String>,
        #[arg(long, default_value = "")]
        from: String,
        #[arg(long, default_value = "")]
        to: String,
        #[arg(long, default_value = "")]
        event: String,
    },
    Export {
        event: String,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        destination: PathBuf,
    },
    Storage,
    /// Apply the saved opt-in retention policy; disabled policies delete nothing.
    Retention,
}
pub fn run(
    args: SessionArgs,
    global_config: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = match args.config.or(global_config) {
        Some(path) => path,
        None => directories::ProjectDirs::from("org", "vidfish", "vidfish")
            .ok_or("Cannot determine settings directory; pass --config")?
            .config_dir()
            .join("config.json"),
    };
    let config = vidfish_core::config::AppConfig::load(&path)?;
    match args.action {
        Action::Rename { name } => println!(
            "{}",
            serde_json::to_string_pretty(&vidfish::sessions::rename(&path, &name)?)?
        ),
        Action::List => println!(
            "{}",
            serde_json::to_string_pretty(&vidfish::sessions::list(&path)?)?
        ),
        Action::New { name } => println!(
            "{}",
            serde_json::to_string_pretty(&vidfish::sessions::start_new(&path, &config, &name)?)?
        ),
        Action::Browse {
            session,
            camera,
            from,
            to,
            event,
        } => println!(
            "{}",
            serde_json::to_string_pretty(&vidfish::library::load(
                &path,
                &vidfish::library::Filter {
                    session,
                    camera,
                    from,
                    to,
                    event
                }
            )?)?
        ),
        Action::Export {
            event,
            session,
            destination,
        } => {
            uuid::Uuid::parse_str(&event)?;
            let directory = match session {
                Some(id) if id == "legacy" => path.with_file_name("operator-events"),
                Some(id) => vidfish::sessions::registry(&path)?
                    .sessions
                    .into_iter()
                    .find(|s| s.id == id)
                    .ok_or("Session ID not found")?
                    .directory
                    .join("events"),
                None => vidfish::sessions::current(&path)?
                    .map(|s| s.directory.join("events"))
                    .unwrap_or_else(|| path.with_file_name("operator-events")),
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&vidfish::event_export::export(
                    &directory.join(event),
                    &destination,
                    config.disk_warning_bytes
                )?)?
            );
        }
        Action::Storage => println!(
            "{}",
            serde_json::to_string_pretty(&vidfish::library::storage(&path, &config)?)?
        ),
        Action::Retention => println!(
            "{}",
            serde_json::to_string_pretty(&vidfish::retention::apply(&path, &config)?)?
        ),
    }
    Ok(())
}
