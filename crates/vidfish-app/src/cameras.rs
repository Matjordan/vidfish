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
    Add {
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
pub fn run(command: Command) -> Result<(), Box<dyn std::error::Error>> {
    let Command::Cameras(args) = command;
    let path = match args.inventory {
        Some(path) => path,
        None => directories::ProjectDirs::from("org", "vidfish", "vidfish")
            .ok_or("cannot determine camera settings directory; pass --inventory")?
            .config_dir()
            .join("cameras.json"),
    };
    let mut store = CameraStore::load(&path)?;
    match args.action {
        Action::List => println!("{}", serde_json::to_string_pretty(&store)?),
        Action::Add { alias, rtsp } => {
            let id = store.add_manual(&alias, &rtsp)?;
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
            store.cameras.remove(index);
            store.save(&path)?;
        }
        Action::Scan { interface, target } => {
            let found = vidfish_onvif::discover(interface, target, Duration::from_secs(2))?;
            let client = vidfish_onvif::OnvifClient::new()?;
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
                let result = match client.resolve(&device.device_url) {
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
