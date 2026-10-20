# Continuous recording

## Controls and independent ownership

Auto-record is enabled by default for registered cameras at startup and hot arrival.
The persisted checkbox disables future automatic starts and leaves active recordings
running. **Record all** resumes suppression and **Stop all** prevents automatic restart
until Record all or application restart. Per-camera Record/Stop affects that camera.
The service uses the same saved policy; Ctrl+C or Linux SIGTERM drains recorder
children. Normal desktop exit also stops and finalizes all recorders. A desktop
crash leaves independent recorder processes running; reopen to reattach. Closing
an individual camera viewport never stops its recording.

Each camera owns a separate process, lock, RTSP connection, and output pipeline.
Preview selection and closing/switching its decoder never send recording stop requests.
Status shows actual file progress, errors, disk alerts, and free space independently of
ONVIF availability and preview. A live preview does not establish recording health.

## Segments and index

The managed camera recorder copies H.264 or H.265 encoded video into MKV or MP4 with no
video decoder or encoder in the recording path. Audio is deliberately excluded:
no audio track is required for acquisition, and codec negotiation/explicit audio
selection are not implemented. Choose a compatible H.264/H.265 camera profile;
unsupported video codecs fail with an operator-visible error.

Choose **Settings → Recording container**, save, then restart affected recordings.
The JSON setting is `"recording_container": "mkv"` or `"mp4"`. MKV remains the default
until real-camera and Windows-player MP4 compatibility is established. MP4 uses
one-second fragments and `first-moov-then-finalise`: a completed file receives its
final duration/index, while interrupted fragments can be recovered best effort.
The writer parser supplies AVC/HVCC codec headers without changing encoded quality.
Each run's manifest records codec and container; filename extensions match the muxer.
Older MKV sessions remain browsable and exportable. H.265 playback depends on the
player's installed codec support; Windows and VLC hardware decoding need field checks.

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

Existing run directories cannot be reused. Original media is never overwritten. Session retention is disabled by default;
explicit opt-in deletion is described in [sessions](sessions.md). `segments.jsonl` is flushed on every open/close
notification. Each record contains `event` (`opened`/`closed`), `location`,
`running_time_ns`, and `observed_unix_ms`. Pair records by location to obtain actual
start/end running times and duration. Times are GStreamer media running time;
`observed_unix_ms` is when the process observed the event, not a precise camera UTC
capture timestamp. Running time restarts for a new run. Async finalization can interleave
notifications for adjacent segments; pair by filename, not adjacent lines.

A clean stop stops the input producer, sends writer EOS, and waits for pipeline EOS
**and every pending async fragment closure**. The EOS wait has a ten-second deadline;
per-file discovery is bounded separately. Every closed journal entry is published only
after media discovery verifies a positive duration and seekability. `finalization.json`
reports `recording`, `finalizing`, `finalized`, or `incomplete/recovery-needed` and
pending filenames/errors. Manual Stop, Stop All, planned app/service exit, startup
errors and stream-loss/reconnect paths all use this cleanup. Failure remains visible
in recorder status and logs; process exit alone never proves successful finalization.

Completed fragments can be played and scrubbed while the next segment records. The
current segment is **open/incomplete** until closure is verified, even if a player can
read some frames. Do not assume full seekability while writing. Never infer the end
of a partial file from the configured interval.

After abrupt worker loss, the next worker checks that run before acquiring new footage.
Browsing **Sessions / recordings** also detects stopped orphan MKV/MP4 files, including
files absent from a damaged journal. Recovery takes the camera's acquisition lock,
skips live writers and attempts at most two unprocessed files per run per scan, with
a 30-second remux deadline per file and free-space checks. It writes a separate
`*.recovered-<UUID>.mkv`, verifies software decode/seek and positive duration, and saves
an original-file `.recovery.json` report. Originals, original segment journals and event
references stay intact. Failed repairs remain reported and aren't retried on every
refresh. Extra recovered copies consume disk space. Power loss/SIGKILL cannot send EOS;
recovery may fail or lose footage since the last complete fragment/keyframe.

