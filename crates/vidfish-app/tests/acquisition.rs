use std::{
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    path::PathBuf,
    time::{Duration, Instant},
};
use vidfish::{
    acquisition::{Control, State, Supervisor},
    camera_preview,
};
use vidfish_core::{cameras::CameraStore, config::AppConfig};
use vidfish_media::fixtures::{OnvifFixture, RtspFixture};
use vidfish_onvif::manager::{Manager, Options};

fn wait(supervisor: &Supervisor, predicate: impl Fn(&vidfish::acquisition::Snapshot) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !predicate(&supervisor.snapshot().borrow()) {
        assert!(
            Instant::now() < deadline,
            "recording state timeout: {}",
            serde_json::to_string(&supervisor.snapshot().borrow().cameras).unwrap()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
struct Cleanup {
    manager: Option<Manager>,
    supervisor: Option<Supervisor>,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.finish_all();
        }
        if let Some(manager) = self.manager.take() {
            manager.join();
        }
    }
}
#[test]
fn camera_arrival_auto_records_and_stop_all_finalizes_without_restarting() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.json");
    let config = AppConfig {
        output_directory: directory.path().join("media"),
        ..Default::default()
    };
    config.save(&config_path).unwrap();
    let rtsp = RtspFixture::start(320, 240).unwrap();
    let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let target = udp.local_addr().unwrap();
    drop(udp);
    let SocketAddr::V4(destination) = target else {
        panic!()
    };
    let mut options = Options::new(
        directory.path().join("cameras.json"),
        Some(Ipv4Addr::LOCALHOST),
    );
    options.target = destination;
    options.discovery_interval = Duration::from_millis(200);
    let manager = Manager::start(options).unwrap();
    let supervisor = Supervisor::start_with_executable(
        config_path.clone(),
        config,
        manager.snapshot(),
        PathBuf::from(env!("CARGO_BIN_EXE_vidfish")),
    )
    .unwrap();
    let mut cleanup = Cleanup {
        manager: Some(manager),
        supervisor: Some(supervisor),
    };
    assert!(
        cleanup
            .supervisor
            .as_ref()
            .unwrap()
            .snapshot()
            .borrow()
            .cameras
            .is_empty()
    );
    let _camera =
        OnvifFixture::start_at(rtsp.uri.clone(), 71, "127.0.0.1:0".parse().unwrap(), target)
            .unwrap();
    let supervisor = cleanup.supervisor.as_ref().unwrap();
    wait(supervisor, |s| {
        s.cameras.len() == 1
            && s.cameras[0].state == State::Recording
            && s.cameras[0].bytes_written > 0
    });
    let run_id = supervisor.snapshot().borrow().cameras[0].run_id.clone();
    supervisor
        .controller()
        .command(Control::AutoRecord(false))
        .unwrap();
    let bytes = supervisor.snapshot().borrow().cameras[0].bytes_written;
    wait(supervisor, |s| s.cameras[0].bytes_written > bytes);
    assert_eq!(supervisor.snapshot().borrow().cameras[0].run_id, run_id);
    supervisor.controller().command(Control::StopAll).unwrap();
    wait(supervisor, |s| {
        s.stop_all_ack && s.cameras[0].state == State::Stopped
    });
    let output = supervisor.snapshot().borrow().cameras[0]
        .output
        .clone()
        .unwrap();
    let media = vidfish_media::inspect_recording(&output).unwrap();
    assert_eq!((media.width, media.height), (320, 240));
    supervisor
        .controller()
        .command(Control::AutoRecord(true))
        .unwrap();
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(supervisor.snapshot().borrow().cameras[0].run_id, run_id);
    cleanup.supervisor.take().unwrap().finish_all().unwrap();
}
#[test]
fn recording_survives_supervisor_restart_and_failed_peer() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.json");
    let config = AppConfig {
        output_directory: directory.path().join("media"),
        ..Default::default()
    };
    config.save(&config_path).unwrap();
    let rtsp = RtspFixture::start(320, 240).unwrap();
    let mut store = CameraStore::default();
    let healthy = store.add_manual("Healthy", &rtsp.uri).unwrap();
    for number in 2..=15 {
        store
            .add_manual(&format!("Healthy {number}"), &rtsp.uri)
            .unwrap();
    }
    store
        .add_manual("Refused", "rtsp://127.0.0.1:1/live")
        .unwrap();
    store.save(&directory.path().join("cameras.json")).unwrap();
    let manager =
        Manager::start(Options::new(directory.path().join("cameras.json"), None)).unwrap();
    let supervisor = Supervisor::start_with_executable(
        config_path.clone(),
        config.clone(),
        manager.snapshot(),
        PathBuf::from(env!("CARGO_BIN_EXE_vidfish")),
    )
    .unwrap();
    let mut cleanup = Cleanup {
        manager: Some(manager),
        supervisor: Some(supervisor),
    };
    let supervisor = cleanup.supervisor.as_ref().unwrap();
    wait(supervisor, |s| {
        s.cameras
            .iter()
            .filter(|c| c.state == State::Recording && c.bytes_written > 0)
            .count()
            == 15
            && s.cameras
                .iter()
                .any(|c| c.camera_id != healthy && c.state == State::Failed)
    });
    let run_id = supervisor
        .snapshot()
        .borrow()
        .cameras
        .iter()
        .find(|c| c.camera_id == healthy)
        .unwrap()
        .run_id
        .clone();
    cleanup.supervisor.take().unwrap().join(); // Desktop crash/close must not stop recording children.
    let restarted = Supervisor::start_with_executable(
        config_path,
        config,
        cleanup.manager.as_ref().unwrap().snapshot(),
        PathBuf::from(env!("CARGO_BIN_EXE_vidfish")),
    )
    .unwrap();
    cleanup.supervisor = Some(restarted);
    let supervisor = cleanup.supervisor.as_ref().unwrap();
    wait(supervisor, |s| {
        s.cameras
            .iter()
            .any(|c| c.camera_id == healthy && c.state == State::Recording && c.run_id == run_id)
    });
    let bytes = supervisor
        .snapshot()
        .borrow()
        .cameras
        .iter()
        .find(|c| c.camera_id == healthy)
        .unwrap()
        .bytes_written;
    wait(supervisor, |s| {
        s.cameras
            .iter()
            .any(|c| c.camera_id == healthy && c.bytes_written > bytes)
    });
    cleanup.supervisor.take().unwrap().finish_all().unwrap();
}
#[test]
fn manual_h265_camera_selects_h265_without_onvif() {
    let mut store = CameraStore::default();
    store.add_manual("H265", "rtsp://127.0.0.1/live").unwrap();
    store.cameras[0].manual_h265 = true;
    let (_, codec) =
        camera_preview::source(&store.cameras[0], None, std::path::Path::new("unused.json"))
            .unwrap();
    assert_eq!(codec, vidfish_media::Codec::H265);
}
