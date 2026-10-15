//! GStreamer spike: independent stream-copy recording and bounded CPU preview upload.
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_pbutils::prelude::DiscovererStreamInfoExt;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

pub mod segmented;
pub use segmented::SegmentedRecording;

#[cfg(feature = "fixtures")]
pub mod fixtures;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264,
    H265,
}

#[derive(Clone)]
pub enum Source {
    Rtsp(String),
    AuthenticatedRtsp {
        uri: String,
        username: zeroize::Zeroizing<String>,
        password: zeroize::Zeroizing<String>,
    },
    /// Test-only local source. Encoding happens only in this simulated source.
    TestPattern {
        width: i32,
        height: i32,
    },
}

#[derive(Debug)]
pub struct MediaError(pub String);
impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for MediaError {}
type Result<T> = std::result::Result<T, MediaError>;

pub fn initialize() -> Result<()> {
    gst::init().map_err(|_| {
        MediaError("GStreamer initialization failed; check runtime installation".into())
    })
}

pub fn check_plugins() -> Result<Vec<&'static str>> {
    check_plugins_for(Codec::H264)
}
pub fn check_plugins_for(codec: Codec) -> Result<Vec<&'static str>> {
    initialize()?;
    let required = [
        "rtspsrc",
        match codec {
            Codec::H264 => "rtph264depay",
            Codec::H265 => "rtph265depay",
        },
        match codec {
            Codec::H264 => "h264parse",
            Codec::H265 => "h265parse",
        },
        match codec {
            Codec::H264 => "avdec_h264",
            Codec::H265 => "avdec_h265",
        },
        "matroskamux",
        "splitmuxsink",
        "filesink",
        "decodebin",
        "videoconvert",
        "appsink",
    ];
    let missing: Vec<_> = required
        .iter()
        .filter(|name| gst::ElementFactory::find(name).is_none())
        .copied()
        .collect();
    if !missing.is_empty() {
        return Err(MediaError(format!(
            "missing GStreamer plugins: {}",
            missing.join(", ")
        )));
    }
    Ok(required.to_vec())
}

fn element(factory: &str, name: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(factory)
        .name(name)
        .build()
        .map_err(|_| MediaError(format!("missing or unusable GStreamer element {factory}")))
}

/// Monotonic buffer progress, independent of pipeline clock and file buffering.
#[derive(Clone)]
struct BufferProgress {
    start: Instant,
    millis: Arc<AtomicU64>,
}
impl BufferProgress {
    fn attach(element: &gst::Element, pad: &str) -> Result<Self> {
        let progress = Self {
            start: Instant::now(),
            millis: Arc::new(AtomicU64::new(0)),
        };
        let observed = progress.clone();
        element
            .static_pad(pad)
            .ok_or_else(|| MediaError("progress pad unavailable".into()))?
            .add_probe(
                gst::PadProbeType::BUFFER | gst::PadProbeType::BUFFER_LIST,
                move |_, _| {
                    observed.millis.store(
                        observed.start.elapsed().as_millis() as u64,
                        Ordering::Relaxed,
                    );
                    gst::PadProbeReturn::Ok
                },
            );
        Ok(progress)
    }
    fn idle(&self) -> Duration {
        self.start
            .elapsed()
            .saturating_sub(Duration::from_millis(self.millis.load(Ordering::Relaxed)))
    }
}

