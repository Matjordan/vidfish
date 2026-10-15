use std::process::Command;

#[test]
fn check_runs_without_display_and_does_not_create_settings() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let result = Command::new(env!("CARGO_BIN_EXE_vidfish"))
        .args(["--check", "--config"])
        .arg(&path)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env("RUST_LOG", "info")
        .output()
        .unwrap();
    assert!(result.status.success(), "{:?}", result);
    assert!(!path.exists());
    let logs = String::from_utf8(result.stderr).unwrap();
    assert!(logs.contains("camera manager started"));
    assert!(logs.contains("camera manager stopped"));
}

#[test]
fn check_rejects_corrupt_config_without_overwriting_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    std::fs::write(&path, "broken json").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_vidfish"))
        .args(["--check", "--config"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "broken json");
}

#[test]
fn conflicting_modes_and_missing_recording_output_fail_before_startup() {
    for args in [
        vec!["--demo", "--check"],
        vec!["--record", "rtsp://127.0.0.1:1/camera"],
        vec!["--desktop-seconds", "1"],
        vec!["--onvif", "http://127.0.0.1/onvif", "--check"],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_vidfish"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(
            !String::from_utf8(result.stdout)
                .unwrap()
                .contains("camera manager started")
        );
    }
}

#[test]
fn onvif_and_discovery_cli_resolve_fixture_without_creating_settings() {
    let camera =
        vidfish_media::fixtures::OnvifFixture::start("rtsp://127.0.0.1/live".into()).unwrap();
    for args in [
        vec!["--onvif".into(), camera.device_url.clone()],
        vec![
            "--discover".into(),
            "--interface".into(),
            "127.0.0.1".into(),
            "--discovery-target".into(),
            camera.discovery_address.to_string(),
        ],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_vidfish"))
            .args(args)
            .env("RUST_LOG", "info")
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        let camera = if json.is_array() {
            &json[0]["camera"]
        } else {
            &json
        };
        assert_eq!(
            camera["serial_number"],
            "fixture-001",
            "ONVIF/discovery response: {json}; stderr: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(camera["profiles"][0]["codec"], "H264");
    }
}

#[test]
fn camera_inventory_commands_persist_aliases_and_reject_credentials_in_urls() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cameras.json");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_vidfish"))
            .args(["cameras", "--inventory"])
            .arg(&path)
            .args(args)
            .output()
            .unwrap()
    };
    let added = run(&["add", "--alias", "Bench", "--rtsp", "rtsp://127.0.0.1/live"]);
    assert!(added.status.success(), "{added:?}");
    let id = String::from_utf8(added.stdout).unwrap();
    assert!(run(&["rename", id.trim(), "Vehicle left"]).status.success());
    let list = run(&["list"]);
    let store: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(store["cameras"][0]["alias"], "Vehicle left");
    let bad = run(&[
        "add",
        "--alias",
        "Invalid",
        "--rtsp",
        "rtsp://user:secret@localhost/live",
    ]);
    assert!(!bad.status.success());
    assert!(!String::from_utf8(bad.stderr).unwrap().contains("secret"));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&run(&["list"]).stdout).unwrap()["cameras"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(!run(&["remove", id.trim()]).status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&run(&["list"]).stdout).unwrap()["cameras"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(run(&["remove", id.trim(), "--confirm"]).status.success());
}

#[test]
fn headless_camera_watch_reports_real_availability_and_exits_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cameras.json");
    let fixture =
        vidfish_media::fixtures::OnvifFixture::start("rtsp://127.0.0.1/live".into()).unwrap();
    let mut store = vidfish_core::cameras::CameraStore::default();
    store.add_onvif("Bench", &fixture.device_url).unwrap();
    store.save(&path).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_vidfish"))
        .args(["cameras", "--inventory"])
        .arg(&path)
        .args(["watch", "--seconds", "5"])
        .env_remove("DISPLAY")
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(
        output
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .any(|value| value["cameras"][0]["state"] == "Available"),
        "{output}"
    );
    assert!(vidfish_core::cameras::CameraStore::lock(&path).is_ok());
}

#[test]
fn camera_login_cli_creates_local_storage_and_keeps_credentials_out_of_listing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cameras.json");
    let mut inventory = vidfish_core::cameras::CameraStore::default();
    let id = inventory
        .add_manual("Bench", "rtsp://127.0.0.1/live")
        .unwrap();
    inventory.save(&path).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_vidfish"))
            .args(["cameras", "--inventory"])
            .arg(&path)
            .args(args)
            .env("VIDFISH_FIXTURE_USERNAME", "fixture-user")
            .env("VIDFISH_FIXTURE_PASSWORD", "never-log-fixture-password")
            .output()
            .unwrap()
    };
    let saved = run(&[
        "set-credentials",
        &id,
        "--username-env",
        "VIDFISH_FIXTURE_USERNAME",
        "--password-env",
        "VIDFISH_FIXTURE_PASSWORD",
    ]);
    assert!(saved.status.success(), "{saved:?}");
    assert!(
        !String::from_utf8(saved.stdout)
            .unwrap()
            .contains("never-log-fixture-password")
    );
    assert!(
        !String::from_utf8(saved.stderr)
            .unwrap()
            .contains("never-log-fixture-password")
    );
    let list = run(&["list"]);
    assert!(
        !String::from_utf8(list.stdout)
            .unwrap()
            .contains("never-log-fixture-password")
    );
    let credentials = vidfish_onvif::credentials::file_path(&path);
    let defaults = run(&[
        "set-default-credentials",
        "--username-env",
        "VIDFISH_FIXTURE_USERNAME",
        "--password-env",
        "VIDFISH_FIXTURE_PASSWORD",
    ]);
    assert!(defaults.status.success());
    assert!(!String::from_utf8_lossy(&defaults.stdout).contains("never-log-fixture-password"));
    assert!(!String::from_utf8_lossy(&defaults.stderr).contains("never-log-fixture-password"));
    assert!(
        vidfish_onvif::credentials::defaults(&credentials)
            .unwrap()
            .is_some()
    );
    assert!(vidfish_onvif::credentials::load(&credentials, &id).is_ok());
    assert!(run(&["forget-credentials", &id]).status.success());
    assert!(vidfish_onvif::credentials::load(&credentials, &id).is_err());
    assert!(
        vidfish_onvif::credentials::defaults(&credentials)
            .unwrap()
            .is_some()
    );
    assert!(run(&["forget-default-credentials"]).status.success());
    assert!(
        vidfish_onvif::credentials::defaults(&credentials)
            .unwrap()
            .is_none()
    );
    assert!(
        vidfish_core::cameras::CameraStore::load(&path)
            .unwrap()
            .cameras[0]
            .credential_key
            .is_none()
    );
}
