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
fn sixteen_recorders_survive_supervisor_restart() {
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
    for number in 2..=16 {
        store
            .add_manual(&format!("Healthy {number}"), &rtsp.uri)
            .unwrap();
    }
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
            == 16
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

#[test]
fn stalled_recording_logs_gap_recovers_and_does_not_interrupt_peer_preview() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.json");
    let config = AppConfig {
        output_directory: directory.path().join("media"),
        segment_seconds: 2,
        stall_seconds: 3,
        ..Default::default()
    };
    config.save(&config_path).unwrap();
    let troubled = RtspFixture::start(320, 240).unwrap();
    let healthy = RtspFixture::start(320, 240).unwrap();
    let mut store = CameraStore::default();
    let bad_id = store.add_manual("Stalling", &troubled.uri).unwrap();
    let good_id = store.add_manual("Healthy", &healthy.uri).unwrap();
    store.save(&directory.path().join("cameras.json")).unwrap();
    let manager =
        Manager::start(Options::new(directory.path().join("cameras.json"), None)).unwrap();
    let supervisor = Supervisor::start_with_executable(
        config_path,
        config,
        manager.snapshot(),
        env!("CARGO_BIN_EXE_vidfish").into(),
    )
    .unwrap();
    let mut cleanup = Cleanup {
        manager: Some(manager),
        supervisor: Some(supervisor),
    };
    let supervisor = cleanup.supervisor.as_ref().unwrap();
    wait(supervisor, |s| {
        s.cameras.len() == 2
            && s.cameras
                .iter()
                .all(|c| c.state == State::Recording && c.bytes_written > 0)
    });
    let statuses = supervisor.snapshot().borrow().cameras.clone();
    let bad_run = statuses
        .iter()
        .find(|c| c.camera_id == bad_id)
        .unwrap()
        .run_id
        .clone();
    let good = statuses
        .iter()
        .find(|c| c.camera_id == good_id)
        .unwrap()
        .clone();
    let preview = vidfish_media::Preview::start(
        vidfish_media::Source::Rtsp(healthy.uri.clone()),
        vidfish_media::Codec::H264,
    );
    troubled.set_paused(true);
    // Durable failure history survives a fast retry transition.
    let events_path = directory
        .path()
        .join("acquisition")
        .join(format!("{bad_id}.events.jsonl"));
    let deadline = Instant::now() + Duration::from_secs(30);
    while !std::fs::read_to_string(&events_path)
        .unwrap_or_default()
        .contains("gap_started")
    {
        assert!(Instant::now() < deadline, "silent stall was not detected");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        preview.frames.borrow().is_some(),
        "healthy preview did not survive peer failure"
    );
    wait(supervisor, |s| {
        s.cameras.iter().any(|c| {
            c.camera_id == good_id
                && c.run_id == good.run_id
                && c.bytes_written > good.bytes_written
        })
    });
    troubled.set_paused(false);
    wait(supervisor, |s| {
        s.cameras.iter().any(|c| {
            c.camera_id == bad_id
                && c.run_id != bad_run
                && c.state == State::Recording
                && c.bytes_written > 0
        })
    });
    preview.request_stop();
    preview.join();
    let switched = vidfish_media::Preview::start(
        vidfish_media::Source::Rtsp(troubled.uri.clone()),
        vidfish_media::Codec::H264,
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while switched.frames.borrow().is_none() {
        assert!(Instant::now() < deadline, "switched preview failed");
        std::thread::sleep(Duration::from_millis(50));
    }
    switched.request_stop();
    switched.join();
    wait(supervisor, |s| {
        s.cameras.iter().any(|c| {
            c.camera_id == good_id
                && c.run_id == good.run_id
                && c.bytes_written > good.bytes_written
        })
    });
    cleanup.supervisor.take().unwrap().finish_all().unwrap();
    let events = std::fs::read_to_string(events_path).unwrap();
    assert!(events.matches("recording_started").count() >= 2);
    for entry in std::fs::read_dir(directory.path().join("media")).unwrap() {
        let index = entry.unwrap().path().join("segments.jsonl");
        for line in std::fs::read_to_string(index).unwrap().lines() {
            let event: vidfish_media::segmented::FragmentEvent =
                serde_json::from_str(line).unwrap();
            if event.event == "closed" {
                vidfish_media::inspect_recording(&event.location).unwrap();
            }
        }
    }
}

#[test]
fn disk_stop_prevents_writes_and_warning_policy_allows_manual_recording() {
    use vidfish_core::config::DiskPolicy;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    let mut config = AppConfig {
        output_directory: directory.path().join("media"),
        auto_record: false,
        disk_warning_bytes: u64::MAX,
        disk_stop_bytes: u64::MAX,
        ..Default::default()
    };
    config.save(&path).unwrap();
    let rtsp = RtspFixture::start(320, 240).unwrap();
    let mut store = CameraStore::default();
    let id = store.add_manual("Disk policy", &rtsp.uri).unwrap();
    store.save(&directory.path().join("cameras.json")).unwrap();
    let manager =
        Manager::start(Options::new(directory.path().join("cameras.json"), None)).unwrap();
    let supervisor = Supervisor::start_with_executable(
        path.clone(),
        config.clone(),
        manager.snapshot(),
        env!("CARGO_BIN_EXE_vidfish").into(),
    )
    .unwrap();
    let mut cleanup = Cleanup {
        manager: Some(manager),
        supervisor: Some(supervisor),
    };
    let supervisor = cleanup.supervisor.as_ref().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        supervisor.snapshot().borrow().cameras.is_empty(),
        "disabled policy auto-started"
    );
    supervisor
        .controller()
        .command(Control::Record(id.clone()))
        .unwrap();
    wait(supervisor, |s| {
        s.cameras.iter().any(|s| {
            s.state == State::Failed
                && s.error
                    .as_ref()
                    .is_some_and(|e| e.contains("Disk capacity"))
        })
    });
    assert!(
        std::fs::read_dir(&config.output_directory)
            .unwrap()
            .next()
            .is_none(),
        "disk-stop policy wrote media"
    );
    let preview = vidfish_media::Preview::start(
        vidfish_media::Source::Rtsp(rtsp.uri.clone()),
        vidfish_media::Codec::H264,
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while preview.frames.borrow().is_none() {
        assert!(
            Instant::now() < deadline,
            "healthy preview unavailable during recording failure"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        supervisor
            .snapshot()
            .borrow()
            .cameras
            .iter()
            .any(|s| s.state == State::Failed),
        "preview incorrectly masked recording failure"
    );
    preview.request_stop();
    preview.join();
    config.disk_policy = DiskPolicy::Warn;
    config.save(&path).unwrap();
    supervisor
        .controller()
        .command(Control::Record(id))
        .unwrap();
    wait(supervisor, |s| {
        s.cameras
            .iter()
            .any(|s| s.state == State::Recording && s.bytes_written > 0 && s.warning.is_some())
    });
    cleanup.supervisor.take().unwrap().finish_all().unwrap();
}
