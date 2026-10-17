# Scientific events and all-camera grabs

**Mark** (F6) records an event without taking images. **Grab All** (F7) takes
stills from every registered camera, including hidden feeds. **Mark + Grab All**
(F8) saves a mark with the frames currently displayed in the viewer. All three actions record a unique event ID, UTC Unix milliseconds,
optional note, stable camera IDs, aliases at the event, and per-camera recording
references. Touch buttons remain available at the bottom of the viewer.

The marker timestamp is taken at the operator action, before background work.
When recording, elapsed nanoseconds use the oldest active recorder's GStreamer
clock and name that recording run as the clock epoch. This survives viewer
restart while that run continues. Without an active recording clock, elapsed
nanoseconds use the viewer's monotonic clock, explicitly labeled as such. Changing
or stopping the anchor run changes the named epoch; do not compare elapsed values
across different epochs as one continuous acquisition. Named session lifetimes and manifests are independent of these clock epochs; see
[sessions](sessions.md).

## Recording file references

Recorders publish sampled GStreamer running time independently of the viewer.
The marker extrapolates that sample to the operator action using UTC; samples
older than three seconds or an action time before the sample are rejected as
gaps. Smaller UTC clock adjustments remain part of the timing uncertainty.
The segment journal contains actual splitmux open/close running times, including
keyframe-aligned boundaries. Lookup uses those boundaries, not the configured
segment interval. An event exactly on a split belongs to the following segment;
a marker beyond a closed segment with no following open boundary remains a gap
or pending reference. The offset is the marker running time minus the actual
segment open running time.

**Offsets are estimates**, not exact camera exposure times or frame numbers:
clock sampling, UTC adjustments, pipeline latency and camera buffering introduce
uncertainty. JSON retains the sample age, clock domain, run ID and journal path.
Open fragments remain pending until close. Background history refresh and viewer
restart reconcile references against the journal; missing media and unavailable
recording clocks remain explicit. No synthetic filename is invented for a gap.
Renaming a camera does not change saved IDs or file paths.

## Saving displayed frames with Mark + Grab

F8 freezes the viewer's displayed frame references at the operator action and saves
those exact pixels asynchronously. It does not call the ONVIF snapshot endpoint or
open a temporary RTSP connection. Cameras without a displayed frame remain in the
event with an explicit uncaptured result. A frozen displayed image is still saved;
its original local receipt timestamp identifies its age. The recording reference
remains anchored to the mark action, not a claimed exposure time of that image.

## Capturing hidden cameras with Grab All

Grab All tries the profile's ONVIF snapshot HTTP URI first, using the saved camera
login when required (HTTP Digest or Basic). It does not follow redirects or send
credentials to a different host. HTTP bodies are capped at 16 MiB; decoded images
are bounded to 4096×2160 and limited allocations. If snapshots are unsupported or
fail, a temporary decoder reads one fresh frame and closes before the next
camera. While recording, that decoder uses the recorder’s local encoded feed;
it never opens a second camera RTSP connection. Without an active recording, it
uses a temporary camera RTSP connection. Existing recorder pipelines are never stopped, replaced or reconfigured.

One capture job processes cameras sequentially, so at most one temporary snapshot
decoder is added to the maximum four persistent previews. RTSP extraction has an
eight-second deadline per camera. A batch stops starting cameras after 45 seconds;
an in-flight request may finish afterward. Failed/skipped cameras stay in the event
with their reason and delay. Capturing all cameras can take several seconds, and
one unresponsive camera can delay later cameras; the viewer remains responsive. Mark remains available during capture, with at most
eight pending mark jobs, so a slow grab does not prevent event timestamps.

Each successful capture stores local frame receipt UTC time, delay from the action,
method and PNG path. Camera exposure time is unavailable. Captures are asynchronous:
**they are not synchronized or frame-perfect**. Stale displayed frames are never
substituted for a failed fresh camera capture. With the source unavailable, Mark
still records the camera's recording state without waiting for image work.

## Durability, notes and exports

New events live in `session-…/events/<event-ID>/` under the acquisition output root.
Earlier settings-adjacent `operator-events` folders remain accessible in the library. Each folder
contains `event.json`, `event.csv`, and collision-free `snapshot-N.png` files.
JSON is the canonical record. CSV has one quoted row per camera, with event ID,
UTC time, elapsed clock/scope, note, run, segment, offset, recording state, image,
capture time/delay and result. Both files are written through synced temporary
files and atomic replacement. Per-event locks prevent reconciliation or note edits
from overwriting in-flight results; different events can be marked independently. If a crash leaves CSV missing or stale, history
refresh regenerates it from JSON. Original media is never deleted.

All camera entries and the mark/note are saved before image work. Results persist
incrementally. An interrupted job stays `complete: false`, retaining completed
images and pending camera entries; restart does not repeat its capture requests.
Failures saving metadata are reported; partial files remain for recovery.

Expand an event in **Show events**, choose **Edit note**, and save the corrected
note. Notes are optional and may include commas, quotes and newlines; JSON and CSV
are updated together through the same background service. Latest 200 events load
on restart, with periodic boundary reconciliation. Up to four latest thumbnails
are retained. The session library adds historical filtering and portable bundles with associated
closed recording segments; see [sessions and export](sessions.md).

## Validation

Linux checks cover 16 hidden-camera captures while 16 recording processes retain
their run IDs and continue writing; HTTP and temporary RTSP paths; split-boundary
reconciliation; camera renames; note/CSV recovery; incomplete capture persistence;
and native touch/mouse workflows at 1280×800 and 800×600. Windows checks remain
paused at the user's request. Release compilation and Windows packaging continue.
No physical Toughpad CPU/GPU measurements or synchronized-capture claim is made.
