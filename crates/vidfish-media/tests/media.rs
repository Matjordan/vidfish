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
    assert!(std::fs::metadata(path).unwrap().len() > 1000);
}