fn encoded_source(pipeline: &gst::Pipeline, source: &Source, codec: Codec) -> Result<gst::Element> {
    let parser = element(
        match codec {
            Codec::H264 => "h264parse",
            Codec::H265 => "h265parse",
        },
        "parser",
    )?;
    pipeline
        .add(&parser)
        .map_err(|_| MediaError("cannot add parser".into()))?;
    match source {
        Source::Rtsp(uri) | Source::AuthenticatedRtsp { uri, .. } => {
            if !url::Url::parse(uri).is_ok_and(|parsed| {
                matches!(parsed.scheme(), "rtsp" | "rtsps") && parsed.host_str().is_some()
            }) {
                return Err(MediaError(
                    "expected a valid rtsp:// or rtsps:// camera URI".into(),
                ));
            }
            let rtsp = element("rtspsrc", "camera")?;
            rtsp.set_property("location", uri);
            if let Source::AuthenticatedRtsp {
                username, password, ..
            } = source
            {
                rtsp.set_property("user-id", username.as_str());
                rtsp.set_property("user-pw", password.as_str());
            }
            rtsp.set_property_from_str("protocols", "tcp");
            rtsp.set_property("latency", 200u32);
            rtsp.set_property("tcp-timeout", 5_000_000u64);
            // A dead camera must not delay watchdog recovery waiting for TEARDOWN.
            rtsp.set_property("teardown-timeout", 0u64);
            let depay = element(
                match codec {
                    Codec::H264 => "rtph264depay",
                    Codec::H265 => "rtph265depay",
                },
                "depay",
            )?;
            pipeline
                .add_many([&rtsp, &depay])
                .map_err(|_| MediaError("cannot add RTSP elements".into()))?;
            depay
                .link(&parser)
                .map_err(|_| MediaError("cannot link depayloader and parser".into()))?;
            let sink = depay
                .static_pad("sink")
                .ok_or_else(|| MediaError("depayloader has no sink pad".into()))?;
            let encoding = match codec {
                Codec::H264 => "H264",
                Codec::H265 => "H265",
            };
            rtsp.connect_pad_added(move |_, pad| {
                let caps = pad.current_caps().unwrap_or_else(|| pad.query_caps(None));
                let selected = caps.structure(0).is_some_and(|s| {
                    s.get::<&str>("media") == Ok("video")
                        && s.get::<&str>("encoding-name") == Ok(encoding)
                });
                if selected && !sink.is_linked() {
                    let _ = pad.link(&sink);
                }
            });
        }
        Source::TestPattern { width, height } => {
            let test = element("videotestsrc", "fixture")?;
            test.set_property("is-live", true);
            let filter = element("capsfilter", "fixture-caps")?;
            filter.set_property(
                "caps",
                gst::Caps::builder("video/x-raw")
                    .field("width", *width)
                    .field("height", *height)
                    .field("framerate", gst::Fraction::new(30, 1))
                    .build(),
            );
            let encoder = element(
                match codec {
                    Codec::H264 => "x264enc",
                    Codec::H265 => "x265enc",
                },
                "fixture-encoder",
            )?;
            encoder.set_property_from_str("key-int-max", "30");
            if codec == Codec::H265 {
                encoder.set_property("option-string", "pools=1:frame-threads=1");
            }
            encoder.set_property_from_str("tune", "zerolatency");
            encoder.set_property_from_str("speed-preset", "ultrafast");
            pipeline
                .add_many([&test, &filter, &encoder])
                .map_err(|_| MediaError("cannot add test source".into()))?;
            gst::Element::link_many([&test, &filter, &encoder, &parser])
                .map_err(|_| MediaError("cannot link test source".into()))?;
        }
    }
    Ok(parser)
}

