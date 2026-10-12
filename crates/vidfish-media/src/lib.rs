//! GStreamer spike: independent stream-copy recording and bounded CPU preview upload.
use gst::prelude::*;
use gstreamer as gst;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

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
    initialize()?;
    let required = [
        "rtspsrc",
        "rtph264depay",
        "h264parse",
        "matroskamux",
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
        Source::Rtsp(uri) => {
            if !url::Url::parse(uri).is_ok_and(|parsed| {
                matches!(parsed.scheme(), "rtsp" | "rtsps") && parsed.host_str().is_some()
            }) {
                return Err(MediaError(
                    "expected a valid rtsp:// or rtsps:// camera URI".into(),
                ));
            }
            let rtsp = element("rtspsrc", "camera")?;
            rtsp.set_property("location", uri);
            rtsp.set_property_from_str("protocols", "tcp");
            rtsp.set_property("latency", 200u32);
            rtsp.set_property("tcp-timeout", 5_000_000u64);
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
            })
        })();
        if result.is_err() {
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
}

pub struct Preview {
    pub frames: tokio::sync::watch::Receiver<Option<Arc<Frame>>>,
    pub status: tokio::sync::watch::Receiver<String>,
    stopping: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Preview {
    pub fn start(source: Source, codec: Codec) -> Self {
        let (frames_tx, frames) = tokio::sync::watch::channel(None);
        let (status_tx, status) = tokio::sync::watch::channel("Connecting".into());
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = stopping.clone();
        let worker = std::thread::spawn(move || {
            let result = preview_worker(source, codec, stop, frames_tx, &status_tx);
            status_tx.send_replace(match result {
                Ok(()) => "Stopped".into(),
                Err(error) => error.to_string(),
            });
        });
        Self {
            frames,
            status,
            stopping,
            worker: Some(worker),
        }
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

fn preview_worker(
    source: Source,
    codec: Codec,
    stop: Arc<AtomicBool>,
    frames: tokio::sync::watch::Sender<Option<Arc<Frame>>>,
    status: &tokio::sync::watch::Sender<String>,
) -> Result<()> {
    initialize()?;
    let pipeline = gst::Pipeline::new();
    let parser = encoded_source(&pipeline, &source, codec)?;
    let decoder = element("decodebin", "decoder")?;
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
    let mut number = 0u64;
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
                let stride = mapped.plane_stride()[0] as usize;
                // Reject extreme camera dimensions before allocation.
                if width > 4096 || height > 2160 || stride < width * 3 {
                    return Err(gst::FlowError::Error);
                }
                let mut rgb = Vec::with_capacity(width * height * 3);
                for row in data.chunks(stride).take(height) {
                    rgb.extend_from_slice(&row[..width * 3]);
                }
                number += 1;
                frames.send_replace(Some(Arc::new(Frame {
                    width,
                    height,
                    rgb,
                    number,
                })));
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|_| MediaError("cannot start preview".into()))?;
    status.send_replace("Playing".into());
    let bus = pipeline
        .bus()
        .ok_or_else(|| MediaError("preview bus unavailable".into()))?;
    let mut result = Ok(());
    while !stop.load(Ordering::Acquire) {
        if let Some(message) = bus.timed_pop_filtered(
            gst::ClockTime::from_mseconds(100),
            &[gst::MessageType::Error, gst::MessageType::Eos],
        ) {
            if message.type_() == gst::MessageType::Error {
                result = Err(bus_error(&message));
            }
            break;
        }
    }
    let _ = pipeline.set_state(gst::State::Null);
    result
}

/// Bounded headless prototype used by CI and benchmark harnesses.
pub fn record_for(source: &Source, codec: Codec, output: &Path, duration: Duration) -> Result<()> {
    let recording = Recording::start(source, codec, output)?;
    let start = Instant::now();
    while start.elapsed() < duration {
        recording.poll_error(Duration::from_millis(100))?;
    }
    recording.finish()
}
