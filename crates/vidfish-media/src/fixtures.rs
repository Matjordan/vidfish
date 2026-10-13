//! Loopback-only simulated cameras for repeatable integration tests.
use crate::{MediaError, Result};
mod onvif;
use gstreamer as gst;
use gstreamer_rtsp_server as rtsp;
pub use onvif::OnvifFixture;
use rtsp::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

pub struct RtspFixture {
    pub uri: String,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl RtspFixture {
    /// Simulate a silent transport stall while leaving the RTSP session alive.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
    }
    pub fn start(width: u32, height: u32) -> Result<Self> {
        Self::start_codec(width, height, crate::Codec::H264)
    }
    pub fn start_codec(width: u32, height: u32, codec: crate::Codec) -> Result<Self> {
        Self::start_inner(width, height, codec, None)
    }
    pub fn start_authenticated(
        width: u32,
        height: u32,
        username: String,
        password: String,
    ) -> Result<Self> {
        Self::start_inner(
            width,
            height,
            crate::Codec::H264,
            Some((username, password)),
        )
    }
    fn start_inner(
        width: u32,
        height: u32,
        codec: crate::Codec,
        authentication: Option<(String, String)>,
    ) -> Result<Self> {
        crate::initialize()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let paused = Arc::new(AtomicBool::new(false));
        let pausing = paused.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<String>>(1);
        let worker = std::thread::spawn(move || {
            let context = gst::glib::MainContext::new();
            let result = context.with_thread_default(|| {
                let server = rtsp::RTSPServer::new();
                server.set_address("127.0.0.1");
                server.set_service("0");
                let factory = rtsp::RTSPMediaFactory::new();
                let (encoder, pay, options) = match codec {
                    crate::Codec::H264 => ("x264enc", "rtph264pay", ""),
                    crate::Codec::H265 => ("x265enc", "rtph265pay", "option-string=pools=1:frame-threads=1"),
                };
                factory.set_launch(&format!("( videotestsrc name=fixture-source is-live=true ! video/x-raw,width={width},height={height},framerate=30/1 ! {encoder} tune=zerolatency speed-preset=ultrafast key-int-max=30 {options} ! {pay} name=pay0 pt=96 config-interval=1 )"));
                factory.set_shared(true);
                factory.connect_media_configure(move |_, media| {
                    let Ok(bin) = media.element().downcast::<gst::Bin>() else { return };
                    let Some(source) = bin.by_name("fixture-source") else { return };
                    let Some(pad) = source.static_pad("src") else { return };
                    let pausing = pausing.clone();
                    pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                        if pausing.load(Ordering::Acquire) {gst::PadProbeReturn::Drop} else {gst::PadProbeReturn::Ok}
                    });
                });
                if let Some((username, password)) = authentication {
                    let auth = rtsp::RTSPAuth::new();
                    let token = rtsp::RTSPToken::builder().field("media.factory.role", "viewer").build();
                    auth.add_basic(&rtsp::RTSPAuth::make_basic(&username, &password), &token);
                    factory.add_role_from_structure(&gst::Structure::builder("viewer").field("media.factory.access", true).field("media.factory.construct", true).build());
                    server.set_auth(Some(&auth));
                }
                let mounts = server.mount_points().ok_or_else(|| MediaError("fixture mount points unavailable".into()))?;
                mounts.add_factory("/camera", factory);
                let source_id = server.attach(Some(&context)).map_err(|_| MediaError("cannot bind RTSP fixture".into()))?;
                let port = server.bound_port();
                let main_loop = gst::glib::MainLoop::new(Some(&context), false);
                let quitting = main_loop.clone();
                let source = gst::glib::timeout_source_new(std::time::Duration::from_millis(50), None, gst::glib::Priority::DEFAULT, move || {
                    if stopping.load(Ordering::Acquire) { quitting.quit(); gst::glib::ControlFlow::Break } else { gst::glib::ControlFlow::Continue }
                });
                source.attach(Some(&context));
                let _ = ready_tx.send(Ok(format!("rtsp://127.0.0.1:{port}/camera")));
                main_loop.run();
                if let Some(source) = context.find_source_by_id(&source_id) { source.destroy(); }
                Ok::<_, MediaError>(())
            });
            if let Ok(Err(error)) = result {
                tracing::error!(%error, "fixture failed");
            }
        });
        match ready_rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Ok(uri)) => Ok(Self {
                uri,
                stop,
                paused,
                worker: Some(worker),
            }),
            _ => {
                stop.store(true, Ordering::Release);
                let _ = worker.join();
                Err(MediaError("RTSP fixture did not become ready".into()))
            }
        }
    }
}
impl Drop for RtspFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
