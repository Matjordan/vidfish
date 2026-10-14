use clap::Parser;
use std::path::PathBuf;
use vidfish::acquisition;
use vidfish_core::{config::AppConfig, engine::Engine};
mod cameras;
mod sessions_cli;

#[cfg(feature = "desktop")]
mod desktop;

#[derive(Parser)]
#[command(version = option_env!("VIDFISH_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")), about = "Vidfish scientific camera acquisition")]
#[command(group(clap::ArgGroup::new("mode").args(["service", "record", "check", "media_check", "demo", "preview", "inspect", "discover", "onvif", "record_camera"])))]
#[command(group(clap::ArgGroup::new("desktop_input").args(["demo", "preview"])))]
struct Args {
    /// Disable startup release checks for offline rigs (manual checks remain available).
    #[arg(long)]
    no_update_check: bool,
    #[arg(long, hide = true)]
    update_startup_health: Option<PathBuf>,
    /// Record a saved camera independently, using its saved login and recording policy.
    #[arg(long)]
    record_camera: Option<String>,
    #[arg(long, requires = "record_camera", hide = true)]
    record_run_id: Option<String>,
    #[command(subcommand)]
    command: Option<cameras::Command>,
    /// Probe ONVIF cameras on the selected IPv4 adapter and resolve their profiles.
    #[arg(long)]
    discover: bool,
    /// Resolve a manually supplied ONVIF device service URL without discovery.
    #[arg(long)]
    onvif: Option<String>,
    /// Optional local IPv4 adapter override; blank probes all active IPv4 adapters.
    #[arg(long, requires = "discover")]
    interface: Option<std::net::Ipv4Addr>,
    /// Discovery destination; override only for a known unicast camera or fixture.
    #[arg(long, requires = "discover", default_value = "239.255.255.250:3702")]
    discovery_target: std::net::SocketAddrV4,
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
    /// Use the OpenGL renderer instead of Windows' default Direct3D-capable wgpu renderer.
    #[arg(long, conflicts_with_all = ["service", "check", "media_check", "record", "inspect"])]
    opengl: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let _process_lease = if !(std::env::var_os("VIDFISH_INSTALL_HEALTH").is_some()
        && (args.check || args.media_check))
    {
        vidfish::installation::process_lease().map_err(|e| e as Box<dyn std::error::Error>)?
    } else {
        None
    };
    if let Some(probe) = &args.update_startup_health {
        let root = vidfish::installation::installed_root()
            .ok_or("Startup confirmation requires a managed install")?;
        let canonical_root = root.canonicalize()?;
        if probe
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .as_deref()
            != Some(canonical_root.as_path())
            || probe.is_symlink()
            || !probe
                .file_name()
                .is_some_and(|p| p.to_string_lossy().starts_with(".tmp"))
            || std::fs::metadata(probe)?.len() != 0
            || !root.join("transaction.json").exists()
        {
            return Err("Invalid update startup confirmation file".into());
        }
    }
    if let Some(command) = args.command {
        if args.discover
            || args.onvif.is_some()
            || args.service
            || args.record.is_some()
            || args.record_camera.is_some()
            || args.check
            || args.media_check
            || args.demo
            || !args.preview.is_empty()
            || args.inspect.is_some()
        {
            return Err("camera commands cannot be combined with another application mode".into());
        }
        return cameras::run(command, args.config);
    }
    if args.discover || args.onvif.is_some() {
        let client = vidfish_onvif::OnvifClient::new()?;
        if let Some(address) = args.onvif {
            println!(
                "{}",
                serde_json::to_string_pretty(&client.resolve(&address)?)?
            );
        } else {
            let report = vidfish_onvif::discovery::selected(
                args.interface,
                args.discovery_target,
                std::time::Duration::from_secs(2),
            )?;
            for warning in report.warnings {
                tracing::warn!("{warning}");
            }
            let devices = report.cameras;
            let results: Vec<_> = devices
                .into_iter()
                .map(|device| match client.resolve(&device.device_url) {
                    Ok(camera) => serde_json::json!({"device": device, "camera": camera}),
                    Err(error) => serde_json::json!({"device": device, "error": error.to_string()}),
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&results)?);
        }
        return Ok(());
    }
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
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(if args.record_camera.is_some() { 1 } else { 2 })
        .enable_all()
        .build()?;
    if let Some(id) = args.record_camera {
        let run_id = args
            .record_run_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let interrupted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopping = interrupted.clone();
        runtime.spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                stopping.store(true, std::sync::atomic::Ordering::Release);
            }
        });
        acquisition::record_camera(&path, &config, &id, &run_id, &interrupted)?;
        return Ok(());
    }
    if let Some(uri) = args.record {
        let _recording_lease = vidfish::installation::recording_lease()
            .map_err(|e| e as Box<dyn std::error::Error>)?;
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
        let cameras = if args.service {
            Some(vidfish_onvif::manager::Manager::start(
                vidfish_onvif::manager::Options::new(
                    path.with_file_name("cameras.json"),
                    config.camera_interface,
                ),
            )?)
        } else {
            None
        };
        let recordings = cameras
            .as_ref()
            .map(|cameras| {
                acquisition::Supervisor::start(path.clone(), config.clone(), cameras.snapshot())
            })
            .transpose()?;
        let result = runtime.block_on(async {
            let engine = Engine::start(config)?;
            if args.service {
                tokio::signal::ctrl_c().await?;
            }
            engine.shutdown().await?;
            Ok::<_, Box<dyn std::error::Error>>(())
        });
        if let Some(cameras) = &cameras {
            cameras.stop();
        }
        if let Some(recordings) = recordings {
            recordings.finish_all()?;
        }
        if let Some(cameras) = cameras {
            cameras.join();
        }
        result?;
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
        desktop::run(
            path,
            config,
            runtime,
            sources,
            codec,
            args.desktop_seconds,
            desktop::LaunchOptions {
                force_opengl: args.opengl,
                no_update_check: args.no_update_check,
                startup_health: args.update_startup_health,
            },
        )?;
    }
    #[cfg(not(feature = "desktop"))]
    return Err("desktop feature disabled; use --service or --check".into());
    #[cfg(feature = "desktop")]
    Ok(())
}