/// No decoding or encoding occurs on the RTSP recording path.
pub struct Recording {
    pipeline: gst::Pipeline,
    finalized: bool,
    received_video: Arc<AtomicBool>,
}
impl Recording {
    pub fn start(source: &Source, codec: Codec, output: &Path) -> Result<Self> {
        initialize()?;
        // Reserve exclusively: an existing scientific recording is never overwritten.
        let reservation = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)
            .map_err(|error| MediaError(format!("cannot reserve output file: {error}")))?;
        drop(reservation);
        let result = (|| {
            let pipeline = gst::Pipeline::new();
            let parser = encoded_source(&pipeline, source, codec)?;
            let received_video = Arc::new(AtomicBool::new(false));
            let received = received_video.clone();
            parser
                .static_pad("src")
                .ok_or_else(|| MediaError("parser source pad missing".into()))?
                .add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                    received.store(true, Ordering::Release);
                    gst::PadProbeReturn::Remove
                });
            let mux = element("matroskamux", "mux")?;
            let file = element("filesink", "file")?;
            file.set_property(
                "location",
                output
                    .to_str()
                    .ok_or_else(|| MediaError("output path must be UTF-8".into()))?,
            );
            pipeline
                .add_many([&mux, &file])
                .map_err(|_| MediaError("cannot add file output".into()))?;
            gst::Element::link_many([&parser, &mux, &file])
                .map_err(|_| MediaError("cannot link recording output".into()))?;
            pipeline
                .set_state(gst::State::Playing)
                .map_err(|_| MediaError("cannot start recording pipeline".into()))?;
            Ok(Self {
                pipeline,
                finalized: false,
                received_video,
            })
        })();
        if result.is_err() && std::fs::metadata(output).is_ok_and(|metadata| metadata.len() == 0) {
            // Clean up only the empty reservation. Preserve any partial media
            // written during a failed state transition for later recovery.
            let _ = std::fs::remove_file(output);
        }
        result
    }

    pub fn poll_error(&self, timeout: Duration) -> Result<()> {
        let bus = self
            .pipeline
            .bus()
            .ok_or_else(|| MediaError("recording bus unavailable".into()))?;
        if let Some(message) = bus.timed_pop_filtered(
            gst::ClockTime::from_nseconds(timeout.as_nanos() as u64),
            &[gst::MessageType::Error],
        ) {
            return Err(bus_error(&message));
        }
        Ok(())
    }

    /// A Playing state alone does not prove camera video has arrived.
    pub fn wait_started(&self, timeout: Duration) -> Result<()> {
        let started = Instant::now();
        while !self.received_video.load(Ordering::Acquire) {
            if started.elapsed() >= timeout {
                return Err(MediaError("camera supplied no encoded video before the startup deadline; check profile and connectivity".into()));
            }
            self.poll_error(Duration::from_millis(100))?;
        }
        Ok(())
    }

    /// EOS drains the muxer before transitioning to Null, with a bounded wait.
    pub fn finish(mut self) -> Result<()> {
        self.pipeline.send_event(gst::event::Eos::new());
        let bus = self
            .pipeline
            .bus()
            .ok_or_else(|| MediaError("recording bus unavailable".into()))?;
        let message = bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(10),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        );
        self.pipeline
            .set_state(gst::State::Null)
            .map_err(|_| MediaError("cannot stop recording".into()))?;
        self.finalized = true;
        match message {
            Some(message) if message.type_() == gst::MessageType::Eos => Ok(()),
            Some(message) => Err(bus_error(&message)),
            None => Err(MediaError(
                "recording finalization timed out; media may be incomplete".into(),
            )),
        }
    }
}
impl Drop for Recording {
    fn drop(&mut self) {
        if !self.finalized {
            let _ = self.pipeline.set_state(gst::State::Null);
        }
    }
}

fn bus_error(message: &gst::Message) -> MediaError {
    // GStreamer error/debug text can contain URI credentials. Never expose it.
    let source = message
        .src()
        .map(|s| s.name().to_string())
        .unwrap_or_else(|| "unknown".into());
    MediaError(format!(
        "media pipeline error at {source}; check URI, codec, network, plugins, and output permissions"
    ))
}

#[derive(Clone)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
    pub number: u64,
    pub received_at: Instant,
}

