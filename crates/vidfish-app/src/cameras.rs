use clap::{Args, Subcommand};
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    path::PathBuf,
    time::Duration,
};
use vidfish_core::cameras::{CameraStore, HardwareIdentity};

#[derive(Subcommand)]
pub enum Command {
    /// Manage persistent camera identities independently of preview/recording.
    Cameras(InventoryArgs),
    /// Browse acquisition sessions and export scientific events.
    Sessions(crate::sessions_cli::SessionArgs),
}
#[derive(Args)]
pub struct InventoryArgs {
    /// Override the camera inventory file.
    #[arg(long)]
    inventory: Option<PathBuf>,
    #[command(subcommand)]
    action: Action,
}
#[derive(Subcommand)]
enum Action {
    List,
    Watch {
        #[arg(long)]
        interface: Option<Ipv4Addr>,
        #[arg(long, default_value = "239.255.255.250:3702")]
        target: SocketAddrV4,
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=3600))]
        seconds: Option<u64>,
    },
    AddOnvif {
        #[arg(long)]
        alias: String,
        #[arg(long)]
        device: String,
    },
    SetCredentials {
        id: String,
        #[arg(long)]
        username_env: String,
        #[arg(long)]
        password_env: String,
    },
    ForgetCredentials {
        id: String,
    },
    Resolve {
        id: String,
    },
    Add {
        #[arg(long)]
        h265: bool,
        #[arg(long)]
        alias: String,
        #[arg(long)]
        rtsp: String,
    },
    Rename {
        id: String,
        alias: String,
    },
    Remove {
        id: String,
    },
    Scan {
        #[arg(long)]
        interface: Ipv4Addr,
        #[arg(long, default_value = "239.255.255.250:3702")]
        target: SocketAddrV4,
    },
}
pub fn run(
    command: Command,
    global_config: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let args = match command {
        Command::Cameras(args) => args,
        Command::Sessions(args) => return crate::sessions_cli::run(args, global_config),
    };
    let path = match args.inventory {
        Some(path) => path,
        None => directories::ProjectDirs::from("org", "vidfish", "vidfish")
            .ok_or("cannot determine camera settings directory; pass --inventory")?
            .config_dir()
            .join("cameras.json"),
    };
    let _lock = if matches!(
        &args.action,
        Action::List | Action::Resolve { .. } | Action::Watch { .. }
    ) {
        None
    } else {
        Some(CameraStore::lock(&path)?)
    };
    let credentials_path = vidfish_onvif::credentials::file_path(&path);
    let mut store = CameraStore::load(&path)?;
    match args.action {
        Action::Watch {
            interface,
            target,
            seconds,
        } => {
            let mut options = vidfish_onvif::manager::Options::new(path, interface);
            options.target = target;
            let manager = vidfish_onvif::manager::Manager::start(options)?;
            let mut snapshot = manager.snapshot();
            let runtime = tokio::runtime::Runtime::new()?;
            let result = runtime.block_on(async {
                let finish = async {
                    match seconds {
                        Some(seconds) => tokio::time::sleep(Duration::from_secs(seconds)).await,
                        None => std::future::pending::<()>().await,
                    }
                };
                tokio::pin!(finish);
                loop {
                    tokio::select! {
                        result = tokio::signal::ctrl_c() => { result?; break; }
                        _ = &mut finish => break,
                        changed = snapshot.changed() => {
                            if changed.is_err() { break; }
                            println!("{}", serde_json::to_string(&*snapshot.borrow_and_update())?);
                        }
                    }
                }
                Ok::<(), Box<dyn std::error::Error>>(())
            });
            manager.join();
            result?;
        }
        Action::List => println!("{}", serde_json::to_string_pretty(&store)?),
        Action::AddOnvif { alias, device } => {
            let id = store.add_onvif(&alias, &device)?;
            store.save(&path)?;
            println!("{id}");
        }
        Action::SetCredentials {
            id,
            username_env,
            password_env,
        } => {
            let camera = store
                .cameras
                .iter_mut()
                .find(|c| c.id == id)
                .ok_or("unknown camera ID")?;
            let credentials = vidfish_onvif::credentials::Credentials::from_environment(
                &username_env,
                &password_env,
            )?;
            vidfish_onvif::credentials::save(&credentials_path, &id, &credentials)?;
            camera.credential_key = Some(id);
            store.save(&path)?;
        }
        Action::ForgetCredentials { id } => {
            let camera = store
                .cameras
                .iter_mut()
                .find(|c| c.id == id)
                .ok_or("unknown camera ID")?;
            vidfish_onvif::credentials::remove(&credentials_path, &id)?;
            camera.credential_key = None;
            store.save(&path)?;
        }
        Action::Resolve { id } => {
            let camera = store
                .cameras
                .iter()
                .find(|c| c.id == id)
                .ok_or("unknown camera ID")?;
            let address = camera
                .device_url
                .as_deref()
                .ok_or("manual RTSP cameras do not have an ONVIF service")?;
            let client = client_for(camera, &credentials_path)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&client.resolve(address)?)?
            );
        }
        Action::Add { alias, rtsp, h265 } => {
            let id = store.add_manual(&alias, &rtsp)?;
            store.cameras.last_mut().unwrap().manual_h265 = h265;
            store.save(&path)?;
            println!("{id}");
        }
        Action::Rename { id, alias } => {
            store.rename(&id, &alias)?;
            store.save(&path)?;
        }
        Action::Remove { id } => {
            let index = store
                .cameras
                .iter()
                .position(|c| c.id == id)
                .ok_or("unknown camera ID")?;
            if store.cameras[index].credential_key.is_some() {
                vidfish_onvif::credentials::remove(&credentials_path, &id)?;
            }
            store.cameras.remove(index);
            store.save(&path)?;
        }
        Action::Scan { interface, target } => {
            let found = vidfish_onvif::discover(interface, target, Duration::from_secs(2))?;
            // Duplicate UUIDs at different addresses are quarantined before any mutation.
            let mut addresses =
                std::collections::BTreeMap::<String, std::collections::BTreeSet<String>>::new();
            for device in &found {
                addresses
                    .entry(device.endpoint_id.clone())
                    .or_default()
                    .insert(device.device_url.clone());
            }
            let mut results = Vec::new();
            for device in found {
                if addresses[&device.endpoint_id].len() > 1 {
                    results.push(serde_json::json!({"endpoint_id": device.endpoint_id, "error": "duplicate endpoint identity at multiple addresses; confirm camera identity"}));
                    continue;
                }
                let client = store
                    .cameras
                    .iter()
                    .find(|c| {
                        c.endpoint_id.as_deref() == Some(&device.endpoint_id)
                            || c.device_url.as_deref() == Some(&device.device_url)
                    })
                    .map(|camera| client_for(camera, &credentials_path))
                    .unwrap_or_else(vidfish_onvif::OnvifClient::new);
                let result = match client.and_then(|client| client.resolve(&device.device_url)) {
                    Ok(camera) => {
                        let hardware = (!camera.manufacturer.is_empty()
                            && !camera.serial_number.is_empty())
                        .then_some(HardwareIdentity {
                            manufacturer: camera.manufacturer,
                            serial_number: camera.serial_number,
                        });
                        match store.observe(&device.endpoint_id, &device.device_url, hardware) {
                            Ok(observation) => {
                                serde_json::json!({"endpoint_id": device.endpoint_id, "observation": format!("{observation:?}")})
                            }
                            Err(error) => {
                                serde_json::json!({"endpoint_id": device.endpoint_id, "error": error.to_string()})
                            }
                        }
                    }
                    Err(error) => {
                        serde_json::json!({"endpoint_id": device.endpoint_id, "error": error.to_string()})
                    }
                };
                results.push(result);
            }
            store.save(&path)?;
            println!("{}", serde_json::to_string_pretty(&results)?);
        }
    }
    Ok(())
}

fn client_for(
    camera: &vidfish_core::cameras::CameraRecord,
    credentials_path: &std::path::Path,
) -> Result<vidfish_onvif::OnvifClient, vidfish_onvif::Error> {
    match &camera.credential_key {
        Some(key) => vidfish_onvif::OnvifClient::with_credentials(
            vidfish_onvif::credentials::load(credentials_path, key)?,
        ),
        None => vidfish_onvif::OnvifClient::new(),
    }
}
