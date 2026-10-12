# Vidfish

Rust desktop software for scientific camera acquisition on Windows and Linux.
The first implementation addresses the foundation in [issue #7](https://github.com/Matjordan/vidfish/issues/7).
It provides an egui settings shell and an independently testable Tokio manager lifecycle.
Camera discovery, video recording, previews, and event capture are **not implemented yet**.

## Development

Install Rust using rustup. The repository pins its toolchain in `rust-toolchain.toml`.

```sh
cargo run --locked -p vidfish
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

The desktop needs an active display and OpenGL. Linux also needs the runtime libraries
for X11 or Wayland, including libxkbcommon. No GStreamer dependencies are required yet.
Windows uses the native window backend. Windows launch and Toughpad touch performance
remain to be validated on actual hardware.

For headless development:

```sh
cargo run --locked -p vidfish --no-default-features -- --check
cargo run --locked -p vidfish --no-default-features -- --service
```

`--check` validates configuration and exercises manager startup/shutdown. `--service`
runs the manager until Ctrl+C, independently of a desktop process; it does not record
media yet. The current desktop starts its own manager and shuts it down when the
window closes. Service-to-desktop IPC and recording survival after desktop crashes
are follow-up work; do not run both expecting a shared manager.

## Settings

Settings default to the platform configuration directory (`directories::ProjectDirs`),
or a path supplied with `--config PATH`. A missing file uses defaults without writing
anything. Save settings in the desktop to persist them. Writes run off the UI thread
and use a synced temporary file followed by atomic replacement. Invalid or newer
configuration files fail visibly rather than being silently reset. Settings changes
take effect on the next manager start.

```json
{
  "schema_version": 1,
  "output_directory": "recordings",
  "auto_record": true,
  "segment_seconds": 600,
  "max_cameras": 16,
  "preview_slots": 4
}
```

Relative output directories are relative to the launch working directory. No files
are recorded or deleted by this foundation. JSON structured logs go to stdout;
set `RUST_LOG=debug` for more detail. Configuration has no credential fields.

## Implementation sequence

1. #7: foundation (this implementation); validate desktop launch on Windows/Linux.
2. #8: GStreamer recording and preview spike; measure target hardware before choosing media integration.
3. #9: extend the initial CI with packaging and simulated RTSP/ONVIF fixtures.
4. #10–12: discovery, stable identities, and camera reconnection.
5. #13–16: isolated recording, segment metadata, controls, and health.
6. #17–19: bounded four-slot previews and operator controls.
7. #20–25: event capture, sessions, browsing, retention, and export.

The engine crate has no UI dependency. Watch channels bound lifecycle communication;
settings writes use a single outstanding background operation. Future media workers
must keep recording independent of preview and event operations.