static PREVIEW_WORKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static PREVIEW_PEAK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub fn preview_worker_counts() -> (usize, usize) {
    (
        PREVIEW_WORKERS.load(Ordering::Acquire),
        PREVIEW_PEAK.load(Ordering::Acquire),
    )
}
struct PreviewGuard;
impl PreviewGuard {
    fn enter() -> Self {
        let active = PREVIEW_WORKERS.fetch_add(1, Ordering::AcqRel) + 1;
        PREVIEW_PEAK.fetch_max(active, Ordering::AcqRel);
        Self
    }
}
impl Drop for PreviewGuard {
    fn drop(&mut self) {
        PREVIEW_WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct Preview {
    pub frames: tokio::sync::watch::Receiver<Option<Arc<Frame>>>,
    pub status: tokio::sync::watch::Receiver<String>,
    stopping: Arc<AtomicBool>,
    retry: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Preview {
    pub fn start(source: Source, codec: Codec) -> Self {
        let seconds = std::env::var("VIDFISH_PREVIEW_STALL_SECONDS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|s| (1..=60).contains(s))
            .unwrap_or(5);
        Self::start_with_stall_timeout(source, codec, Duration::from_secs(seconds))
    }
    /// Configure the decoded-frame watchdog independently of recording policy.
    pub fn start_with_stall_timeout(source: Source, codec: Codec, timeout: Duration) -> Self {
        let timeout = timeout.clamp(Duration::from_millis(100), Duration::from_secs(60));
        let (frames_tx, frames) = tokio::sync::watch::channel(None);
        let (status_tx, status) = tokio::sync::watch::channel("Connecting".into());
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = stopping.clone();
        let retry = Arc::new(AtomicBool::new(false));
        let retry_worker = retry.clone();
        let worker = std::thread::spawn(move || {
            let _active = PreviewGuard::enter();
            let recovery = PreviewRecovery {
                timeout,
                retry: retry_worker,
                healthy: Arc::new(AtomicBool::new(false)),
            };
            let mut failures = 0u32;
            while !stop.load(Ordering::Acquire) {
                recovery.retry.store(false, Ordering::Release);
                recovery.healthy.store(false, Ordering::Release);
                let result = preview_worker(
                    source.clone(),
                    codec,
                    stop.clone(),
                    frames_tx.clone(),
                    &status_tx,
                    None,
                    Some(&recovery),
                );
                if stop.load(Ordering::Acquire) {
                    break;
                }
                if recovery.healthy.load(Ordering::Acquire) {
                    failures = 0;
                }
                let reason = result
                    .err()
                    .map_or_else(|| "stream ended".into(), |e| e.to_string());
                tracing::warn!(%reason, retry = failures.saturating_add(1), "preview reconnecting");
                status_tx.send_replace(format!("Reconnecting · {reason}"));
                let jitter = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .subsec_millis() as u64
                    % 251;
                let until =
                    Instant::now() + Duration::from_millis((250u64 << failures.min(4)) + jitter);
                failures = failures.saturating_add(1);
                while Instant::now() < until && !stop.load(Ordering::Acquire) {
                    if recovery.retry.swap(false, Ordering::AcqRel) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            status_tx.send_replace("Stopped".into());
        });
        Self {
            frames,
            status,
            stopping,
            retry,
            worker: Some(worker),
        }
    }
    pub fn retry_now(&self) {
        self.retry.store(true, Ordering::Release);
    }
    pub fn request_stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }
    pub fn join(mut self) {
        self.request_stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for Preview {
    fn drop(&mut self) {
        self.request_stop();
    }
}

/// Extract a single fresh frame with a temporary decoder, stopped before returning.
/// Callers bound concurrent calls separately from the four persistent previews.
pub fn snapshot(source: Source, codec: Codec, timeout: Duration) -> Result<Arc<Frame>> {
    let (sender, frames) = tokio::sync::watch::channel(None);
    let (status, _) = tokio::sync::watch::channel(String::new());
    preview_worker(
        source,
        codec,
        Arc::new(AtomicBool::new(false)),
        sender,
        &status,
        Some(Instant::now() + timeout),
        None,
    )?;
    let result = frames.borrow().clone();
    result.ok_or_else(|| MediaError("Snapshot deadline reached without a frame".into()))
}
struct PreviewRecovery {
    timeout: Duration,
    retry: Arc<AtomicBool>,
    healthy: Arc<AtomicBool>,
}

fn preview_worker(
    source: Source,
    codec: Codec,
    stop: Arc<AtomicBool>,
    frames: tokio::sync::watch::Sender<Option<Arc<Frame>>>,
    status: &tokio::sync::watch::Sender<String>,
    deadline: Option<Instant>,
    recovery: Option<&PreviewRecovery>,
) -> Result<()> {
    initialize()?;
    let pipeline = gst::Pipeline::new();
    let parser = encoded_source(&pipeline, &source, codec)?;
    // These policies are preview-only: recording stays a separate stream-copy pipeline.
    if let Some(depay) = pipeline.by_name("depay") {
        for property in ["request-keyframe", "wait-for-keyframe"] {
            if depay.find_property(property).is_some() {
                depay.set_property(property, true);
            }
        }
    }
    let encoded = BufferProgress::attach(&parser, "src")?;
    let ingress = pipeline
        .by_name("depay")
        .map(|p| BufferProgress::attach(&p, "sink"))
        .transpose()?;
    let decoder = element("decodebin", "decoder")?;
    decoder
        .clone()
        .downcast::<gst::Bin>()
        .map_err(|_| MediaError("decoder is not a bin".into()))?
        .connect_element_added(|_, child| {
            if let Some(factory) = child.factory()
                && factory
                    .metadata("klass")
                    .is_some_and(|klass| klass.contains("Decoder"))
            {
                tracing::info!(decoder = %factory.name(), "preview decoder selected");
            }
        });
    let convert = element("videoconvert", "convert")?;
    let sink = gstreamer_app::AppSink::builder()
        .name("preview")
        .caps(
            &gst::Caps::builder("video/x-raw")
                .field("format", "RGB")
                .build(),
        )
        .max_buffers(1)
        .drop(true)
        .sync(false)
        .build();
    pipeline
        .add_many([&decoder, &convert, sink.upcast_ref()])
        .map_err(|_| MediaError("cannot add preview elements".into()))?;
    parser
        .link(&decoder)
        .map_err(|_| MediaError("cannot link decoder".into()))?;
    convert
        .link(&sink)
        .map_err(|_| MediaError("cannot link preview sink".into()))?;
    let pad = convert
        .static_pad("sink")
        .ok_or_else(|| MediaError("convert sink pad missing".into()))?;
    decoder.connect_pad_added(move |_, source_pad| {
        if !pad.is_linked() {
            let _ = source_pad.link(&pad);
        }
    });
    let mut number = frames.borrow().as_ref().map_or(0, |f| f.number);
    let mut first_frame = None;
    let healthy = recovery.map(|r| r.healthy.clone());
    let observed = frames.clone();
    let started = Instant::now();
    let frame_status = status.clone();
    let sample_stop = stop.clone();
    sink.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                use gstreamer_video::VideoFrameExt;
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let caps = sample.caps().ok_or(gst::FlowError::Error)?;
                let info = gstreamer_video::VideoInfo::from_caps(caps)
                    .map_err(|_| gst::FlowError::Error)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let mapped =
                    gstreamer_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info)
                        .map_err(|_| gst::FlowError::Error)?;
                let data = mapped.plane_data(0).map_err(|_| gst::FlowError::Error)?;
                let width = info.width() as usize;
                let height = info.height() as usize;
                let rgb = pack_rgb(data, width, height, mapped.plane_stride()[0])?;
                if first_frame.is_none() {
                    first_frame = Some(Instant::now());
                    let rate = info.fps();
                    tracing::info!(
                        width,
                        height,
                        nominal_fps = rate.numer() as f64 / rate.denom() as f64,
                        "preview negotiated"
                    );
                    frame_status.send_replace("Playing".into());
                }
                if first_frame.is_some_and(|t| t.elapsed() >= Duration::from_secs(10))
                    && let Some(healthy) = &healthy
                {
                    healthy.store(true, Ordering::Release);
                }
                number += 1;
                frames.send_replace(Some(Arc::new(Frame {
                    width,
                    height,
                    rgb,
                    number,
                    received_at: Instant::now(),
                })));
                if deadline.is_some() {
                    sample_stop.store(true, Ordering::Release);
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    status.send_replace("Waiting for frames".into());
    if pipeline.set_state(gst::State::Playing).is_err() {
        let _ = pipeline.set_state(gst::State::Null);
        return Err(MediaError("cannot start preview".into()));
    }
    let bus = pipeline
        .bus()
        .ok_or_else(|| MediaError("preview bus unavailable".into()))?;
    let mut result = Ok(());
    while !stop.load(Ordering::Acquire) && deadline.is_none_or(|d| Instant::now() < d) {
        if let Some(recovery) = recovery {
            if recovery.retry.load(Ordering::Acquire) {
                break;
            }
            let last = observed
                .borrow()
                .as_ref()
                .map(|f| f.received_at)
                .filter(|t| *t >= started);
            let stalled = last.map_or_else(
                || started.elapsed() >= (recovery.timeout * 3).max(Duration::from_secs(3)),
                |last| last.elapsed() >= recovery.timeout,
            );
            if stalled {
                tracing::warn!(
                    decoded_idle_ms = last.map(|t| t.elapsed().as_millis() as u64),
                    attempt_ms = started.elapsed().as_millis() as u64,
                    encoded_idle_ms = encoded.idle().as_millis() as u64,
                    rtp_idle_ms = ingress.as_ref().map(|p| p.idle().as_millis() as u64),
                    "decoded-frame watchdog expired"
                );
                result = Err(MediaError(
                    "decoded frames stalled; rebuilding preview".into(),
                ));
                break;
            }
        }
        if let Some(message) = bus.timed_pop_filtered(
            gst::ClockTime::from_mseconds(100),
            &[
                gst::MessageType::Error,
                gst::MessageType::Eos,
                gst::MessageType::Warning,
            ],
        ) {
            if message.type_() == gst::MessageType::Warning {
                // GStreamer error/debug strings can contain authenticated camera URIs.
                tracing::warn!(stage = %message.src().map(|s| s.name().to_string()).unwrap_or_default(), "preview pipeline warning (details redacted)");
                continue;
            }
            if message.type_() == gst::MessageType::Error {
                result = Err(bus_error(&message));
            }
            break;
        }
    }
    let _ = pipeline.set_state(gst::State::Null);
    result
}

fn pack_rgb(
    data: &[u8],
    width: usize,
    height: usize,
    stride: i32,
) -> std::result::Result<Vec<u8>, gst::FlowError> {
    if width == 0 || height == 0 || width > 4096 || height > 2160 || stride <= 0 {
        return Err(gst::FlowError::Error);
    }
    let stride = stride as usize;
    if stride < width * 3 || data.len() < (height - 1) * stride + width * 3 {
        return Err(gst::FlowError::Error);
    }
    let mut rgb = Vec::with_capacity(width * height * 3);
    for row in 0..height {
        rgb.extend_from_slice(&data[row * stride..row * stride + width * 3]);
    }
    Ok(rgb)
}

#[cfg(test)]
mod frame_tests {
    #[test]
    fn frame_packing_removes_padding_and_rejects_invalid_layouts() {
        assert_eq!(
            super::pack_rgb(&[1, 2, 3, 99, 4, 5, 6], 1, 2, 4).unwrap(),
            [1, 2, 3, 4, 5, 6]
        );
        for (width, height, stride) in [
            (0, 1, 4),
            (1, 0, 4),
            (1, 1, -1),
            (2, 1, 4),
            (4097, 1, 4),
            (1, 2161, 4),
            (1, 3, 4),
        ] {
            assert!(super::pack_rgb(&[0; 7], width, height, stride).is_err());
        }
    }
}

/// Bounded headless prototype used by CI and benchmark harnesses.
pub fn record_for(source: &Source, codec: Codec, output: &Path, duration: Duration) -> Result<()> {
    let recording = Recording::start(source, codec, output)?;
    recording.wait_started(Duration::from_secs(15))?;
    let start = Instant::now();
    while start.elapsed() < duration {
        recording.poll_error(Duration::from_millis(100))?;
    }
    recording.finish()
}

#[derive(Debug)]
pub struct RecordingInfo {
    pub width: u32,
    pub height: u32,
    pub duration_seconds: f64,
    pub caps: String,
}

/// Inspect a finalized file using GStreamer's bounded media discoverer.
pub fn inspect_recording(path: &Path) -> Result<RecordingInfo> {
    initialize()?;
    let absolute = std::fs::canonicalize(path)
        .map_err(|error| MediaError(format!("cannot open recording: {error}")))?;
    let uri = url::Url::from_file_path(&absolute)
        .map_err(|_| MediaError("cannot represent recording path as a file URI".into()))?;
    let discoverer = gstreamer_pbutils::Discoverer::new(gst::ClockTime::from_seconds(10))
        .map_err(|_| MediaError("cannot initialize media inspector".into()))?;
    let info = discoverer.discover_uri(uri.as_str()).map_err(|_| {
        MediaError("recording cannot be decoded; check file and decoder plugins".into())
    })?;
    if info.result() != gstreamer_pbutils::DiscovererResult::Ok {
        return Err(MediaError(
            "recording is incomplete or has missing decoder plugins".into(),
        ));
    }
    let videos = info.video_streams();
    let video = videos
        .first()
        .ok_or_else(|| MediaError("recording has no video stream".into()))?;
    Ok(RecordingInfo {
        width: video.width(),
        height: video.height(),
        duration_seconds: info
            .duration()
            .ok_or_else(|| MediaError("recording duration unavailable".into()))?
            .nseconds() as f64
            / 1e9,
        caps: video
            .caps()
            .map(|caps| caps.to_string())
            .unwrap_or_default(),
    })
}

#[cfg(all(test, unix))]
mod write_failure_tests {
    use super::*;
    #[test]
    fn full_device_reports_media_error_without_disrupting_peer() {
        initialize().unwrap();
        let root = tempfile::tempdir().unwrap();
        let source = Source::TestPattern {
            width: 320,
            height: 240,
        };
        let peer = Recording::start(&source, Codec::H264, &root.path().join("peer.mkv")).unwrap();
        peer.wait_started(Duration::from_secs(15)).unwrap();
        // /dev/full returns ENOSPC on actual writes without filling the runner disk.
        let pipeline = gst::Pipeline::new();
        let parser = encoded_source(&pipeline, &source, Codec::H264).unwrap();
        let mux = element("matroskamux", "mux").unwrap();
        let sink = element("filesink", "full-device").unwrap();
        sink.set_property("location", "/dev/full");
        pipeline.add_many([&mux, &sink]).unwrap();
        gst::Element::link_many([&parser, &mux, &sink]).unwrap();
        let _ = pipeline.set_state(gst::State::Playing);
        let bus = pipeline.bus().unwrap();
        let error = bus
            .timed_pop_filtered(gst::ClockTime::from_seconds(10), &[gst::MessageType::Error])
            .expect("full disk did not surface an error");
        assert!(bus_error(&error).to_string().contains("full-device"));
        pipeline.set_state(gst::State::Null).unwrap();
        peer.poll_error(Duration::from_millis(500)).unwrap();
        peer.finish().unwrap();
        inspect_recording(&root.path().join("peer.mkv")).unwrap();
    }
}
