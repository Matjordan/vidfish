//! Keyframe-aligned stream copy with a durable append-only fragment journal.
use crate::{
    BufferProgress, Codec, MediaError, Result, Source, bus_error, element, initialize,
    recording_input,
};
use gst::prelude::*;
use gstreamer as gst;
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// A brief input interruption must keep the current recording open.
pub const STREAM_LOSS_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FragmentEvent {
    pub event: String,
    pub location: PathBuf,
    /// GStreamer running time of the actual fragment boundary, not the requested interval.
    pub running_time_ns: u64,
    pub observed_unix_ms: u64,
}

pub struct SegmentedRecording {
    pipeline: gst::Pipeline,
    input: Option<gst::Pipeline>,
    input_source: Source,
    codec: Codec,
    bridge: gstreamer_app::AppSrc,
    timeline: recording_input::Timeline,
    broker: crate::shared_feed::Broker,
    retry_input: bool,
    next_input_retry: Instant,
    input_attempts: u32,
    journal: std::fs::File,
    input_journal: std::fs::File,
    current: Option<PathBuf>,
    closed_bytes: u64,
    finalized: bool,
    video_progress: BufferProgress,
    pending_input_failure: Option<(MediaError, Instant)>,
}
impl SegmentedRecording {
    /// The caller owns a unique run directory. Refuse an existing directory so even
    /// accidental run-ID reuse can never truncate scientific media.
    pub fn start(source: &Source, codec: Codec, directory: &Path, seconds: u64) -> Result<Self> {
        initialize()?;
        if seconds == 0 || seconds > u64::MAX / 1_000_000_000 {
            return Err(MediaError("invalid segment interval".into()));
        }
        std::fs::create_dir(directory).map_err(|_| {
            MediaError(
                "cannot reserve recording run directory; check collisions and permissions".into(),
            )
        })?;
        let journal = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(directory.join("segments.jsonl"))
            .map_err(|_| MediaError("cannot create segment index".into()))?;
        let input_journal = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(directory.join("input-recovery.jsonl"))
            .map_err(|_| MediaError("cannot create recording input recovery index".into()))?;
        let pipeline = gst::Pipeline::new();
        let parser = element(
            match codec {
                Codec::H264 => "h264parse",
                Codec::H265 => "h265parse",
            },
            "parser",
        )?;
        let bridge = recording_input::writer_source();
        pipeline
            .add_many([bridge.upcast_ref(), &parser])
            .map_err(|_| MediaError("cannot add persistent recording writer".into()))?;
        bridge
            .link(&parser)
            .map_err(|_| MediaError("cannot link persistent recording writer".into()))?;
        let video_progress = BufferProgress::attach(&parser, "src")?;
        let sink = element("splitmuxsink", "segments")?;
        sink.set_property(
            "location",
            directory
                .join("segment-%08d.mkv")
                .to_str()
                .ok_or_else(|| MediaError("recording path must be UTF-8".into()))?,
        );
        sink.set_property("max-size-time", seconds * 1_000_000_000);
        sink.set_property("max-size-bytes", 0u64);
        sink.set_property("async-finalize", true);
        sink.set_property("muxer-factory", "matroskamux");
        // Remote encoders may ignore requests. Boundaries are therefore observed,
        // never inferred from the nominal interval.
        sink.set_property("send-keyframe-requests", true);
        pipeline
            .add(&sink)
            .map_err(|_| MediaError("cannot add segmented output".into()))?;
        parser
            .link(&sink)
            .map_err(|_| MediaError("cannot link segmented stream-copy output".into()))?;
        let mut recording = Self {
            pipeline,
            input: None,
            input_source: source.clone(),
            broker: crate::shared_feed::Broker::start(codec)?,
            codec,
            bridge,
            timeline: std::sync::Arc::new(std::sync::Mutex::new(0)),
            retry_input: false,
            next_input_retry: Instant::now(),
            input_attempts: 0,
            journal,
            input_journal,
            current: None,
            closed_bytes: 0,
            finalized: false,
            video_progress,
            pending_input_failure: None,
        };
        recording
            .pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| MediaError("cannot start segmented recording".into()))?;
        recording.input = Some(recording_input::start(
            source,
            codec,
            &recording.pipeline,
            &recording.bridge,
            recording.timeline.clone(),
            recording.broker.publisher(),
        )?);
        Ok(recording)
    }
    fn handle(&mut self, message: &gst::Message) -> Result<bool> {
        match message.view() {
            gst::MessageView::Error(_) => return Err(bus_error(message)),
            gst::MessageView::Eos(_) => return Ok(true),
            gst::MessageView::Element(element) => {
                if let Some(s) = element.structure() {
                    let name = s.name().as_str();
                    if matches!(
                        name,
                        "splitmuxsink-fragment-opened" | "splitmuxsink-fragment-closed"
                    ) {
                        let location = s
                            .get::<String>("location")
                            .map_err(|_| MediaError("fragment location missing".into()))?;
                        let time = s
                            .get::<gst::ClockTime>("running-time")
                            .map_err(|_| MediaError("fragment timestamp missing".into()))?;
                        let opened = name.ends_with("opened");
                        let event = FragmentEvent {
                            event: if opened { "opened" } else { "closed" }.into(),
                            location: location.into(),
                            running_time_ns: time.nseconds(),
                            observed_unix_ms: SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64,
                        };
                        serde_json::to_writer(&mut self.journal, &event).map_err(|_| {
                            MediaError("cannot write segment index; check disk".into())
                        })?;
                        self.journal
                            .write_all(b"\n")
                            .and_then(|_| self.journal.sync_data())
                            .map_err(|_| {
                                MediaError("cannot persist segment index; check disk".into())
                            })?;
                        if opened {
                            self.current = Some(event.location);
                        } else {
                            self.closed_bytes += std::fs::metadata(&event.location)
                                .map_err(|_| MediaError("closed segment is unavailable".into()))?
                                .len();
                            if self.current.as_ref() == Some(&event.location) {
                                self.current = None;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(false)
    }
    fn input_event(&mut self, event: &str, reason: Option<&str>) -> Result<()> {
        let entry = serde_json::json!({
            "event": event, "output": self.current,
            "running_time_ns": self.running_time_ns(),
            "observed_unix_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
            "last_encoded_age_ms": self.video_idle().as_millis() as u64,
            "attempt": self.input_attempts, "reason": reason,
        });
        serde_json::to_writer(&mut self.input_journal, &entry)
            .map_err(|_| MediaError("cannot write input recovery index".into()))?;
        self.input_journal
            .write_all(b"\n")
            .and_then(|_| self.input_journal.sync_data())
            .map_err(|_| MediaError("cannot persist input recovery index".into()))
    }
    fn poll_input(&mut self) -> Result<()> {
        if let Some(bus) = self.input.as_ref().and_then(|p| p.bus()) {
            for _ in 0..64 {
                let Some(message) =
                    bus.pop_filtered(&[gst::MessageType::Error, gst::MessageType::Eos])
                else {
                    break;
                };
                let error = if message.type_() == gst::MessageType::Error {
                    bus_error(&message)
                } else {
                    MediaError("camera input connection ended".into())
                };
                if self.pending_input_failure.is_none() {
                    self.input_event("input_lost", Some(&error.to_string()))?;
                }
                self.pending_input_failure
                    .get_or_insert((error, Instant::now()));
                self.retry_input = true;
            }
        }
        if self.retry_input
            && self.video_idle() < STREAM_LOSS_TIMEOUT
            && Instant::now() >= self.next_input_retry
        {
            if let Some(input) = self.input.take() {
                let _ = input.set_state(gst::State::Null);
            }
            self.input_attempts = self.input_attempts.saturating_add(1);
            tracing::warn!(
                attempt = self.input_attempts,
                "reconnecting recording input while retaining writer and file"
            );
            match recording_input::start(
                &self.input_source,
                self.codec,
                &self.pipeline,
                &self.bridge,
                self.timeline.clone(),
                self.broker.publisher(),
            ) {
                Ok(input) => {
                    self.input = Some(input);
                    self.retry_input = false;
                }
                Err(error) => {
                    self.pending_input_failure = Some((error, Instant::now()));
                }
            }
            if let Some((_, observed)) = &mut self.pending_input_failure {
                *observed = Instant::now();
            }
            self.next_input_retry = Instant::now()
                + Duration::from_millis(250u64 << self.input_attempts.saturating_sub(1).min(2));
        }
        if !self.retry_input
            && self
                .pending_input_failure
                .as_ref()
                .is_some_and(|(_, observed)| self.video_idle() < observed.elapsed())
        {
            tracing::info!(
                attempts = self.input_attempts,
                "recording input recovered in existing file"
            );
            self.input_event("input_recovered", None)?;
            self.pending_input_failure = None;
            self.input_attempts = 0;
        }
        if self.video_idle() >= STREAM_LOSS_TIMEOUT
            && let Some((error, _)) = self.pending_input_failure.take()
        {
            return Err(error);
        }
        Ok(())
    }
    pub fn poll(&mut self, timeout: Duration) -> Result<()> {
        let bus = self
            .pipeline
            .bus()
            .ok_or_else(|| MediaError("recording bus unavailable".into()))?;
        let types = [
            gst::MessageType::Error,
            gst::MessageType::Eos,
            gst::MessageType::Element,
        ];
        if let Some(message) = bus.timed_pop_filtered(
            gst::ClockTime::from_nseconds(timeout.as_nanos().min(u64::MAX as u128) as u64),
            &types,
        ) && self.handle(&message)?
        {
            return Err(MediaError("recording writer ended unexpectedly".into()));
        }
        // Drain bounded pending events before observing progress.
        for _ in 0..64 {
            let Some(message) = bus.pop_filtered(&types) else {
                break;
            };
            if self.handle(&message)? {
                return Err(MediaError("recording writer ended unexpectedly".into()));
            }
        }
        self.poll_input()
    }
    pub fn wait_started(&mut self, timeout: Duration) -> Result<()> {
        let start = Instant::now();
        while self.current.is_none() || self.bytes_written()? == 0 {
            if start.elapsed() >= timeout {
                return Err(MediaError(
                    "camera supplied no writable fragment before startup deadline".into(),
                ));
            }
            self.poll(Duration::from_millis(100))?;
        }
        Ok(())
    }
    pub fn video_format(&self) -> (Option<u32>, Option<u32>, Option<f64>) {
        let caps = self
            .pipeline
            .by_name("parser")
            .and_then(|p| p.static_pad("src"))
            .and_then(|p| p.current_caps());
        let Some(s) = caps.as_ref().and_then(|c| c.structure(0)) else {
            return (None, None, None);
        };
        let width = s
            .get::<i32>("width")
            .ok()
            .filter(|n| *n > 0)
            .map(|n| n as u32);
        let height = s
            .get::<i32>("height")
            .ok()
            .filter(|n| *n > 0)
            .map(|n| n as u32);
        let fps = s
            .get::<gst::Fraction>("framerate")
            .ok()
            .filter(|f| f.numer() > 0)
            .map(|f| f.numer() as f64 / f.denom() as f64);
        (width, height, fps)
    }
    /// Clock running time shares the splitmux fragment timestamp domain.
    pub fn running_time_ns(&self) -> Option<u64> {
        self.pipeline.current_running_time().map(|t| t.nseconds())
    }
    pub fn current_output(&self) -> Option<PathBuf> {
        self.current.clone()
    }
    pub fn shared_feed(&self) -> crate::SharedFeed {
        self.broker.descriptor()
    }
    pub fn video_idle(&self) -> Duration {
        self.video_progress.idle()
    }
    pub fn bytes_written(&self) -> Result<u64> {
        Ok(self.closed_bytes
            + match &self.current {
                Some(path) => std::fs::metadata(path)
                    .map_err(|_| MediaError("recording file progress unavailable".into()))?
                    .len(),
                None => 0,
            })
    }
    pub fn finish(mut self) -> Result<u64> {
        self.bridge
            .end_of_stream()
            .map_err(|_| MediaError("cannot finalize recording writer".into()))?;
        if let Some(input) = self.input.take() {
            let _ = input.set_state(gst::State::Null);
        }
        let bus = self
            .pipeline
            .bus()
            .ok_or_else(|| MediaError("recording bus unavailable".into()))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if Instant::now() >= deadline {
                return Err(MediaError(
                    "segment finalization timed out; open fragment retained for recovery".into(),
                ));
            }
            if let Some(message) = bus.timed_pop_filtered(
                gst::ClockTime::from_mseconds(100),
                &[
                    gst::MessageType::Error,
                    gst::MessageType::Eos,
                    gst::MessageType::Element,
                ],
            ) && self.handle(&message)?
            {
                break;
            }
        }
        self.pipeline
            .set_state(gst::State::Null)
            .map_err(|_| MediaError("cannot stop segmented recording".into()))?;
        self.finalized = true;
        self.journal
            .sync_all()
            .map_err(|_| MediaError("cannot finalize segment index".into()))?;
        self.bytes_written()
    }
}
impl Drop for SegmentedRecording {
    fn drop(&mut self) {
        if !self.finalized {
            let _ = self.pipeline.set_state(gst::State::Null);
            if let Some(input) = self.input.take() {
                let _ = input.set_state(gst::State::Null);
            }
        }
    }
}

#[cfg(all(test, feature = "fixtures"))]
mod input_grace_tests {
    use super::*;

    #[test]
    fn transient_input_error_preserves_file_but_output_error_is_immediate() {
        let camera = crate::fixtures::RtspFixture::start(320, 240).unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut recording = SegmentedRecording::start(
            &Source::Rtsp(camera.uri.clone()),
            Codec::H264,
            &root.path().join("run"),
            60,
        )
        .unwrap();
        recording.wait_started(Duration::from_secs(10)).unwrap();
        let original = recording.current_output();
        camera.disconnect_client(0);
        let until = Instant::now() + Duration::from_secs(2);
        while camera.client_count() < 2 || recording.pending_input_failure.is_some() {
            recording.poll(Duration::from_millis(20)).unwrap();
            assert!(
                Instant::now() < until,
                "RTSP input did not recover inside grace period"
            );
        }
        assert_eq!(recording.current_output(), original);
        let output = recording.pipeline.by_name("segments").unwrap();
        recording
            .pipeline
            .post_message(
                gst::message::Error::builder(gst::ResourceError::Write, "output fault")
                    .src(&output)
                    .build(),
            )
            .unwrap();
        let until = Instant::now() + Duration::from_secs(1);
        loop {
            if let Err(error) = recording.poll(Duration::from_millis(20)) {
                assert!(error.to_string().contains("Write"));
                break;
            }
            assert!(
                Instant::now() < until,
                "output failure was hidden by stream grace"
            );
        }
    }
}
