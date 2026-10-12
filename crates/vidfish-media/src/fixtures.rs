//! Loopback-only simulated cameras for repeatable integration tests.
use crate::{MediaError, Result};
use gstreamer as gst;
use gstreamer_rtsp_server as rtsp;
use rtsp::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

pub struct RtspFixture {
    pub uri: String,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl RtspFixture {
    pub fn start(width: u32, height: u32) -> Result<Self> {
        crate::initialize()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<String>>(1);
        let worker = std::thread::spawn(move || {
            let context = gst::glib::MainContext::new();
            let result = context.with_thread_default(|| {
                let server = rtsp::RTSPServer::new();
                server.set_address("127.0.0.1");
                server.set_service("0");
                let factory = rtsp::RTSPMediaFactory::new();
                factory.set_launch(&format!("( videotestsrc is-live=true ! video/x-raw,width={width},height={height},framerate=30/1 ! x264enc tune=zerolatency speed-preset=ultrafast key-int-max=30 ! rtph264pay name=pay0 pt=96 config-interval=1 )"));
                factory.set_shared(true);
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
