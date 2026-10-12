use clap::Parser;
use std::path::PathBuf;
use vidfish_core::{config::AppConfig, engine::Engine};

#[cfg(feature = "desktop")]
mod desktop;

#[derive(Parser)]
#[command(version = option_env!("VIDFISH_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")), about = "Vidfish scientific camera acquisition")]
#[command(group(clap::ArgGroup::new("mode").args(["service", "record", "check", "media_check", "demo", "preview", "inspect"])))]
#[command(group(clap::ArgGroup::new("desktop_input").args(["demo", "preview"])))]
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
    /// Check required GStreamer runtime elements without opening cameras.
    #[arg(long)]
    media_check: bool,
    /// Record one RTSP stream in a process independent of the desktop.
    #[arg(long, requires = "output", conflicts_with_all = ["preview", "demo", "check", "media_check"])]
    record: Option<String>,
    /// New MKV file; existing files are never overwritten.
    #[arg(long, requires = "record")]
    output: Option<PathBuf>,
    /// Finish a recording after this many seconds instead of waiting for Ctrl+C.
    #[arg(long, requires = "record", value_parser = clap::value_parser!(u64).range(1..=86400))]
    seconds: Option<u64>,
    /// Show up to four RTSP previews. May be supplied multiple times.
    #[arg(long, conflicts_with_all = ["demo", "service", "check", "media_check"])]
    preview: Vec<String>,
    /// Show four simulated 1080p feeds (not a recording or hardware benchmark).
    #[arg(long, conflicts_with_all = ["service", "check", "media_check"])]
    demo: bool,
    /// Close the desktop after a bounded validation/measurement run.
    #[arg(long, requires = "desktop_input", value_parser = clap::value_parser!(u64).range(1..=3600), conflicts_with_all = ["service", "check", "media_check", "record"])]
    desktop_seconds: Option<u64>,
    /// Inspect a finalized recording's codec, dimensions, and media duration.
    #[arg(long)]
    inspect: Option<PathBuf>,
    /// Select H.265 instead of the default H.264 stream profile.
    #[arg(long)]
    h265: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    if let Some(path) = args.inspect {
        let info = vidfish_media::inspect_recording(&path)?;
        println!(
            "{}",
            serde_json::json!({"width": info.width, "height": info.height, "duration_seconds": info.duration_seconds, "caps": info.caps})
        );
        return Ok(());
    }
    if args.media_check {
        let plugins = vidfish_media::check_plugins_for(if args.h265 {
            vidfish_media::Codec::H265
        } else {
            vidfish_media::Codec::H264
        })?;
        println!("GStreamer runtime ready: {}", plugins.join(", "));
        return Ok(());
    }
    if args.preview.len() > 4 {
        return Err("at most four previews are supported".into());
    }
    let codec = if args.h265 {
        vidfish_media::Codec::H265
    } else {
        vidfish_media::Codec::H264
    };
    let path = match args.config {
        Some(path) => path,
        None => directories::ProjectDirs::from("org", "vidfish", "vidfish")
            .ok_or("cannot determine settings directory; pass --config")?
            .config_dir()
            .join("config.json"),
    };
    let config = AppConfig::load(&path)?;
    let runtime = tokio::runtime::Runtime::new()?;
    if let Some(uri) = args.record {
        let output = args.output.ok_or("recording needs --output")?;
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        let seconds = args.seconds;
        let mut worker = runtime.spawn_blocking(move || {
            let recording =
                vidfish_media::Recording::start(&vidfish_media::Source::Rtsp(uri), codec, &output)?;
            recording.wait_started(std::time::Duration::from_secs(15))?;
            tracing::info!("independent recording started");
            let started = std::time::Instant::now();
            while stop_rx.try_recv() == Err(std::sync::mpsc::TryRecvError::Empty)
                && seconds
                    .is_none_or(|limit| started.elapsed() < std::time::Duration::from_secs(limit))
            {
                recording.poll_error(std::time::Duration::from_millis(100))?;
            }
            recording.finish()?;
            tracing::info!("recording finalized");
            Ok::<_, vidfish_media::MediaError>(())
        });
        runtime.block_on(async {
            tokio::select! {
                result = &mut worker => { result??; }
                result = tokio::signal::ctrl_c() => { result?; let _ = stop_tx.send(()); worker.await??; }
            }
            Ok::<_, Box<dyn std::error::Error>>(())
        })?;
        // Joining the blocking worker also completes EOS/file finalization.
        runtime.shutdown_timeout(std::time::Duration::from_secs(15));
        return Ok(());
    }
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
    {
        let sources = if args.demo {
            (0..4)
                .map(|_| vidfish_media::Source::TestPattern {
                    width: 1920,
                    height: 1080,
                })
                .collect()
        } else {
            args.preview
                .into_iter()
                .map(vidfish_media::Source::Rtsp)
                .collect()
        };
        desktop::run(path, config, runtime, sources, codec, args.desktop_seconds)?;
    }
    #[cfg(not(feature = "desktop"))]
    return Err("desktop feature disabled; use --service or --check".into());
    #[cfg(feature = "desktop")]
    Ok(())
}
