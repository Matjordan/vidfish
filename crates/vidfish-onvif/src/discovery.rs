//! Re-enumerate adapters on each pass so mobile network changes need no restart.
use crate::{DiscoveredCamera, Error, Result, discover};
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddrV4},
    time::Duration,
};
#[derive(Default)]
pub struct Report {
    pub cameras: Vec<DiscoveredCamera>,
    pub warnings: Vec<String>,
}
pub fn adapters() -> Result<Vec<Ipv4Addr>> {
    let interfaces = if_addrs::get_if_addrs().map_err(|_| {
        Error("Cannot enumerate network adapters; select an explicit local IPv4 address")
    })?;
    let mut addresses: Vec<_> = interfaces
        .into_iter()
        .filter(|i| i.oper_status == if_addrs::IfOperStatus::Up && !i.is_loopback())
        .filter_map(|i| match i.ip() {
            std::net::IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_multicast() => Some(ip),
            _ => None,
        })
        .collect();
    addresses.sort();
    addresses.dedup();
    Ok(addresses)
}
pub fn selected(
    interface: Option<Ipv4Addr>,
    target: SocketAddrV4,
    window: Duration,
) -> Result<Report> {
    if interface
        .is_some_and(|ip| ip.is_unspecified() || ip.is_multicast() || ip == Ipv4Addr::BROADCAST)
    {
        return Err(Error(
            "Adapter override must be a local unicast IPv4 address; leave blank to search all active adapters",
        ));
    }
    let addresses = match interface {
        Some(ip) => vec![ip],
        None => adapters()?,
    };
    if addresses.is_empty() {
        return Err(Error(
            "No active IPv4 network adapter; connect Ethernet/Wi-Fi or select a local IPv4 address",
        ));
    }
    probe(&addresses, target, window)
}
/// One bound socket and multicast adapter per address. Concurrent collection windows
/// avoid multiplying the scan duration by the number of adapters.
pub fn probe(addresses: &[Ipv4Addr], target: SocketAddrV4, window: Duration) -> Result<Report> {
    if addresses.len() > 64 {
        return Err(Error(
            "More than 64 discovery addresses; select an explicit local IPv4 address",
        ));
    }
    let mut addresses = addresses.to_vec();
    addresses.sort();
    addresses.dedup();
    let results = std::thread::scope(|scope| {
        let jobs: Vec<_> = addresses
            .into_iter()
            .map(|ip| {
                (
                    ip,
                    scope.spawn(move || {
                        tracing::info!(interface=%ip,"probing camera network adapter");
                        discover(ip, target, window)
                    }),
                )
            })
            .collect();
        jobs.into_iter()
            .map(|(ip, job)| {
                (
                    ip,
                    job.join()
                        .unwrap_or(Err(Error("Discovery adapter worker stopped"))),
                )
            })
            .collect::<Vec<_>>()
    });
    let mut cameras = BTreeMap::new();
    let mut report = Report::default();
    for (ip, result) in results {
        match result {
            Ok(devices) => {
                for device in devices {
                    cameras
                        .entry((device.endpoint_id.clone(), device.device_url.clone()))
                        .or_insert(device);
                }
            }
            Err(error) => report.warnings.push(format!("Adapter {ip}: {error}")),
        }
    }
    if cameras.len() > 16 {
        report
            .warnings
            .push("More than 16 cameras discovered; only the first 16 are supported".into());
    }
    report.cameras = cameras.into_values().take(16).collect();
    Ok(report)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multiple_adapter_replies_deduplicate_and_a_bad_adapter_does_not_hide_peers() {
        let fixture =
            vidfish_media::fixtures::OnvifFixture::start("rtsp://127.0.0.1/live".into()).unwrap();
        let std::net::SocketAddr::V4(target) = fixture.discovery_address else {
            panic!("IPv4 fixture required")
        };
        let report = probe(
            &[
                Ipv4Addr::LOCALHOST,
                Ipv4Addr::new(127, 0, 0, 2),
                Ipv4Addr::new(192, 0, 2, 123),
            ],
            target,
            Duration::from_millis(150),
        )
        .unwrap();
        assert_eq!(report.cameras.len(), 1);
        assert_eq!(report.cameras[0].device_url, fixture.device_url);
        assert_eq!(report.warnings.len(), 1);
    }
}
