# Sessions, library and storage

Open **Sessions / recordings** (F9) to browse or manage acquisitions. Enter a test
ID/name or leave it blank for an automatic UTC name. Stop recordings and wait for
captures before **Start new session**; the backend rejects rollover while a recorder
or session operation owns it. Starting a session never stops or starts cameras.
**Rename active session** changes its label without moving files or changing IDs.
Viewer restarts and newly arriving cameras reuse the active session.

Under the configured output directory, each collision-safe
`session-YYYYMMDD-HHMMSS` folder contains `manifest.json`, `recordings/`,
`events/` (event JSON/CSV and PNG snapshots), and `logs/`. The manifest records stable
camera IDs, aliases, runs, codec, acquisition settings and negotiated dimensions/FPS
when available; unavailable values are null. Camera credentials are not included.
A registry beside the settings tracks sessions across output-directory changes.
Changing the output path takes effect for the next session, leaving existing media
in place. Recording processes retain session ownership independently of the viewer.

## Finding media

Choose a session, camera, inclusive UTC date range (`YYYY-MM-DD`), or event ID/kind/
note text, then **Refresh library**. Events show referenced files and estimated offsets;
missing media is explicit. Open recording uses VLC with an approximate start offset
when installed; otherwise the system file opener launches the file and reports the
manual seek time. Linux needs a configured file opener/player (typically xdg-utils).
There is no integrated playback timeline or exact exposure/frame synchronization.

Catalog loading, file checks and export run in background jobs. Results are paged and
capped at 2,000 events and 2,000 segments; narrow filters when truncated. Unindexed
recordings remain visible. Earlier `operator-events` folders remain browsable without
migration. An explicitly selected session is protected from retention while browsing.

## Portable event bundles

Choose an export directory outside the source session and **Export event**. Each
attempt creates a unique folder containing event JSON/CSV, snapshots, referenced
closed MKV segments, selected segment journals, relevant run logs and session metadata.
Canonical references are relative to the bundle; `source/` preserves original metadata
for provenance. Open fragments, missing files, recording gaps and copy failures are
reported in `export.json`; `complete: false` must not be interpreted as a complete
scientific archive. Retry after recording closes when an open segment is needed.
Export never modifies source recordings. Copies check free space before and during
streaming and retain the configured warning reserve; partial reports remain inspectable.

## Space and retention

Free space and low-space warnings are available without recording. Estimated remaining
time uses observed recording growth; it is unavailable until progress is measured and
is not a guarantee. Existing recorder disk warning/stop policy still applies.

Retention is **off by default**. Explicitly enable an age in Settings and **Save settings**
to opt in. Saved policy runs at startup and daily; **Apply saved retention** runs it now.
Only registered, closed sessions older than the policy are eligible. Active sessions,
recording/capture/export owners, selected library sessions, symlinked roots and legacy
unregistered files are skipped. Metadata and deletion intent are synced to
`retention-audit/` beside the settings before deletion. External player processes are
not ownership tracked; keep retention disabled when manually using older recordings.

## Command line

Use `vidfish --config /path/settings.json cameras sessions` followed by `list`,
`new --name "Test ID"`, `rename --name "Corrected ID"`, `browse`, `storage`,
`retention`, or `export <event-ID> --destination /path/export`.
Run each subcommand with `--help` for filters and session selection. Retention uses
only the saved opt-in policy. Commands do not implicitly start or stop recording.

Linux automated checks exercise manifests, ownership, real MKV exports, missing/open
media, opt-in deletion safeguards and native browser/export operation while recording.
Windows checks remain paused at the user's request; Windows release packaging continues.

## Public identifiers

New session IDs use UTC `YYYYMMDD-HHMMSS`; events add `-SSS` milliseconds.
An atomic directory reservation adds `-02`, `-03`, etc. on collisions, including
clock corrections and restarts. UTC avoids daylight-saving ambiguity in IDs;
metadata also stores RFC3339 UTC/local timestamps and the local offset at creation.
The UI displays local timestamps with offsets. IDs do not replace precise timing
metadata. Existing UUID folders and historical metadata are never renamed.

## Preset event labels and destination pickers

Session Details in the session library edits an ordered list of up to 16 preset
labels. Saving validates the active session identity so a session switch cannot
apply edits to the wrong acquisition. Presets are carried in the session manifest;
older manifests default to an empty list. Existing event notes are never rewritten.
Output and export destination fields provide native folder pickers; cancel preserves
the current value, and manual entry remains available.