Inspect local footage without camera credentials:

```sh
vidfish --inspect /path/segment.mkv
vidfish --inspect /path/segment.mkv --remux-output /path/new-copy.mp4
```

The JSON report includes codec/profile caps, dimensions, container, media duration,
seekability and a software-decoded midpoint seek result. Remux refuses an existing
destination and never re-encodes. Compare the same original and remux in VLC with
hardware acceleration enabled/disabled; container conversion alone does not prove a
decoder bug fixed.

## Green VLC playback (#58)

Field testing on a **Panasonic Toughbook CF-33** reported green video with VLC hardware
decoding enabled, and correct video with hardware decoding disabled. This points to
the selected hardware decode path; it does not establish a damaged MKV or prove MP4
fixes the symptom. Use VLC **Preferences → Input / Codecs → Hardware-accelerated
decoding → Disable**, save and reopen footage as a workaround. No affected recording
was available for inspection. Camera model/profile, VLC version and GPU/driver details
remain unconfirmed; no specific GPU or driver defect is claimed. The original failing
sample, VLC hardware on/off and Windows-player validation remain field acceptance.

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
write and index errors fail only the affected recorder. Input connection errors
first reconnect the reader while preserving the writer and file during the two-second
grace period. Recovery retries
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

Optional Linux independent-player check (requires FFmpeg/ffprobe):

```sh
cargo build --locked -p vidfish -p vidfish-media --features vidfish-media/fixtures --bin vidfish --example camera-fixture
python3 scripts/recording-playback-smoke.py --binary target/debug/vidfish --fixture target/debug/examples/camera-fixture
```

This checks real RTSP H.264/H.265 fixtures with MKV and MP4: software decode/seek,
non-green decoded pixels, duration agreement between GStreamer and ffprobe, completed
segment playback while recording continues, and SIGTERM finalization. The fixture uses
8-bit 4:2:0 video. It does not validate VLC GPU decoding or Windows players. Rust tests
also cover split/immediate stop, two-camera shutdown, orphan recovery after SIGKILL,
failed remux, live-writer exclusion and unchanged original bytes. CI remains build/package
only; Windows tests remain paused.

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

The RTSP reader and file writer are separate pipelines. A broken input connection is
recreated immediately while the existing mux and file remain open. Fresh encoded
buffers within two seconds keep the same recording run and file; a connection that
cannot recover by that deadline starts a new run. Reconnection waits for a keyframe
and maps the new RTP timestamp epoch into the existing writer clock, preserving real
time gaps and PTS/DTS ordering without decoding or re-encoding the recording. Disk, mux and index
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


## Shared viewing and input recovery evidence

The recording process publishes its encoded frames over an automatically provisioned
loopback-only feed. A per-run random identity prevents attaching to a stale endpoint;
no camera URL/login is forwarded to viewers. At most eight local clients are admitted,
with two encoded packets queued per client (at most 4 MiB per packet). The publisher
never waits for socket writes or viewer decoding. Slow clients wait for a fresh IDR
after losing their own backlog; recording retains its separate bounded writer queue.
Fresh captures of recorded cameras also use this feed, including cameras not displayed.
Transient feed endpoints/identities remain in runtime status, outside scientific logs.

`input-recovery.jsonl` in each recording run records `input_lost` and `input_recovered`
events, the current output, writer running time, observation time, last encoded-buffer
age and a safe error category. These entries preserve evidence of short connection
gaps inside the same file; they do not fabricate missing frames or exact camera-side
timestamps. `segments.jsonl` retains only actual file-open/file-close boundaries.
Repeated real TCP disconnect tests cover H.264 and H.265, including fresh RTP timestamp
epochs, unchanged output paths, playable final files and durable recovery events.
