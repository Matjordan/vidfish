# Operator monitoring

The desktop follows the [target layout](target-ui.svg): session/status header,
central camera previews, camera roster on the right, and collapsible event/snapshot
area beneath the video. Primary controls, camera editing, and input fields use
44-point touch targets. Settings open in a scrollable window. The header remains
visible when a camera is maximized or the event log is collapsed.

## Select, page, and maximize

Choose **Slot 1–4** in the roster and press a camera's **View feed**. A camera already
shown is focused rather than opened twice. One selected feed uses the central area;
two use one row; three/four use a 2×2 grid. Empty selections wait for a compatible
profile, including saved selections whose cameras are unavailable at startup.
The roster is sorted by alias, with local ID as a tie-breaker, and scrolls through
all 16 cameras. **Previous**, **Next**, and **Show this group** select consecutive
groups sized to the configured `preview_slots` (1–4). Paging joins old workers
before starting replacements. Use **Close** to release a slot's preview.

**Max** opens that slot in a fullscreen viewport. Other selected previews remain
bounded to the same total worker limit. **Return to grid** or Escape restores the
window and grid without changing recording or opening another decoder. Choosing
another slot/focusing a displayed camera while maximized changes the maximized
view. Saved `views.json` beside the configuration contains local camera IDs,
selected slot, event-panel visibility, and session display label. It contains no
camera URLs or logins. Reducing the configured slot limit preserves other choices;
selecting a camera from a hidden slot moves it safely into an active slot.

Use **Camera actions** for a large alias editor and login action. Alias changes
persist in the inventory and update displayed names without restarting a recording
or preview. `views.json` and inventory writes happen off the UI thread. Corrupt view
preferences produce a startup error rather than being silently overwritten.

## Recording confidence

Camera availability, preview state, and recording state are independent. Each feed
and roster row displays **NOT RECORDING** when acquisition is absent or unhealthy,
even when its preview is playing. The header shows actual recording count, cameras
not recording, recording/disk alerts, and minimum reported free disk space. Errors
and warnings appear in the roster and persistent header. Stale recorder heartbeats
cannot be displayed as healthy acquisition. Record all/Stop all and per-camera
Record/Stop use the independent recording supervisor.

The session display label is a view preference, not a scientific session ID.
The clock uses the earliest active recorder's actual video-start timestamp. With no
known active start timestamp, it explicitly shows viewer uptime. Reopening the viewer
reattaches to existing recorder status/start times. Full session organization and
scientific event timestamps and recording references are documented in events.md; named session organization remains Epic 6.

A frame older than five seconds is conspicuously **STALE**, including its age.
The last image remains visible for context. Error/stalled feeds do not block controls
or peer feeds. Each persistent preview automatically rebuilds after five seconds
without decoded frames, or after a pipeline error/end of stream. Initial connection
has a fifteen-second frame deadline. Retry delays grow from 250 ms to four seconds
plus up to 250 ms jitter, and reset after ten seconds of healthy frames. **Retry now**
on a feed bypasses the delay and rebuilds that preview only. Closing a slot cancels
retries. The last image stays visible with its stale age and current recovery status.

`VIDFISH_PREVIEW_STALL_SECONDS` tunes the preview watchdog (integer 1–60, default 5);
the initial-frame deadline is three times that setting. Keyframe requests and waiting
for a clean keyframe are enabled when supported by the installed depayloader. Logs
report encoded-buffer and RTP-buffer idle ages when the decoded-frame watchdog fires,
without including camera URLs or credentials. The optional UI probe records decoded-frame
age and texture-upload age separately (an upload is not proof of physical display
presentation). Recording has its own encoded-video
and file-progress watchdogs and recovery policy; preview health never clears a
recording failure.

## Scientific event controls

Mark + Grab All, Mark and Grab All now use the Epic 5 scientific event service.
Fresh captures include all registered cameras, even hidden feeds. Recording file
references use actual split boundaries with explicit clock uncertainty and gap
states. Notes are editable, JSON/CSV are durable, and recent history restores on
restart. See [events.md](events.md) for shortcuts, capture deadlines and recovery.
Collapsing this area never hides the recording header.

## Resource and platform validation

- At most four preview workers/RTSP decoders; a slot's old worker is joined before
  its replacement starts. Fullscreen reuses its existing worker. Configuring fewer
  slots lowers the limit. Duplicate selection focuses the existing camera connection.
- Appsink retains at most one sample and drops old frames. The watch channel keeps
  only the latest frame. Application frame packing accepts 1–4096 × 1–2160 pixels
  (at most 26,542,080 RGB bytes per frame), rejects invalid strides, and handles
  different incoming sizes; UI textures resize to the delivered dimensions.
- One capture worker captures sequentially with one temporary decoder/frame; images
  are encoded sequentially. Thumbnails are at most 128×128 pixels and only four from
  the latest event are retained. Summary history is bounded to 200 items. These are
  application queue/retention bounds, not a fixed total RSS promise for codec/driver
  buffers, GPU textures, or the allocator.
- GStreamer `decodebin` selects installed decoders by rank; hardware acceleration
  depends on the camera codec, plugins, GPU and driver. Logs name the actual selected
  decoder and desktop renderer separately. Rendering on a GPU does not establish
  hardware video decoding. Current RGB upload is not a zero-copy path.

The shared native input smoke requests 1280×800 and 800×600 windows and reports the
actual rendered viewports (a smaller Windows host may clamp the first window to
1024×768). It exercises all 16
camera choices, four-worker paging, 1080p/640×360 swaps, fullscreen/return, persisted
choices, nonblocking Mark during stalls, all-camera captures, note edits, restored
history, event-panel collapse, and a reduced one-worker limit.
The same recorder run IDs continue growing across these actions in the passing Linux
Xvfb check. Windows operator input automation remains unverified; Windows checks
are paused at the user's request. GitHub PR and manual CI compile and package Linux/Windows executables and installers
as downloadable artifacts. Automatic lint, tests, native smoke and package-launch
checks are disabled.
Versioned releases still compile and package Linux/Windows installers and uninstallers,
but skip optional check suites on both platforms. Focused checks run locally during
development. The reusable workflow retains opt-in `checks` and `windows_tests` switches;
leave them off unless the operator explicitly requests checks. Existing acquisition checks cover 16 simultaneous
recorder processes separately.

The operator observed that the earlier four-stream build looked fine. Numerical
Toughpad CPU/GPU measurements are deferred at the user's request. Native/cloud
functional checks are not a measurement of sustained physical Toughpad throughput.
The bundled `benchmark-toughpad.ps1` remains available when measurements are desired.

## Operator presentation

The operator screen uses a shared dark palette, consistent type and touch controls,
outlined selected feeds, separate recording/preview/connection states and a prominent
Mark + Grab All action. Camera rows distinguish ready, connecting, reconnecting,
offline, identity conflict and authentication required. **Hide cameras / Show cameras**
collapses the roster without changing preview workers or recorder ownership; this
choice persists with view preferences. Events remain collapsible, and preview images
fit within the remaining card area at smaller resolutions. Keyboard shortcuts remain
F6/F7/F8 for events, F9 for the session library, and Escape to return from fullscreen.

Camera cleanup lives in the **Camera actions** details panel. **Show hidden** and
**Clear offline cameras** are roster controls; bulk cleanup hides confirmed offline
entries and preserves their settings. Forgetting is separately confirmed and disabled
while recording. The native cleanup smoke exercises these touch controls, cancellation,
restart persistence and uninterrupted peer recording; see [camera cleanup](cameras.md#hide-restore-and-forget-cameras).
