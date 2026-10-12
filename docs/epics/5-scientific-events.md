# Epic #5: Scientific event marking and frame grabs

Depends on epic #3 segment metadata and epic #2 camera identities. Implementation has not started.

- [ ] #20: Event IDs, wall-clock UTC plus monotonic time, actual segment lookup/offsets, boundary resolution, and gaps.
- [ ] #21: All-camera snapshots using ONVIF where supported and temporary RTSP fallback, including hidden feeds.
- [ ] #22: Separate Mark, Grab All, and Mark+Grab actions, editable notes, durable JSON/CSV, and touch/keyboard controls.

Validation: split-boundary events; 16-camera hidden-feed grabs; explicit per-camera timestamps/delays/errors; crash/restart persistence; camera renames preserve references; snapshot operations never interrupt recording. Do not promise frame-perfect synchronization.
