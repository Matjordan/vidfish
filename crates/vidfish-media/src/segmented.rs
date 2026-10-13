//! Keyframe-aligned stream copy with a durable append-only fragment journal.
use crate::{Codec, MediaError, Result, Source, bus_error, element, encoded_source, initialize};
use gst::prelude::*;
use gstreamer as gst;
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

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
    journal: std::fs::File,
    current: Option<PathBuf>,
    closed_bytes: u64,
    finalized: bool,
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
        let pipeline = gst::Pipeline::new();
        let parser = encoded_source(&pipeline, source, codec)?;
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
        let recording = Self {
            pipeline,
            journal,
            current: None,
            closed_bytes: 0,
            finalized: false,
        };
        recording
            .pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| MediaError("cannot start segmented recording".into()))?;
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
    pub fn poll(&mut self, timeout: Duration) -> Result<()> {
        let bus = self
            .pipeline
            .bus()
            .ok_or_else(|| MediaError("recording bus unavailable".into()))?;
        if let Some(message) = bus.timed_pop_filtered(
            gst::ClockTime::from_nseconds(timeout.as_nanos().min(u64::MAX as u128) as u64),
            &[
                gst::MessageType::Error,
                gst::MessageType::Eos,
                gst::MessageType::Element,
            ],
        ) && self.handle(&message)?
        {
            return Err(MediaError("recording stream ended unexpectedly".into()));
        }
        // Drain bounded pending events before observing progress.
        for _ in 0..64 {
            let Some(message) = bus.pop_filtered(&[
                gst::MessageType::Error,
                gst::MessageType::Eos,
                gst::MessageType::Element,
            ]) else {
                break;
            };
            if self.handle(&message)? {
                return Err(MediaError("recording stream ended unexpectedly".into()));
            }
        }
        Ok(())
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
        self.pipeline.send_event(gst::event::Eos::new());
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
        }
    }
}
