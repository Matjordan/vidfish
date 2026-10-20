use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use vidfish_media::{Codec, SegmentedRecording, Source, segmented::FragmentEvent};

#[test]
fn keyframe_segments_have_actual_boundaries_and_play_after_clean_stop() {
    for (codec, container) in [
        (Codec::H264, "mkv"),
        (Codec::H265, "mkv"),
        (Codec::H264, "mp4"),
        (Codec::H265, "mp4"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let run = root.path().join("run");
        let source = Source::TestPattern {
            width: 320,
            height: 240,
        };
        let mut recording =
            SegmentedRecording::start_with_container(&source, codec, &run, 2, container).unwrap();
        recording.wait_started(Duration::from_secs(15)).unwrap();
        assert!(SegmentedRecording::start(&source, codec, &run, 2).is_err());
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            recording.poll(Duration::from_millis(100)).unwrap();
            let events: Vec<FragmentEvent> = std::fs::read_to_string(run.join("segments.jsonl"))
                .unwrap()
                .lines()
                .map(|s| serde_json::from_str(s).unwrap())
                .collect();
            if let Some(closed) = events.iter().find(|e| e.event == "closed") {
                // Read/seek the previous segment while a later muxer is still recording.
                let info = vidfish_media::inspect_recording(&closed.location).unwrap();
                assert!(info.seekable && info.duration_seconds > 0.0);
                let (_, _, pixels) = vidfish_media::recording_tools::software_frame(
                    &closed.location,
                    info.duration_seconds / 2.0,
                )
                .unwrap();
                assert!(
                    pixels
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .any(|p| p[0] > p[1] || p[2] > p[1]),
                    "decoded fixture is solid green"
                );
            }
            if events.iter().filter(|e| e.event == "opened").count() >= 3 {
                break;
            }
            assert!(Instant::now() < deadline, "segment rotation stalled");
        }
        // Stop as soon as a split is observed, with async predecessors potentially pending.
        let bytes = recording.finish().unwrap();
        assert!(bytes > 0);
        let events: Vec<FragmentEvent> = std::fs::read_to_string(run.join("segments.jsonl"))
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        let mut opened = BTreeMap::new();
        let mut closed = 0;
        let mut previous_end = None;
        for event in events {
            if event.event == "opened" {
                opened.insert(event.location.clone(), event.running_time_ns);
            } else {
                let start = opened.remove(&event.location).unwrap();
                assert!(event.running_time_ns > start);
                if let Some(previous) = previous_end {
                    assert_eq!(start, previous);
                }
                previous_end = Some(event.running_time_ns);
                let info = vidfish_media::inspect_recording(&event.location).unwrap();
                assert_eq!((info.width, info.height), (320, 240));
                assert!(info.seekable && info.duration_seconds > 0.0);
                assert_eq!(event.location.extension().unwrap(), container);
                closed += 1;
            }
        }
        assert!(closed >= 3, "segment setting did not rotate");
        assert!(opened.is_empty(), "clean shutdown left an unclosed segment");
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(run.join("finalization.json")).unwrap()).unwrap();
        assert_eq!(state["state"], "finalized");
        assert_eq!(state["pending"].as_array().unwrap().len(), 0);
    }
}
