# Epic #3: Reliable continuous recording

Depends on epic #1 media validation and epic #2 camera lifecycle. Implementation has not started.

- [ ] #13: Independent H.264 stream-copy recording pipelines; evaluate H.265 and optional audio.
- [ ] #14: Default 600-second MKV segmentation, collision-safe paths, and actual segment boundary index.
- [ ] #15: Persistent auto-record policy, explicit per-camera/all-camera controls, and Stop All suppression.
- [ ] #16: Actual file-progress health, disk warnings, write/stall failures, recovery, and gap records.

Validation: 16 independent simulated streams; preview switches cannot stop recordings; clean and abrupt termination; disk-full and stalled-stream injection; keyframe-aligned boundary metadata. No automatic data deletion. Report actual recording state separately from preview health.
