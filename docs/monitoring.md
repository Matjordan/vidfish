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

Use **Rename / login** for a large alias editor and login action. Alias changes
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
scientific timestamps remain the later session/event workflows.

A frame older than five seconds is conspicuously **STALE**, including its age.
The last image remains visible for context. Error/stalled feeds do not block controls
or peer feeds. **Focus feed**, or reselecting the group, retries the preview after
connectivity returns. Recording has its own stall deadline and bounded recovery
policy; preview health never clears a recording failure.

## Operator action bridge

**Mark + Grab All**, **Mark**, and **Grab All** accept an optional note and run in
one background worker. In this monitoring epic, grabs use the freshest selected
preview frames without opening extra RTSP connections. All registered cameras get
an explicit result; unviewed, unavailable, or stale previews are not silently
presented as captured. This scope is stated beside the controls. Camera-wide snapshot
RPCs, recording offsets, synchronized timestamps, and scientific event correlation
belong to Epic 5.

`operator-events/<event UUID>/event.json` beside `config.json` preserves the action,
UTC Unix milliseconds observed by its worker, viewer elapsed time, note, and each
camera's capture result. Successful images are lossless PNGs. The mark/note are
persisted before image work; metadata is replaced atomically after each result. A
failed image does not discard other results or the mark. Partial images remain for
recovery. `complete: false` means the worker did not finish all requested results;
`complete: true` can still contain explicit per-camera failures. Failed action saves
leave the typed note in the UI. No existing event directory/image is overwritten.
These preview frames are asynchronous, not frame-perfect or synchronized snapshots.

**Show events / Hide events** preserves video space on small screens. The log keeps
at most 200 current-viewer action summaries and thumbnails from only the latest action.
Durable event files remain on disk after restart; loading/browsing full scientific
history is a later epic. Collapsing this area never hides the recording header.

## Resource and platform validation

- At most four preview workers/RTSP decoders; a slot's old worker is joined before
  its replacement starts. Fullscreen reuses its existing worker. Configuring fewer
  slots lowers the limit. Duplicate selection focuses the existing camera connection.
- Appsink retains at most one sample and drops old frames. The watch channel keeps
  only the latest frame. Application frame packing accepts 1–4096 × 1–2160 pixels
  (at most 26,542,080 RGB bytes per frame), rejects invalid strides, and handles
  different incoming sizes; UI textures resize to the delivered dimensions.
- One event worker can pin at most four selected RGB frames in the desktop; images
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
choices, stale-frame exclusion, event-panel collapse, and a reduced one-worker limit.
The same recorder run IDs continue growing across these actions in the passing Linux
Xvfb check. Windows operator input automation remains unverified; Windows checks
are paused at the user's request. PR and manual CI run Linux only. Versioned releases
still compile/package Windows and check the bundled launcher, but skip Windows
test suites and GUI/runtime smoke. Existing acquisition checks cover 16 simultaneous
recorder processes separately.

The operator observed that the earlier four-stream build looked fine. Numerical
Toughpad CPU/GPU measurements are deferred at the user's request. Native/cloud
functional checks are not a measurement of sustained physical Toughpad throughput.
The bundled `benchmark-toughpad.ps1` remains available when measurements are desired.
