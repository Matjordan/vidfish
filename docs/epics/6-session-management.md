# Epic #6: Sessions and recording management

Depends on epic #3 recordings and epic #5 event references. Implemented on the merged recording/event foundation.

- [x] #23: Named/automatic sessions with recordings, snapshots, logs, manifests, acquisition metadata, and collision-safe names.
- [x] #24: Filtered recording/event browsing, correct file references, and explicit missing-file reports.
- [x] #25: Configurable storage path, space estimates/warnings, opt-in retention, and complete event export.

Validation: hot-added cameras join sessions; duplicate names cannot overwrite data; exported references resolve or explicitly report missing media; browsing/export load never blocks recorders; retention defaults preserve all scientific data. Linux checks cover these workflows; Windows checks are paused by explicit user direction. See [session workflows and limits](../sessions.md).
