# Epic #3: Reliable continuous recording

Implemented on the merged foundation and camera lifecycle:

- #13: Independent H.264/H.265 stream-copy recording, 16 recorder processes,
  preview isolation, and operator-visible media failures. Audio is optional and
  deliberately excluded; no recording depends on an audio track.
- #14: Configurable default 600-second keyframe MKV segmentation, exclusively
  reserved run directories, durable actual open/close boundary journals, EOS
  finalization, and preserved closed segments after abrupt recorder termination.
- #15: Default/persistent auto-record policy, manual per-camera/all-camera
  controls, disable-without-stop, and Stop All suppression until explicit resume/restart.
- #16: Actual file-growth health, free-space alerts and Warn/Stop policy, bounded
  stall detection/retry, isolated failures, and durable gap/recovery history.

See [continuous recording](../recording.md) for controls, settings, index semantics,
recovery, limits, and validation. Final native Windows/Linux review checks are the
completion checkpoint; physical Toughpad throughput must be measured on the device.
No automatic data deletion or inferred segment boundaries.
