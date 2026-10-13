# Vidfish

Rust desktop software for scientific camera acquisition on Windows and Linux.
The foundation provides an egui desktop, persistent typed settings, structured
logging, an independently testable Tokio lifecycle, and GStreamer RTSP recording/
preview. Epic #2 adds ONVIF discovery, persistent camera identities/aliases, OS-backed
credentials, and independent camera polling/reconnection. Automatic recording,
segmentation, media-health recovery, events, and session management remain later work.

## Development

Install rustup and the GStreamer development/runtime prerequisites in
[docs/media.md](docs/media.md). `rust-toolchain.toml` pins Rust 1.99.0.

```sh
cargo run --locked -p vidfish
cargo test --locked --workspace --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
```

## Prototype workflows

```sh
cargo run --locked -p vidfish -- --media-check
cargo run --locked -p vidfish -- --demo --desktop-seconds 10
cargo run --locked -p vidfish -- --preview rtsp://CAMERA/PROFILE
cargo run --locked -p vidfish -- --record rtsp://CAMERA/PROFILE --output NEW_FILE.mkv
cargo run --locked -p vidfish -- --inspect NEW_FILE.mkv
cargo run --locked -p vidfish -- --check
cargo run --locked -p vidfish -- --service
cargo run --locked -p vidfish -- --discover --interface LOCAL_IPV4
cargo run --locked -p vidfish -- cameras list
```

Repeat `--preview` up to four times; add `--h265` for an H.265 profile. Recording
runs in a separate process and survives preview window closure/restart. Ctrl+C
finalizes the recording. The desktop and service discover/poll cameras when an adapter
is configured; preview and recording starts remain manual. There is no UI-to-service
IPC or automatic recording policy integration yet. See [media setup](docs/media.md)
and [camera discovery, credentials, and lifecycle](docs/cameras.md).

For a simulated 1080p camera with ONVIF endpoints:

```sh
cargo run --locked -p vidfish-media --example camera-fixture --features fixtures
```

Fixtures bind only loopback. They exercise RTSP stream-copy, preview restarts,
H.264/H.265 media inspection, ONVIF SOAP/WS-Discovery, and snapshots. Process smoke
tests verify recording remains usable after a preview process is killed.

## Settings

Settings use the platform configuration directory (`directories::ProjectDirs`),
or `--config PATH`. A missing file uses defaults without writing anything. Save
settings in the desktop to persist them. Writes run off the UI thread and use a
synced temporary file followed by atomic replacement. Invalid/future settings fail
visibly. Changes take effect on the next manager start.

```json
{
  "schema_version": 1,
  "output_directory": "recordings",
  "auto_record": true,
  "segment_seconds": 600,
  "max_cameras": 16,
  "preview_slots": 4,
  "camera_interface": null
}
```

Relative paths use the launch working directory. Auto-record/segment preferences
are foundations for the later recording engine, not enabled prototype behavior.
The prototype refuses existing output files and never deletes scientific data.
JSON structured logs go to stdout; `RUST_LOG=debug` increases detail. Settings have
no credential fields. Raw GStreamer error/debug strings are excluded from errors
because camera URIs can contain credentials.

## Branches, CI, and releases

[AGENTS.md](AGENTS.md) records the per-epic branches/PRs and commit rules. Each
verified coherent change is committed on its epic branch. Planning PRs remain drafts.

CI and release workflows share build, lint, tests, RTSP smoke, desktop launch checks,
and package steps. Draft PR updates run Linux only; marking a PR ready for review
runs both Windows and Linux, as do later updates to ready PRs. Manual CI runs and
releases always check both platforms. PR CI uploads executable packages. Every
push to `main`, including PR merges, publishes only after both platform packages pass.
Version `0.1.<release workflow run number>` appears in `--version`, Git tags, and
archive names. Failed runs may leave gaps; reruns retain their version and do not
replace an existing release. PR packages use `0.1.0-ci.<CI run number>`. The Cargo
manifest remains at development version `0.1.0`; update the release version prefix
to start a new major/minor series.

GitHub Releases include a Windows ZIP with GStreamer runtime, a Linux tarball with
runtime installation instructions, and SHA-256 checksums. No Rust installation is
needed to launch packages. See [deployment instructions](docs/media.md). Publishing
needs Actions enabled; its token has write access only in the publication job.

## Epic sequence and outstanding acceptance

1. #7: workspace, settings, desktop/manager lifecycle.
2. #8: GStreamer recording/preview spike and four-preview Toughpad measurements.
3. #9: Windows/Linux delivery, runtime packaging, and simulated camera fixtures.
4. #10–12: discovery, stable identities, and reconnection.
5. #13–16: isolated multi-camera recording, segments, controls, and health.
6. #17–19: operator preview controls and resource limits.
7. #20–25: event capture, sessions, browsing, retention, and export.

The operator reported successful four-stream operation after merging the foundation.
Cloud virtual-display checks establish functionality, not target hardware performance.
Epic #1/#8 remain incomplete until actual Toughpad CPU/GPU measurements and required
Windows/Linux acceptance evidence are recorded. Do not claim zero-copy preview upload,
frame-perfect synchronization, or sustained 16-camera acquisition from this spike.
