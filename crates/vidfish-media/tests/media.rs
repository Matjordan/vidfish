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

#[cfg(feature = "fixtures")]
#[test]
fn saved_rtsp_credentials_support_preview_and_stream_copy_without_uri_passwords() {
    let fixture = vidfish_media::fixtures::RtspFixture::start_authenticated(
        320,
        240,
        "fixture-user".into(),
        "fixture-password".into(),
    )
    .unwrap();
    let source = vidfish_media::Source::AuthenticatedRtsp {
        uri: fixture.uri.clone(),
        username: zeroize::Zeroizing::new("fixture-user".into()),
        password: zeroize::Zeroizing::new("fixture-password".into()),
    };
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("authenticated.mkv");
    let recording =
        vidfish_media::Recording::start(&source, vidfish_media::Codec::H264, &output).unwrap();
    recording
        .wait_started(std::time::Duration::from_secs(10))
        .unwrap();
    let preview = vidfish_media::Preview::start(source, vidfish_media::Codec::H264);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while preview.frames.borrow().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "authenticated preview timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    preview.join();
    recording.finish().unwrap();
    let info = vidfish_media::inspect_recording(&output).unwrap();
    assert_eq!((info.width, info.height), (320, 240));
}

#[cfg(feature = "fixtures")]
fn wait_frame(preview: &Preview, after: u64) -> u64 {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(frame) = preview.frames.borrow().as_ref()
            && frame.number > after
            && frame.received_at.elapsed() < Duration::from_millis(500)
        {
            return frame.number;
        }
        assert!(
            Instant::now() < until,
            "no fresh frame: {}",
            *preview.status.borrow()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(feature = "fixtures")]
#[test]
fn silent_rtsp_stalls_recover_without_restarting_peer_or_resetting_frame_numbers() {
    for codec in [Codec::H264, Codec::H265] {
        let camera = vidfish_media::fixtures::RtspFixture::start_codec(320, 240, codec).unwrap();
        let healthy = vidfish_media::fixtures::RtspFixture::start(320, 240).unwrap();
        let preview = Preview::start_with_stall_timeout(
            Source::Rtsp(camera.uri.clone()),
            codec,
            Duration::from_secs(1),
        );
        let peer = Preview::start(Source::Rtsp(healthy.uri.clone()), Codec::H264);
        let before = wait_frame(&preview, 0);
        let peer_before = wait_frame(&peer, 0);
        camera.set_paused(true);
        std::thread::sleep(Duration::from_millis(150));
        camera.set_paused(false);
        let resumed = wait_frame(&preview, before);
        assert!(!preview.status.borrow().starts_with("Reconnecting"));
        camera.set_paused(true);
        let until = Instant::now() + Duration::from_secs(10);
        while !preview.status.borrow().starts_with("Reconnecting") {
            assert!(Instant::now() < until, "watchdog did not reconnect");
            std::thread::sleep(Duration::from_millis(20));
        }
        let stale = preview.frames.borrow().as_ref().unwrap().number;
        assert!(stale >= resumed);
        camera.set_paused(false);
        preview.retry_now();
        assert!(wait_frame(&preview, stale) > stale);
        assert!(wait_frame(&peer, peer_before) > peer_before);
        preview.join();
        peer.join();
    }
}

#[cfg(feature = "fixtures")]
#[test]
fn missing_keyframes_recover_after_idr_delivery_resumes() {
    for codec in [Codec::H264, Codec::H265] {
        let camera = vidfish_media::fixtures::RtspFixture::start_codec(320, 240, codec).unwrap();
        let source = Source::Rtsp(camera.uri.clone());
        let directory = tempfile::tempdir().unwrap();
        let mut recording = vidfish_media::SegmentedRecording::start(
            &source,
            codec,
            &directory.path().join("run"),
            2,
        )
        .unwrap();
        recording.wait_started(Duration::from_secs(10)).unwrap();
        camera.set_drop_keyframes(true);
        // Keep the shared server producing interframes, then join without an IDR.
        std::thread::sleep(Duration::from_millis(200));
        let preview = Preview::start_with_stall_timeout(source, codec, Duration::from_secs(1));
        let until = Instant::now() + Duration::from_secs(12);
        while !preview.status.borrow().starts_with("Reconnecting") {
            recording.poll(Duration::from_millis(20)).unwrap();
            assert!(
                Instant::now() < until,
                "missing IDR did not trigger recovery: {}",
                *preview.status.borrow()
            );
        }
        camera.set_drop_keyframes(false);
        wait_frame(&preview, 0);
        preview.join();
        recording.finish().unwrap();
    }
}

#[test]
fn closing_preview_cancels_reconnect_backoff() {
    // Reserve a loopback port number without accepting TCP connections.
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    let preview = Preview::start(
        Source::Rtsp(format!("rtsp://127.0.0.1:{port}/camera")),
        Codec::H264,
    );
    let until = Instant::now() + Duration::from_secs(8);
    while !preview.status.borrow().starts_with("Reconnecting") {
        assert!(Instant::now() < until, "connection failure was not retried");
        std::thread::sleep(Duration::from_millis(10));
    }
    let start = Instant::now();
    preview.join();
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "stop waited for reconnect backoff"
    );
    drop(socket);
}
