# Operator monitoring

Vidfish uses a live camera workspace, a scrollable camera roster, a recording-health
header, and a resizable event section. Controls use touch-sized targets. Viewing,
thumbnail collection and event work are independent of recorder ownership.

## Dynamic camera workspace

Drag a camera alias from the roster into empty workspace to add a view, or onto a
tile to replace it. Drag tiles onto each other to swap their positions. Empty tiles
are not retained in the layout. One to eight views reflow to suit the window;
the configured preview limit can be reduced in Settings and applies after restart.
Existing configurations retain their earlier preview limit until changed.

For a non-drag workflow, choose **Add to view**, or select a tile and choose
**Replace selected view** on a camera. **Camera actions** includes **Move view
earlier/later** and **Retry now** for a displayed camera. **×** removes a view
without forgetting the camera or stopping its recording. At the preview limit,
adding another camera is refused; replacing an existing tile remains available.
**Show this group** fills the available views from the start of the alias-sorted roster.

Double-tap a viewport or a camera alias to open fullscreen. Each tile also has a
semi-transparent corner expand/restore control; sidebar cameras have a Fullscreen
button. A camera outside the grid temporarily borrows a view, including when all
eight are occupied. **Return to grid**, Escape, or the restore control returns the
previous selections and order. If a temporary camera is still switching, wait for
that transition before restoring. No fullscreen operation changes recording.

`views.json` beside the configuration persists selections, ordering, roster visibility,
thumbnail interval and event-section visibility. Older four-slot preferences load
without renaming cameras. There is no dedicated slot selector in the roster.
Unavailable selected cameras remain placeholders instead of being replaced silently.
View saves run off the UI thread and contain no camera URLs or passwords.

## Roster thumbnails

Each available camera has a small aspect-fitted thumbnail, including cameras outside
the grid. The default refresh target is five seconds; Settings allows 1–60 seconds.
The UI shows the image's local receipt age and clearly labels stale images and
unavailable cameras. Receipt time is not camera exposure time.

A displayed frame is reused where possible. Otherwise one background worker makes
one bounded still capture at a time. Recorded cameras use the recorder's local
encoded feed; unrecorded cameras prefer ONVIF HTTP snapshots and fall back to a
one-shot RTSP capture. Each camera retains only its latest 128×72 thumbnail.
Unresponsive cameras can make refreshes take longer than five seconds; thumbnails
are never presented as guaranteed synchronized or continuously live images.

## One camera connection while recording

Each recording worker owns one camera RTSP connection. Viewers and temporary RTSP
still captures use its bounded local encoded feed. Slow viewers discard their own
backlog until a keyframe and cannot block the recorder. Preview-only cameras use
direct RTSP. Starting/stopping recording automatically changes the viewing route.
Older recording workers without a local feed must be stopped and restarted before
shared viewing can be used; Vidfish does not fall back to a second camera stream.

Up to eight preview decoders can run, plus a bounded temporary decoder for each
independent sequential capture worker (event grabs and thumbnails). Appsink/watch
channels retain the latest preview frame, not an unbounded queue. Decoder/driver
buffers and GPU textures still consume resources. Eight full-resolution previews
can cost substantially more CPU/GPU than four; reduce the configured preview limit
on constrained hardware. Recorded video is never resized or re-encoded for the UI.

## Recording confidence and preview recovery

Camera availability, preview state and recording health are independent. The header
shows actual recording count, cameras not recording, recording/disk alerts, and
free space. A playing preview is not proof that the recording file is growing.
Stale recorder heartbeats are not displayed as healthy acquisition.

A preview older than five seconds remains visible with its stale age. Preview
workers rebuild after a pipeline failure or the decoded-frame watchdog, with bounded
retry delays. **Camera actions → Retry now** retries that viewer only. Removing a
view cancels its retries. `VIDFISH_PREVIEW_STALL_SECONDS` tunes the watchdog (1–60,
default 5); the initial-frame deadline is three times that value. Recording uses its
own two-second input-loss grace and file-progress watchdog; see [recording](recording.md).

## Events, details and presets

F6 **Mark** saves a timestamp and note. F7 **Grab All** requests fresh images from
all registered cameras. F8 **Mark + Grab All** saves the currently displayed frames,
without ONVIF requests. Cameras without a displayed frame are explicitly uncaptured.
A frozen displayed image is retained with its original receipt timestamp.

Expand **Show events** and drag the panel boundary to resize it. Newest-first rows
show local date/time, type, elapsed time, capture/file coverage, a short public ID
and a note excerpt. Search time, note, type, ID or camera; optionally reverse the
sort. Rendering uses virtual rows. The newest 10,000 events are loaded off-thread;
use the session library for historical session/date/camera browsing.

Single-tap or double-tap selects an event. One persistent details pane shows its full
note, local/UTC timestamps, exact ID, elapsed clock scope, camera coverage, linked
recording paths, estimated offsets and capture errors. On narrow screens it sits
below the list. Details scroll independently and contain no images. **Edit note**,
**Copy timestamp / ID**, **Open recording** and **View snapshots** avoid transcribing
identifiers. Snapshot viewing is a separate optional window with system-viewer links.
Missing files produce an actionable error when opened, without blocking recording.

List scroll anchoring and selection are retained when events arrive. **Follow newest**
explicitly follows new events; new-event highlighting does not take keyboard focus.
Long notes remain available in details and the note editor.

**Session Details** opens the session library's preset editor. Add, rename, reorder
and remove up to 16 labels and choose **Save event presets**. Quick preset buttons
create ordinary timestamped Mark events with the chosen label as the note and normal
recording references. Presets belong to the active session. Edits affect future
marks only and never rename historical events. Free-form Mark remains available.

See [events](events.md) and [sessions](sessions.md) for durable storage and exports.

## Paths and platform checks

Settings output and library export destinations both provide native **Browse…**
folder dialogs; manual typing/pasting remains available. Cancel leaves the existing
path unchanged. Dialogs run on a worker and never launch from headless/CLI modes.
Linux uses the desktop's XDG portal: install `xdg-desktop-portal` and your desktop's
portal backend if native dialogs are unavailable. Windows uses the native picker.

Local Linux checks cover eight displayed feeds with nine recorders, real sidebar
drag replacement, fullscreen for an out-of-grid camera and exact grid restoration,
hidden-camera thumbnails, note edits, preset persistence/marks, displayed-frame
grabs and separate snapshot navigation. Optional scripts are in `scripts/` and are
not enabled in CI. Windows runtime/native tests remain paused at the operator's
request; Windows compilation and installer packaging remain enabled. Physical
Toughpad eight-preview performance and native picker behavior require operator checks.
Previous successful four-stream operation is not an eight-stream benchmark.
