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
    let logs = String::from_utf8(result.stdout).unwrap();
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
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        let camera = if json.is_array() {
            &json[0]["camera"]
        } else {
            &json
        };
        assert_eq!(camera["serial_number"], "fixture-001");
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
    assert!(run(&["remove", id.trim()]).status.success());
}
