use std::time::{Duration, Instant};
use vidfish_media::{Codec, Preview, Source, record_for};

#[test]
fn preview_delivers_rgb_and_recording_preserves_existing_files() {
    let source = Source::TestPattern {
        width: 321,
        height: 240,
    };
    let preview = Preview::start(source.clone(), Codec::H264);
    let start = Instant::now();
    loop {
        if let Some(frame) = preview.frames.borrow().as_ref() {
            assert_eq!((frame.width, frame.height), (321, 240));
            assert_eq!(frame.rgb.len(), 321 * 240 * 3);
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "{}",
            *preview.status.borrow()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("recording.mkv");
    record_for(
        &Source::TestPattern {
            width: 320,
            height: 240,
        },
        Codec::H264,
        &output,
        Duration::from_secs(1),
    )
    .unwrap();
    assert!(std::fs::metadata(&output).unwrap().len() > 1000);
    let info = vidfish_media::inspect_recording(&output).unwrap();
    assert_eq!((info.width, info.height), (320, 240));
    assert!(info.duration_seconds >= 0.5);
    assert!(info.caps.contains("video/x-h264"));
    let original = std::fs::read(&output).unwrap();
    assert!(record_for(&source, Codec::H264, &output, Duration::from_secs(1)).is_err());
    assert_eq!(std::fs::read(&output).unwrap(), original);
    preview.join();
}

#[test]
fn invalid_uri_and_unwritable_output_are_reported() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("invalid.mkv");
    assert!(
        record_for(
            &Source::Rtsp("not a camera".into()),
            Codec::H264,
            &output,
            Duration::from_millis(100)
        )
        .is_err()
    );
    assert!(!output.exists());
    assert!(
        record_for(
            &Source::TestPattern {
                width: 320,
                height: 240
            },
            Codec::H264,
            directory.path(),
            Duration::from_millis(100)
        )
        .is_err()
    );
}

#[cfg(feature = "fixtures")]
#[test]
fn rtsp_recording_survives_preview_restart() {
    let camera = vidfish_media::fixtures::RtspFixture::start(320, 240).unwrap();
    let source = Source::Rtsp(camera.uri.clone());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("camera.mkv");
    let recording = vidfish_media::Recording::start(&source, Codec::H264, &path).unwrap();
    for _ in 0..2 {
        let preview = Preview::start(source.clone(), Codec::H264);
        let start = Instant::now();
        while preview.frames.borrow().is_none() {
            recording.poll_error(Duration::from_millis(20)).unwrap();
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "{}",
                *preview.status.borrow()
            );
        }
        preview.join();
        recording.poll_error(Duration::from_millis(250)).unwrap();
    }
    recording.finish().unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 1000);
    let info = vidfish_media::inspect_recording(&path).unwrap();
    assert!(info.duration_seconds > 0.1);
    assert!(info.caps.contains("video/x-h264"));
}

#[cfg(feature = "fixtures")]
#[test]
fn h265_rtsp_stream_copy_produces_inspectable_media() {
    let camera = vidfish_media::fixtures::RtspFixture::start_codec(320, 240, Codec::H265).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("camera-h265.mkv");
    record_for(
        &Source::Rtsp(camera.uri.clone()),
        Codec::H265,
        &path,
        Duration::from_secs(2),
    )
    .unwrap();
    let info = vidfish_media::inspect_recording(&path).unwrap();
    assert_eq!((info.width, info.height), (320, 240));
    assert!(info.caps.contains("video/x-h265"));
    assert!(info.duration_seconds > 0.5);
}

#[test]
fn connection_errors_do_not_expose_camera_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("unreachable.mkv");
    let recording = vidfish_media::Recording::start(
        &Source::Rtsp("rtsp://fixture-user:fixture-password@127.0.0.1:1/camera".into()),
        Codec::H264,
        &path,
    )
    .unwrap();
    let start = Instant::now();
    let error = loop {
        if let Err(error) = recording.poll_error(Duration::from_millis(100)) {
            break error.to_string();
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "expected refused connection"
        );
    };
    assert!(!error.contains("fixture-user"));
    assert!(!error.contains("fixture-password"));
}
