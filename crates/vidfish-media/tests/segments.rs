use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use vidfish_media::{Codec, SegmentedRecording, Source, segmented::FragmentEvent};

#[test]
fn keyframe_segments_have_actual_boundaries_and_play_after_clean_stop() {
    for codec in [Codec::H264, Codec::H265] {
        let root = tempfile::tempdir().unwrap();
        let run = root.path().join("run");
        let source = Source::TestPattern {
            width: 320,
            height: 240,
        };
        let mut recording = SegmentedRecording::start(&source, codec, &run, 2).unwrap();
        recording.wait_started(Duration::from_secs(15)).unwrap();
        assert!(SegmentedRecording::start(&source, codec, &run, 2).is_err());
        let deadline = Instant::now() + Duration::from_secs(6);
        while Instant::now() < deadline {
            recording.poll(Duration::from_millis(100)).unwrap();
        }
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
                closed += 1;
            }
        }
        assert!(closed >= 3, "segment setting did not rotate");
        assert!(opened.is_empty(), "clean shutdown left an unclosed segment");
    }
}
