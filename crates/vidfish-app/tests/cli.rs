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
