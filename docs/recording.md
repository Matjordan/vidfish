# Continuous recording

## Controls and independent ownership

Auto-record is enabled by default for registered cameras at startup and hot arrival.
The persisted checkbox disables future automatic starts and leaves active recordings
running. **Record all** resumes suppression and **Stop all** prevents automatic restart
until Record all or application restart. Per-camera Record/Stop affects that camera.
The service uses the same saved policy; Ctrl+C drains and closes recorder children.
Closing or crashing the desktop leaves independent recorder processes running. Reopen
the desktop to reattach, or use Stop all before exiting to end acquisition.

Each camera owns a separate process, lock, RTSP connection, and output pipeline.
Preview selection and closing/switching its decoder never send recording stop requests.
Status shows actual file progress, errors, disk alerts, and free space independently of
ONVIF availability and preview. A live preview does not establish recording health.

## Segments and index

The managed camera recorder copies H.264 or H.265 encoded video into MKV with no
video decoder or encoder in the recording path. Audio is deliberately excluded:
no audio track is required for acquisition, and codec negotiation/explicit audio
selection are not implemented. Choose a compatible H.264/H.265 camera profile;
unsupported video codecs fail with an operator-visible error.

Default segments are 600 seconds. Change **Settings → Segment interval** (1–86400
seconds), Save settings, then stop/resume the affected recorders. Existing recorder
runs retain their original settings; the next run loads the saved configuration.
Splitting uses the next eligible keyframe and keeps the RTSP connection alive. Camera
encoders may ignore keyframe requests, so the segment duration can exceed the setting.

Each new run exclusively reserves a directory:

```
session-<timestamp>-<name>-<session UUID>/recordings/camera-<local UUID>-<run UUID>/
  segment-00000000.mkv
  segment-00000001.mkv
  segments.jsonl
```

Existing run directories cannot be reused. Media is not overwritten or repaired. Session retention is disabled by default;
explicit opt-in deletion is described in [sessions](sessions.md). `segments.jsonl` is flushed on every open/close
notification. Each record contains `event` (`opened`/`closed`), `location`,
`running_time_ns`, and `observed_unix_ms`. Pair records by location to obtain actual
start/end running times and duration. Times are GStreamer media running time;
`observed_unix_ms` is when the process observed the event, not a precise camera UTC
capture timestamp. Running time restarts for a new run. Async finalization can interleave
notifications for adjacent segments; pair by filename, not adjacent lines.

A clean stop sends EOS and waits up to 10 seconds for muxers. Closed fragments are
playable independently, including after the recorder is forcibly killed. An opened
fragment with no corresponding closed record is incomplete/uncertain and is retained
for recovery. Never infer its end from the configured interval. Inspect retained media
with `vidfish --inspect PATH`; playable partial media is still not marked finalized.
The app does not reconstruct or rewrite a damaged MKV. A torn last JSONL record after
power loss should be ignored while retaining earlier complete lines.

## Disk policy, stalls, and gaps

Saved defaults (bytes, not percentages):

```json
{
  "segment_seconds": 600,
  "stall_seconds": 30,
  "disk_warning_bytes": 1073741824,
  "disk_stop_bytes": 268435456,
  "disk_policy": "Stop"
}
```

The desktop edits thresholds in MiB. **Stop recording at disk stop threshold** selects
`Stop`; unchecked selects `Warn`. The stop threshold cannot exceed the warning threshold.
Each worker checks actual available space before recording and every second while active.
At the warning threshold an alert appears; Stop policy refuses a new run or finalizes
an active recording at its stop threshold. Warn policy keeps recording and reports
low space. No retention deletion happens. Space can still run out between checks,
particularly with high bitrate/many cameras: write errors remain visible, completed
segments remain intact, and the open fragment is retained.

File growth across closed and current segments proves progress. No growth for the saved
stall deadline (1–3600 seconds) fails the recorder even if a preview remains healthy.
Startup waits at most 15 seconds for writable video after profile resolution. Media,
write, index, and network errors fail only the affected recorder. Recovery retries
with bounded backoff up to 60 seconds; restoring connectivity/free space permits a
new run. Stop all or per-camera Stop suppresses retries. A preview is never used to
reset recording failure/recovery state.

Beside `config.json`, `acquisition/` contains atomic per-camera status files, per-run
worker logs, supervisor `events.jsonl`, and `<camera UUID>.events.jsonl`. The latter is
written by the independent worker and records attempt, recording_started,
recording_stopped, disk_low, and gap_started events. The next worker records
`gap_started_after_worker_loss` for an abruptly lost worker, with its last heartbeat;
the loss time is uncertain between that heartbeat and detection. A subsequent
recording_started ends the recovery gap. Errors and run IDs support joining events
with segment indexes. These records are available without the desktop; they are not
an event/session timeline UI (later epics).

## Validation

Native tests exercise H.264/H.265 keyframe segments, collision refusal, actual boundaries,
clean stop, 16 active recording processes, persisted disabled/manual policy, disk Stop
and Warn policies, silent RTSP stall/recovery, and preview switching while a peer keeps
recording. Linux injects real ENOSPC writes via `/dev/full`. A shared Windows/Linux
runtime smoke kills the owner and recorder, checks reattachment, distinct recovery runs,
gap history, preserved closed segments, and finalized 1080p video. Simulated test sources
are not a claim of 16-camera throughput on the Toughpad. Check real camera GOPs, bitrate,
filesystem, network loss, and sustained disk writes during deployment testing.


## Silent RTSP stalls

The recording worker checks encoded-video progress independently of preview and file
growth. A video interruption shorter than two seconds keeps the current recording
run and file open. Fresh encoded buffers reset the timer; no recording restart or
outage-driven file rollover occurs during that grace period. Two continuous seconds
without encoded video triggers a failed run and supervisor recovery in a new run.
Normal scheduled segment rotation still follows `segment_seconds`.

RTSP input resource errors and stream EOS are held until the two-second loss deadline;
fresh encoded buffers cancel a pending input failure. A terminal connection that
cannot resume still requires reconnection after that deadline. Disk, mux and index
errors remain immediately actionable. `stall_seconds` remains the separate file-growth
deadline (default 30 seconds, with a two-second minimum during acquisition); it no
longer sets the stream-loss timeout. RTSP TCP reads use the standard twenty-second
timeout, while the independent encoded-frame watchdog catches a silent loss sooner.
Shutdown allows up to 100 ms to send TEARDOWN, releasing limited camera sessions
without an unbounded wait when a camera is unreachable.
Recovered runs retain separate segment journals and durable gap events. No missing
footage is synthesized, and a playing preview does not imply a healthy recording.

Issue #44 was reported on H.264, 1920×1080, 30 fps, 3000 kbps, GOP 15, affecting both
preview and recording. Loopback fault tests cover H.264/H.265 silence and missing
keyframes plus recording recovery with an unaffected peer. Physical-camera recovery
and comparison against Cayenne remain operator validation; cloud fixtures do not
establish those results.
