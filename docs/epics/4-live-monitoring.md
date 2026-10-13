# Epic #4: Four-camera live monitoring UI

Implemented on the merged recording/camera foundation:

- #17: Responsive one/two/2×2 previews, four selected slots, paging across 16 cameras,
  duplicate-camera focus, source swaps, and independent recorder lifetime.
- #18: Right-hand touch roster, persistent aliases/selections, availability/preview/
  recording distinctions, conspicuous failures/stale heartbeats, and recording/storage
  header with actual active-recorder elapsed time and a session display label.
- #19: Fullscreen/return, stale-frame alerts, resolution-aware textures, bounded latest
  frame queues and worker count, decoder/renderer logging, and documented resource limits.

The operator layout substantially follows the checked-in target. Functional Mark/
Grab controls provide a durable selected-preview action bridge, optional notes,
explicit capture failures, and a collapsible event/thumbnail panel; scientific
camera-wide snapshot/time/recording-offset correlation belongs to Epic 5.

See [operator monitoring](../monitoring.md) for workflows and limits. Native mouse-input
smoke tests exercise 1280×800/800×600, all 16 choices, four workers, fullscreen, persisted
selections, stalls, changed resolutions, a reduced one-worker configuration, and
unchanged recording run IDs/file progress. Final Windows/Linux review validation is
required. The user explicitly deferred numerical Toughpad measurements and reported
that the earlier four-stream build looked fine; do not invent hardware measurements.
