#![cfg(unix)]
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use vidfish::{
    acquisition::{State, read_status},
    library, sessions,
};
use vidfish_core::{cameras::CameraStore, config::AppConfig};
use vidfish_media::fixtures::RtspFixture;

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn wait(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(40);
    while !predicate() {
        assert!(Instant::now() < deadline, "recording acceptance timeout");
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn setup(root: &Path, uri: &str, container: &str, count: usize) -> (PathBuf, Vec<String>) {
    let config = root.join("config.json");
    AppConfig {
        output_directory: root.join("media"),
        recording_container: container.into(),
        segment_seconds: 3,
        disk_warning_bytes: 0,
        disk_stop_bytes: 0,
        ..Default::default()
    }
    .save(&config)
    .unwrap();
    let mut cameras = CameraStore::default();
    let ids = (0..count)
        .map(|i| cameras.add_manual(&format!("Camera {i}"), uri).unwrap())
        .collect();
    cameras.save(&root.join("cameras.json")).unwrap();
    (config, ids)
}
fn launch(config: &Path, extra: &[&str]) -> Worker {
    let log = std::fs::File::create(config.with_extension(format!("{}.log", uuid::Uuid::new_v4())))
        .unwrap();
    Worker(
        Command::new(env!("CARGO_BIN_EXE_vidfish"))
            .arg("--config")
            .arg(config)
            .args(extra)
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap(),
    )
}
fn complete(run: &Path) {
    let events = library::journal(&run.join("segments.jsonl")).unwrap();
    let opened: Vec<_> = events.iter().filter(|e| e.event == "opened").collect();
    assert!(!opened.is_empty());
    for start in opened {
        assert!(
            events
                .iter()
                .any(|e| e.event == "closed" && e.location == start.location),
            "stop stranded a segment"
        );
        let info = vidfish_media::inspect_recording(&start.location).unwrap();
        assert!(info.seekable && info.duration_seconds > 0.0);
        vidfish_media::recording_tools::software_frame(
            &start.location,
            info.duration_seconds / 2.0,
        )
        .unwrap_or_else(|error| panic!("{error}: {info:?}, {}", start.location.display()));
    }
    let lifecycle: serde_json::Value =
        serde_json::from_slice(&std::fs::read(run.join("finalization.json")).unwrap()).unwrap();
    assert_eq!(lifecycle["state"], "finalized");
}

#[test]
fn individual_stop_then_service_sigterm_finalize_all_cameras() {
    let root = tempfile::tempdir().unwrap();
    let camera = RtspFixture::start(320, 240).unwrap();
    let (config, ids) = setup(root.path(), &camera.uri, "mp4", 2);
    let mut service = launch(&config, &["--service"]);
    wait(|| {
        ids.iter().all(|id| {
            read_status(&config, id)
                .is_some_and(|s| s.state == State::Recording && s.bytes_written > 0)
        })
    });
    let before: Vec<_> = ids
        .iter()
        .map(|id| read_status(&config, id).unwrap())
        .collect();
    wait(|| {
        before.iter().all(|s| {
            library::journal(
                &s.output
                    .as_ref()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join("segments.jsonl"),
            )
            .is_ok_and(|rows| rows.iter().any(|e| e.event == "closed"))
        })
    });
    std::fs::write(
        root.path()
            .join("acquisition")
            .join(format!("{}.stop", ids[0])),
        &before[0].run_id,
    )
    .unwrap();
    wait(|| read_status(&config, &ids[0]).is_some_and(|s| s.state == State::Stopped));
    complete(before[0].output.as_ref().unwrap().parent().unwrap());
    let peer = read_status(&config, &ids[1]).unwrap();
    assert_eq!(peer.run_id, before[1].run_id);
    assert_eq!(peer.state, State::Recording);
    assert!(
        Command::new("kill")
            .args(["-TERM", &service.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    wait(|| service.0.try_wait().unwrap().is_some());
    assert!(service.0.wait().unwrap().success());
    for id in ids {
        let status = read_status(&config, &id).unwrap();
        assert_eq!(status.state, State::Stopped);
        complete(status.output.as_ref().unwrap().parent().unwrap());
    }
}

#[test]
fn abrupt_kill_preserves_prior_files_and_recovers_orphan_without_overwrite() {
    for container in ["mkv", "mp4"] {
        let root = tempfile::tempdir().unwrap();
        let camera = RtspFixture::start(320, 240).unwrap();
        let (config, ids) = setup(root.path(), &camera.uri, container, 1);
        let mut worker = launch(&config, &["--record-camera", &ids[0]]);
        wait(|| {
            read_status(&config, &ids[0])
                .is_some_and(|s| s.state == State::Recording && s.output.is_some())
        });
        let run = read_status(&config, &ids[0])
            .unwrap()
            .output
            .unwrap()
            .parent()
            .unwrap()
            .to_owned();
        wait(|| {
            library::journal(&run.join("segments.jsonl"))
                .is_ok_and(|rows| rows.iter().any(|e| e.event == "closed"))
        });
        // Give the current partial MP4 at least one independently written fragment.
        std::thread::sleep(Duration::from_millis(1700));
        worker.0.kill().unwrap();
        worker.0.wait().unwrap();
        let before: Vec<_> = std::fs::read_dir(&run)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|s| s == container))
            .map(|e| (e.path(), std::fs::read(e.path()).unwrap()))
            .collect();
        let catalog = library::load(&config, &Default::default()).unwrap();
        assert!(
            catalog
                .warnings
                .iter()
                .any(|w| w.contains("recovered-copy")),
            "{:?}",
            catalog.warnings
        );
        assert!(
            catalog
                .segments
                .iter()
                .any(|s| s.path.to_string_lossy().contains(".recovered-"))
        );
        for (path, bytes) in before {
            assert_eq!(
                std::fs::read(path).unwrap(),
                bytes,
                "recovery changed original"
            );
        }
        let catalog2 = library::load(&config, &Default::default()).unwrap();
        assert_eq!(
            catalog.segments.len(),
            catalog2.segments.len(),
            "recovery was repeated"
        );
        assert!(!sessions::list(&config).unwrap()[0].runs.is_empty());
    }
}

#[test]
fn recovery_skips_live_writer_and_failed_remux_preserves_existing_bytes() {
    let root = tempfile::tempdir().unwrap();
    let camera = RtspFixture::start(320, 240).unwrap();
    let (config, ids) = setup(root.path(), &camera.uri, "mkv", 1);
    let mut worker = launch(&config, &["--record-camera", &ids[0]]);
    wait(|| {
        read_status(&config, &ids[0])
            .is_some_and(|s| s.state == State::Recording && s.bytes_written > 0)
    });
    let catalog = library::load(&config, &Default::default()).unwrap();
    assert!(catalog.warnings.iter().all(|w| !w.contains("Recovery")));
    let original = root.path().join("broken.mkv");
    let output = root.path().join("repaired.mkv");
    std::fs::write(&original, b"damaged footage").unwrap();
    assert!(
        vidfish_media::recording_tools::remux(
            &original,
            &output,
            vidfish_media::Codec::H264,
            "mkv"
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&original).unwrap(), b"damaged footage");
    assert!(!output.exists());
    std::fs::write(&output, b"owned").unwrap();
    assert!(
        vidfish_media::recording_tools::remux(
            &original,
            &output,
            vidfish_media::Codec::H264,
            "mp4"
        )
        .is_err()
    );
    assert_eq!(std::fs::read(output).unwrap(), b"owned");
    let status = read_status(&config, &ids[0]).unwrap();
    std::fs::write(
        root.path()
            .join("acquisition")
            .join(format!("{}.stop", ids[0])),
        status.run_id,
    )
    .unwrap();
    wait(|| worker.0.try_wait().unwrap().is_some());
    assert!(worker.0.wait().unwrap().success());
}
