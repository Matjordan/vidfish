# Epic 5: Scientific event marking and frame grabs

Implemented on the merged Epic 4 foundation in PR #30.

- #20: stable IDs, UTC action time, monotonic acquisition clock epoch (viewer fallback),
  actual split-journal lookup, approximate media offsets, pending-close reconciliation,
  missing-file/gap states and rename-independent references.
- #21: fresh ONVIF HTTP snapshots or bounded temporary RTSP extraction, including
  hidden feeds; per-camera timestamps, delays, methods and failures; serial capture
  independent of acquisition; 16-camera recording/capture integration validation.
- #22: independent touch actions and F6/F7/F8 shortcuts, editable notes, synced atomic
  JSON and derived CSV, incremental crash-safe results and restart recovery.

See [operator and post-processing instructions](../events.md) for clock uncertainty,
resource bounds, acquisition-clock epoch changes and session-management boundaries.
Windows checks remain paused at the user's request. Linux validation is required.
