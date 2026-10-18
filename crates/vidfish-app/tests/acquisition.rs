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
        if Instant::now() >= deadline {
            for status in &supervisor.snapshot().borrow().cameras {
                if let Some(output) = &status.output
                    && let Some(root) = output
                        .parent()
                        .and_then(|p| p.parent())
                        .and_then(|p| p.parent())
                {
                    let path = root
                        .join("acquisition")
                        .join(format!("{}-{}.log", status.camera_id, status.run_id));
                    if let Ok(log) = std::fs::read_to_string(path) {
                        eprintln!(
                            "Recorder diagnostic: {}",
                            log.lines()
                                .rev()
                                .take(12)
                                .collect::<Vec<_>>()
                                .into_iter()
                                .rev()
                                .collect::<Vec<_>>()
                                .join("\n")
                        );
                    }
                }
            }
        }
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
    for _ in 0..3 {
        supervisor.controller().command(Control::RecordAll).unwrap();
        wait(supervisor, |s| {
            s.cameras[0].state == State::Recording && s.cameras[0].bytes_written > 0
        });
        supervisor.controller().command(Control::StopAll).unwrap();
        wait(supervisor, |s| {
            s.stop_all_ack && s.cameras[0].state == State::Stopped
        });
        vidfish_media::inspect_recording(sup_output(supervisor).as_path()).unwrap();
    }
    cleanup.supervisor.take().unwrap().finish_all().unwrap();
}
fn sup_output(supervisor: &Supervisor) -> PathBuf {
    supervisor.snapshot().borrow().cameras[0]
        .output
        .clone()
        .unwrap()
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
    // All sixteen hidden cameras can be grabbed while all sixteen recording
    // processes keep their identities and continue making progress.
    let initial = supervisor.snapshot().borrow().cameras.clone();
    let marker_ms = vidfish::operator_events::now();
    let inputs = store
        .cameras
        .iter()
        .map(|camera| vidfish::operator_events::CameraInput {
            input: vidfish::operator_events::Input {
                camera: camera.id.clone(),
                label: camera.alias.clone(),
                frame: None,
            },
            camera: Some(camera.clone()),
            profile: None,
            recording: vidfish::event_references::Reference::at(
                initial.iter().find(|s| s.camera_id == camera.id),
                marker_ms,
            ),
        })
        .collect();
    let root = directory.path().join("operator-events");
    let saved = vidfish::operator_events::save_request(
        &root,
        vidfish::operator_events::Request {
            kind: "Grab All".into(),
            note: "sixteen hidden feeds".into(),
            unix_ms: marker_ms,
            elapsed_ns: 1_000_000_000,
            inputs,
            inventory: directory.path().join("cameras.json"),
            grab: true,
        },
    )
    .unwrap();
    assert_eq!(saved.event.captures.len(), 16);
    assert!(
        saved
            .event
            .captures
            .iter()
            .all(|c| c.image.as_ref().is_some_and(|p| p.exists())
                && c.capture_unix_ms.is_some()
                && c.delay_ms.is_some()
                && c.recording.run_id.is_some())
    );
    assert_eq!(saved.thumbnails.len(), 4);
    wait(supervisor, |s| {
        initial.iter().all(|before| {
            s.cameras.iter().any(|after| {
                after.camera_id == before.camera_id
                    && after.run_id == before.run_id
                    && after.state == State::Recording
                    && after.bytes_written > before.bytes_written
            })
        })
    });
    // Renaming changes current roster text only; durable references retain IDs.
    let id = store.cameras[0].id.clone();
    store.rename(&id, "Renamed after marker").unwrap();
    let recovered = vidfish::operator_events::load(&root).unwrap();
    assert_eq!(recovered[0].captures[0].camera, id);
    assert_eq!(recovered[0].captures[0].label, "Healthy");
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
        segment_seconds: 60,
        // File-output health is separate from the two-second stream-loss policy.
        stall_seconds: 30,
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
    let affected_preview = vidfish_media::Preview::start_with_stall_timeout(
        vidfish_media::Source::Rtsp(troubled.uri.clone()),
        vidfish_media::Codec::H264,
        Duration::from_millis(400),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while affected_preview.frames.borrow().is_none() {
        assert!(
            Instant::now() < deadline,
            "affected preview failed to start"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let old_frame = affected_preview.frames.borrow().as_ref().unwrap().number;
    let original_file = supervisor
        .snapshot()
        .borrow()
        .cameras
        .iter()
        .find(|c| c.camera_id == bad_id)
        .unwrap()
        .output
        .clone();
    troubled.set_paused(true);
    // A preview's short watchdog must not shorten the recording's own policy.
    let transient = Instant::now();
    while transient.elapsed() < Duration::from_secs(1) {
        let snapshot = supervisor.snapshot();
        let snapshot = snapshot.borrow();
        let status = snapshot
            .cameras
            .iter()
            .find(|c| c.camera_id == bad_id)
            .unwrap();
        assert_eq!(status.run_id, bad_run, "brief outage restarted recording");
        assert_eq!(
            status.state,
            State::Recording,
            "brief outage stopped recording"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let interrupted_bytes = supervisor
        .snapshot()
        .borrow()
        .cameras
        .iter()
        .find(|c| c.camera_id == bad_id)
        .unwrap()
        .bytes_written;
    troubled.set_paused(false);
    wait(supervisor, |s| {
        s.cameras.iter().any(|c| {
            c.camera_id == bad_id
                && c.run_id == bad_run
                && c.state == State::Recording
                && c.bytes_written > interrupted_bytes
        })
    });
    assert_eq!(
        supervisor
            .snapshot()
            .borrow()
            .cameras
            .iter()
            .find(|c| c.camera_id == bad_id)
            .unwrap()
            .output,
        original_file,
        "short outage started a new file"
    );
    // A genuinely sustained interruption still creates a gap and a new run.
    troubled.set_paused(true);
    let sustained = Instant::now();
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
        // The last video buffer can precede fixture pause by one frame interval.
        sustained.elapsed() >= Duration::from_millis(1800),
        "recording rolled over before two-second loss timeout"
    );
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
    let deadline = Instant::now() + Duration::from_secs(15);
    while affected_preview.frames.borrow().as_ref().is_none_or(|f| {
        f.number <= old_frame || f.received_at.elapsed() > Duration::from_millis(500)
    }) {
        assert!(
            Instant::now() < deadline,
            "affected preview failed to recover"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    affected_preview.join();
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
    for session in std::fs::read_dir(directory.path().join("media")).unwrap() {
        for entry in std::fs::read_dir(session.unwrap().path().join("recordings")).unwrap() {
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
        !std::fs::read_dir(&config.output_directory)
            .unwrap()
            .filter_map(Result::ok)
            .any(|session| session
                .path()
                .join("recordings")
                .read_dir()
                .is_ok_and(|runs| runs.filter_map(Result::ok).any(|run| run
                    .path()
                    .read_dir()
                    .is_ok_and(|files| files
                        .filter_map(Result::ok)
                        .any(|file| file.path().extension().is_some_and(|ext| ext == "mkv")))))),
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

#[test]
fn forget_requires_actual_recorder_stop_preserves_history_and_peer_and_auto_records_rediscovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let inventory = dir.path().join("cameras.json");
    let config = AppConfig {
        output_directory: dir.path().join("media"),
        ..Default::default()
    };
    config.save(&path).unwrap();
    let rtsp = RtspFixture::start(320, 240).unwrap();
    let camera = OnvifFixture::start_at(
        rtsp.uri.clone(),
        236,
        "127.0.0.1:0".parse().unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let peer_fixture = OnvifFixture::start_at(
        rtsp.uri.clone(),
        237,
        "127.0.0.1:0".parse().unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let mut store = CameraStore::default();
    let id = store
        .add_onvif("Obsolete alias", &camera.device_url)
        .unwrap();
    let peer_id = store.add_onvif("Peer", &peer_fixture.device_url).unwrap();
    store.save(&inventory).unwrap();
    let mut options = Options::new(inventory.clone(), Some(Ipv4Addr::LOCALHOST));
    let SocketAddr::V4(target) = camera.discovery_address else {
        panic!("IPv4 required")
    };
    options.target = target;
    options.discovery_interval = Duration::from_millis(200);
    let manager = Manager::start(options).unwrap();
    let snapshot = manager.snapshot();
    let supervisor = Supervisor::start_with_executable(
        path.clone(),
        config,
        manager.snapshot(),
        PathBuf::from(env!("CARGO_BIN_EXE_vidfish")),
    )
    .unwrap();
    let mut cleanup = Cleanup {
        manager: Some(manager),
        supervisor: Some(supervisor),
    };
    let manager = cleanup.manager.as_ref().unwrap();
    let supervisor = cleanup.supervisor.as_ref().unwrap();
    wait(supervisor, |s| {
        s.cameras.len() == 2
            && s.cameras
                .iter()
                .all(|c| c.state == State::Recording && c.bytes_written > 0)
    });
    manager
        .command(vidfish_onvif::manager::Command::Forget { id: id.clone() })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while snapshot.borrow().inventory_error.is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        CameraStore::load(&inventory)
            .unwrap()
            .cameras
            .iter()
            .any(|c| c.id == id)
    );
    let original = supervisor
        .snapshot()
        .borrow()
        .cameras
        .iter()
        .find(|c| c.camera_id == id)
        .unwrap()
        .clone();
    let peer = supervisor
        .snapshot()
        .borrow()
        .cameras
        .iter()
        .find(|c| c.camera_id == peer_id)
        .unwrap()
        .clone();
    supervisor
        .controller()
        .command(Control::Stop(id.clone()))
        .unwrap();
    wait(supervisor, |s| {
        s.cameras
            .iter()
            .any(|c| c.camera_id == id && c.state == State::Stopped)
    });
    let record_lock = dir.path().join("acquisition").join(format!("{id}.record"));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(guard) = CameraStore::lock(&record_lock) {
            drop(guard);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let stopped = vidfish::acquisition::read_status(&path, &id).unwrap();
    let media = stopped.output.as_ref().unwrap();
    let bytes = std::fs::read(media).unwrap();
    let session = original.session_directory.as_ref().unwrap();
    let original_run = vidfish::sessions::read(session)
        .unwrap()
        .runs
        .into_iter()
        .find(|r| r.id == original.run_id)
        .unwrap();
    let original_run = serde_json::to_value(original_run).unwrap();
    let historical = session.join("events").join("retained-event.json");
    std::fs::write(&historical, b"historical-camera-reference").unwrap();
    manager
        .command(vidfish_onvif::manager::Command::Forget { id: id.clone() })
        .unwrap();
    wait(supervisor, |s| {
        s.cameras.iter().any(|c| {
            c.camera_id != id
                && c.camera_id != peer_id
                && c.state == State::Recording
                && c.bytes_written > 0
        }) && s.cameras.iter().any(|c| {
            c.camera_id == peer_id
                && c.run_id == peer.run_id
                && c.bytes_written > peer.bytes_written
        })
    });
    let current = CameraStore::load(&inventory).unwrap();
    let new = current.cameras.iter().find(|c| c.id != peer_id).unwrap();
    assert_ne!(new.id, id);
    assert_ne!(new.alias, "Obsolete alias");
    assert!(new.credential_key.is_none());
    assert_eq!(std::fs::read(media).unwrap(), bytes);
    vidfish_media::inspect_recording(media).unwrap();
    assert_eq!(
        std::fs::read(&historical).unwrap(),
        b"historical-camera-reference"
    );
    let retained = vidfish::sessions::read(session)
        .unwrap()
        .runs
        .into_iter()
        .find(|r| r.id == original.run_id)
        .unwrap();
    assert_eq!(serde_json::to_value(retained).unwrap(), original_run);
    assert_eq!(
        vidfish::sessions::current(&path)
            .unwrap()
            .unwrap()
            .directory,
        *session
    );
    cleanup.supervisor.take().unwrap().finish_all().unwrap();
}

#[test]
fn two_recorded_cameras_share_their_connections_with_viewing_and_capture() {
    let root = tempfile::tempdir().unwrap();
    let config_path = root.path().join("config.json");
    let inventory = root.path().join("cameras.json");
    let config = AppConfig {
        output_directory: root.path().join("media"),
        segment_seconds: 60,
        ..Default::default()
    };
    config.save(&config_path).unwrap();
    let first = RtspFixture::start(320, 240).unwrap();
    let second = RtspFixture::start(320, 240).unwrap();
    let mut store = CameraStore::default();
    let first_id = store.add_manual("First", &first.uri).unwrap();
    let second_id = store.add_manual("Second", &second.uri).unwrap();
    store.save(&inventory).unwrap();
    let manager = Manager::start(Options::new(inventory.clone(), None)).unwrap();
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
                .all(|c| c.state == State::Recording && c.shared_feed.is_some())
    });
    let original = supervisor.snapshot().borrow().cameras.clone();
    let mut previews = Vec::new();
    for camera in &store.cameras {
        let (source, codec) = camera_preview::source(camera, None, &inventory).unwrap();
        assert!(matches!(source, vidfish_media::Source::Shared(_)));
        let preview = vidfish_media::Preview::start(source, codec);
        let until = Instant::now() + Duration::from_secs(5);
        while preview.frames.borrow().is_none() {
            assert!(
                Instant::now() < until,
                "shared preview did not deliver video"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let (source, codec) = camera_preview::source(camera, None, &inventory).unwrap();
        vidfish_media::snapshot(source, codec, Duration::from_secs(3)).unwrap();
        previews.push(preview);
    }
    assert_eq!(first.client_count(), 1);
    assert_eq!(second.client_count(), 1);
    first.disconnect_client(0);
    let disconnected = Instant::now();
    let until = disconnected + Duration::from_secs(5);
    loop {
        if first.client_count() >= 2
            && previews.iter().all(|p| {
                p.frames
                    .borrow()
                    .as_ref()
                    .is_some_and(|f| f.received_at > disconnected)
            })
        {
            break;
        }
        assert!(
            Instant::now() < until,
            "shared reader/viewer did not recover from TCP break"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    wait(supervisor, |s| {
        original.iter().all(|before| {
            s.cameras.iter().any(|c| {
                c.camera_id == before.camera_id
                    && c.run_id == before.run_id
                    && c.output == before.output
                    && c.state == State::Recording
                    && c.bytes_written > before.bytes_written
            })
        })
    });
    for preview in previews {
        preview.join();
    }
    assert_eq!(
        first.client_count(),
        2,
        "viewer opened an extra RTSP connection"
    );
    assert_eq!(second.client_count(), 1);
    supervisor
        .controller()
        .command(Control::Stop(first_id.clone()))
        .unwrap();
    wait(supervisor, |s| {
        s.cameras
            .iter()
            .any(|c| c.camera_id == first_id && c.state == State::Stopped)
    });
    let camera = store.cameras.iter().find(|c| c.id == first_id).unwrap();
    let (source, _) = camera_preview::source(camera, None, &inventory).unwrap();
    assert!(
        matches!(source, vidfish_media::Source::Rtsp(_)),
        "viewer-only camera did not return to direct access"
    );
    assert!(
        supervisor
            .snapshot()
            .borrow()
            .cameras
            .iter()
            .any(|c| c.camera_id == second_id && c.state == State::Recording)
    );
    cleanup.supervisor.take().unwrap().finish_all().unwrap();
    for session in std::fs::read_dir(root.path().join("media")).unwrap() {
        for log in std::fs::read_dir(session.unwrap().path().join("logs")).unwrap() {
            let text = std::fs::read_to_string(log.unwrap().path()).unwrap();
            for status in &original {
                assert!(
                    !text.contains(&status.shared_feed.as_ref().unwrap().identity),
                    "local feed credential leaked into scientific logs"
                );
            }
        }
    }
}
