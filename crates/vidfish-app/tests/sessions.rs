use std::{path::PathBuf, sync::Arc, time::Instant};
use vidfish::{
    event_export, library,
    operator_events::{self, CameraInput, Input, Request},
    retention, sessions,
};
use vidfish_core::{cameras::CameraStore, config::AppConfig};
struct Fixture {
    root: tempfile::TempDir,
    config: PathBuf,
    settings: AppConfig,
    event: PathBuf,
    segment: PathBuf,
    camera: String,
    session: PathBuf,
}
fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.json");
    let settings = AppConfig {
        output_directory: root.path().join("output with spaces"),
        disk_warning_bytes: 0,
        disk_stop_bytes: 0,
        ..Default::default()
    };
    settings.save(&config).unwrap();
    let lease = sessions::acquire(&config, &settings).unwrap();
    let mut cameras = CameraStore::default();
    let camera = cameras
        .add_manual("Bench original", "rtsp://127.0.0.1/live")
        .unwrap();
    let run = uuid::Uuid::new_v4().to_string();
    let directory = lease
        .add_run(
            &cameras.cameras[0],
            &run,
            "H264",
            Some(320),
            Some(240),
            &settings,
        )
        .unwrap();
    std::fs::create_dir(&directory).unwrap();
    let segment = directory.join("segment-00000000.mkv");
    vidfish_media::record_for(
        &vidfish_media::Source::TestPattern {
            width: 320,
            height: 240,
        },
        vidfish_media::Codec::H264,
        &segment,
        std::time::Duration::from_secs(1),
    )
    .unwrap();
    let journal = directory.join("segments.jsonl");
    let mut index = String::new();
    for (event, time) in [("opened", 0), ("closed", 1_000_000_000)] {
        index += &(serde_json::to_string(&vidfish_media::segmented::FragmentEvent {
            event: event.into(),
            location: segment.clone(),
            running_time_ns: time,
            observed_unix_ms: operator_events::now(),
        })
        .unwrap()
            + "\n");
    }
    std::fs::write(&journal, index).unwrap();
    let mut reference = vidfish::event_references::Reference {
        run_id: Some(run.clone()),
        journal: Some(journal),
        running_time_ns: Some(500_000_000),
        run_started_unix_ms: operator_events::now() - 1000,
        ..Default::default()
    };
    reference.resolve();
    std::fs::write(
        lease
            .session
            .directory
            .join("logs")
            .join(format!("{camera}-{run}.jsonl")),
        "{\"event\":\"recording_started\"}\n",
    )
    .unwrap();
    let saved = operator_events::save_request(
        &lease.session.directory.join("events"),
        Request {
            kind: "Mark + Grab All".into(),
            note: "valve, \"open\"".into(),
            unix_ms: operator_events::now(),
            elapsed_ns: 500_000_000,
            inventory: config.with_file_name("cameras.json"),
            grab: true,
            inputs: vec![CameraInput {
                input: Input {
                    camera: camera.clone(),
                    label: "Bench original".into(),
                    frame: Some(Arc::new(vidfish_media::Frame {
                        width: 2,
                        height: 2,
                        rgb: vec![127; 12],
                        number: 1,
                        received_at: Instant::now(),
                    })),
                },
                camera: None,
                profile: None,
                recording: reference,
            }],
        },
    )
    .unwrap();
    let event = lease.session.directory.join("events").join(saved.event.id);
    let session = lease.session.directory.clone();
    drop(lease);
    Fixture {
        root,
        config,
        settings,
        event,
        segment,
        camera,
        session,
    }
}
#[test]
fn filtered_catalog_and_portable_export_resolve_real_media_and_report_missing_files() {
    let f = fixture();
    let filter = library::Filter {
        camera: Some(f.camera.clone()),
        event: "valve".into(),
        from: library::date(operator_events::now()),
        to: library::date(operator_events::now()),
        ..Default::default()
    };
    let catalog = library::load(&f.config, &filter).unwrap();
    assert_eq!(catalog.events.len(), 1);
    assert_eq!(catalog.segments.len(), 1);
    assert!(catalog.segments[0].exists);
    assert_eq!(
        catalog.events[0].event.captures[0].recording.offset_ns,
        Some(500_000_000)
    );
    assert!(
        library::load(
            &f.config,
            &library::Filter {
                from: "2026-02-30".into(),
                ..Default::default()
            }
        )
        .is_err()
    );
    let dest = f.root.path().join("exports with spaces");
    let export = event_export::export(&f.event, &dest, 0).unwrap();
    assert!(
        export.complete,
        "{}",
        serde_json::to_string(&export).unwrap()
    );
    let event: operator_events::Event =
        serde_json::from_slice(&std::fs::read(export.directory.join("event.json")).unwrap())
            .unwrap();
    let relative = event.captures[0].recording.segment.as_ref().unwrap();
    assert!(relative.is_relative());
    let info = vidfish_media::inspect_recording(&export.directory.join(relative)).unwrap();
    assert_eq!((info.width, info.height), (320, 240));
    assert!(
        export
            .directory
            .join(event.captures[0].image.as_ref().unwrap())
            .exists()
    );
    assert!(export.directory.join("session.json").exists());
    assert!(export.directory.join("source/session.json").exists());
    assert!(export.directory.join("logs/camera-0.jsonl").exists());
    let exported_index = library::journal(
        &export
            .directory
            .join(event.captures[0].recording.journal.as_ref().unwrap()),
    )
    .unwrap();
    assert!(exported_index.iter().all(|r| r.location.is_relative()));
    let again = event_export::export(&f.event, &dest, 0).unwrap();
    assert_ne!(again.directory, export.directory);
    std::fs::remove_file(&f.segment).unwrap();
    let missing = library::load(&f.config, &filter).unwrap();
    assert!(!missing.segments[0].exists);
    let export = event_export::export(&f.event, &dest, 0).unwrap();
    assert!(!export.complete);
    assert!(export.items.iter().any(|i| i.status.contains("Missing")));
}
#[test]
fn open_segments_and_low_export_space_are_explicit_and_source_is_preserved() {
    let f = fixture();
    let index = f.segment.parent().unwrap().join("segments.jsonl");
    let rows = library::journal(&index).unwrap();
    std::fs::write(&index, serde_json::to_string(&rows[0]).unwrap() + "\n").unwrap();
    let original = std::fs::metadata(&f.segment).unwrap().len();
    let report = event_export::export(&f.event, &f.root.path().join("exports"), 0).unwrap();
    assert!(!report.complete);
    assert!(
        report
            .items
            .iter()
            .any(|i| i.status.contains("Open or unverified"))
    );
    assert!(
        !report
            .directory
            .join("recordings/camera-0/segment-00000000.mkv")
            .exists()
    );
    assert_eq!(std::fs::metadata(&f.segment).unwrap().len(), original);
    // Other tests can release disk space between the query and export. Use a
    // reserve that stays impossible regardless of concurrent filesystem work.
    let reserve = u64::MAX;
    let report = event_export::export(&f.event, &f.root.path().join("exports"), reserve).unwrap();
    assert!(!report.complete);
    assert!(
        report
            .items
            .iter()
            .any(|i| i.status.contains("Insufficient export storage"))
    );
    assert_eq!(std::fs::metadata(&f.segment).unwrap().len(), original);
}
#[test]
fn retention_is_opt_in_preserves_busy_active_and_unregistered_data() {
    let f = fixture();
    sessions::start_new(&f.config, &f.settings, "Next").unwrap();
    let mut old = sessions::read(&f.session).unwrap();
    old.closed_unix_ms = Some(operator_events::now() - 3 * 86400 * 1000);
    sessions::write_json(&f.session.join("manifest.json"), &old).unwrap();
    let legacy = f.settings.output_directory.join("legacy-camera.mkv");
    std::fs::write(&legacy, "scientific legacy data").unwrap();
    retention::apply(&f.config, &f.settings).unwrap();
    assert!(f.segment.exists());
    let lease = sessions::lease_directory(&f.session).unwrap();
    let mut opted = f.settings.clone();
    opted.retention_days = Some(1);
    let busy = retention::apply(&f.config, &opted).unwrap();
    assert!(busy.deleted.is_empty());
    assert!(!busy.skipped.is_empty());
    drop(lease);
    let report = retention::apply(&f.config, &opted).unwrap();
    assert_eq!(report.deleted, vec![old.id.clone()]);
    assert!(!f.session.exists());
    assert!(legacy.exists());
    assert!(
        sessions::current(&f.config)
            .unwrap()
            .unwrap()
            .directory
            .exists()
    );
    assert!(
        f.config
            .with_file_name("retention-audit")
            .join(format!("{}.json", old.id))
            .exists()
    );
}
#[test]
fn storage_warning_is_visible_without_starting_a_recording() {
    let f = fixture();
    let mut config = f.settings.clone();
    config.disk_warning_bytes = u64::MAX;
    let report = library::storage(&f.config, &config).unwrap();
    assert!(report.free_bytes.is_some());
    assert!(report.warning.is_some());
    assert!(report.bytes_per_second.is_none());
    assert_eq!(report.directory, f.session);
}

#[test]
fn cli_creates_and_renames_sessions_with_global_config_and_default_retention_preserves_data() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.json");
    let config = AppConfig {
        output_directory: root.path().join("media"),
        ..Default::default()
    };
    config.save(&path).unwrap();
    let invoke = |args: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_vidfish"))
            .arg("--config")
            .arg(&path)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let first = invoke(&["sessions", "new", "--name", "bench-a"]);
    let rename = invoke(&["sessions", "rename", "--name", "bench-b"]);
    assert_eq!(first["id"], rename["id"]);
    assert_eq!(first["directory"], rename["directory"]);
    assert_eq!(rename["label"], "bench-b");
    let list = invoke(&["sessions", "list"]);
    assert_eq!(list.as_array().unwrap().len(), 1);
    let browse = invoke(&["sessions", "browse"]);
    assert_eq!(browse["sessions"].as_array().unwrap().len(), 1);
    let retained = invoke(&["sessions", "retention"]);
    assert!(retained["deleted"].as_array().unwrap().is_empty());
    assert!(PathBuf::from(first["directory"].as_str().unwrap()).exists());
}
