//! Bounded loopback fanout of the recorder's encoded stream; never a camera connection.
use crate::{Codec, MediaError, Result};
use gstreamer as gst;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

const MAX_CLIENTS: usize = 8;
const MAX_HEADER: usize = 65536;
const MAX_PAYLOAD: usize = 4 * 1024 * 1024;
const MAGIC: &[u8; 8] = b"VFENC001";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedFeed {
    pub address: SocketAddr,
    pub identity: String,
    pub codec: Codec,
}
impl std::fmt::Debug for SharedFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedFeed")
            .field("address", &self.address)
            .field("codec", &self.codec)
            .finish_non_exhaustive()
    }
}

struct Packet {
    buffer: gst::Buffer,
    caps: gst::Caps,
}
struct Delivery {
    packet: Arc<Packet>,
    discontinuity: bool,
}
struct Client {
    sender: mpsc::SyncSender<Delivery>,
    needs_keyframe: bool,
}
struct State {
    stopping: AtomicBool,
    active: AtomicUsize,
    clients: Mutex<Vec<Client>>,
    workers: Mutex<Vec<thread::JoinHandle<()>>>,
}
#[derive(Clone)]
pub(crate) struct Publisher(Arc<State>);
impl Publisher {
    pub(crate) fn publish(&self, buffer: gst::Buffer, caps: gst::Caps) {
        let keyframe = !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT);
        let packet = Arc::new(Packet { buffer, caps });
        self.0.clients.lock().unwrap().retain_mut(|client| {
            if client.needs_keyframe && !keyframe {
                return true;
            }
            let delivery = Delivery {
                packet: packet.clone(),
                discontinuity: client.needs_keyframe,
            };
            match client.sender.try_send(delivery) {
                Ok(()) => {
                    client.needs_keyframe = false;
                    true
                }
                Err(mpsc::TrySendError::Full(_)) => {
                    client.needs_keyframe = true;
                    true
                }
                Err(mpsc::TrySendError::Disconnected(_)) => false,
            }
        });
    }
}

pub(crate) struct Broker {
    feed: SharedFeed,
    state: Arc<State>,
    listener: Option<thread::JoinHandle<()>>,
}
impl Broker {
    pub(crate) fn start(codec: Codec) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|_| MediaError("cannot bind shared recording feed".into()))?;
        listener
            .set_nonblocking(true)
            .map_err(|_| MediaError("cannot configure shared recording feed".into()))?;
        let feed = SharedFeed {
            address: listener
                .local_addr()
                .map_err(|_| MediaError("shared feed address unavailable".into()))?,
            identity: uuid::Uuid::new_v4().to_string(),
            codec,
        };
        let state = Arc::new(State {
            stopping: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            clients: Mutex::new(Vec::new()),
            workers: Mutex::new(Vec::new()),
        });
        let serving = state.clone();
        let identity = feed.identity.clone();
        let worker = thread::spawn(move || {
            while !serving.stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, peer)) => {
                        if !peer.ip().is_loopback()
                            || serving.active.load(Ordering::Acquire) >= MAX_CLIENTS
                        {
                            continue;
                        }
                        serving.active.fetch_add(1, Ordering::AcqRel);
                        let state = serving.clone();
                        let identity = identity.clone();
                        let worker = thread::spawn(move || {
                            let _ = serve(stream, &identity, &state);
                            state.active.fetch_sub(1, Ordering::AcqRel);
                        });
                        let mut workers = serving.workers.lock().unwrap();
                        workers.retain(|w| !w.is_finished());
                        workers.push(worker);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            feed,
            state,
            listener: Some(worker),
        })
    }
    pub(crate) fn descriptor(&self) -> SharedFeed {
        self.feed.clone()
    }
    pub(crate) fn publisher(&self) -> Publisher {
        Publisher(self.state.clone())
    }
}
impl Drop for Broker {
    fn drop(&mut self) {
        self.state.stopping.store(true, Ordering::Release);
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
        for worker in self.state.workers.lock().unwrap().drain(..) {
            let _ = worker.join();
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Header {
    caps: String,
    size: usize,
    flags: u32,
    pts: Option<u64>,
    dts: Option<u64>,
    duration: Option<u64>,
}
fn serve(mut stream: TcpStream, identity: &str, state: &State) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(Duration::from_millis(250)))?;
    stream.set_nodelay(true)?;
    let mut length = [0; 2];
    stream.read_exact(&mut length)?;
    let size = u16::from_be_bytes(length) as usize;
    if size != identity.len() {
        return Ok(());
    }
    let mut presented = vec![0; size];
    stream.read_exact(&mut presented)?;
    if presented != identity.as_bytes() {
        return Ok(());
    }
    stream.write_all(MAGIC)?;
    let (sender, receiver) = mpsc::sync_channel(2);
    state.clients.lock().unwrap().push(Client {
        sender,
        needs_keyframe: true,
    });
    while !state.stopping.load(Ordering::Acquire) {
        let delivery = match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(delivery) => delivery,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };
        let Packet { buffer, caps } = &*delivery.packet;
        let data = buffer.map_readable().map_err(std::io::Error::other)?;
        let flags = buffer.flags()
            | if delivery.discontinuity {
                gst::BufferFlags::DISCONT
            } else {
                gst::BufferFlags::empty()
            };
        let header = serde_json::to_vec(&Header {
            caps: caps.to_string(),
            size: data.len(),
            flags: flags.bits(),
            pts: buffer.pts().map(|t| t.nseconds()),
            dts: buffer.dts().map(|t| t.nseconds()),
            duration: buffer.duration().map(|t| t.nseconds()),
        })
        .map_err(std::io::Error::other)?;
        if header.len() > MAX_HEADER || data.len() > MAX_PAYLOAD {
            break;
        }
        stream.write_all(&(header.len() as u32).to_be_bytes())?;
        stream.write_all(&header)?;
        stream.write_all(&data)?;
    }
    Ok(())
}

pub(crate) fn source(feed: &SharedFeed) -> Result<gstreamer_app::AppSrc> {
    if !feed.address.ip().is_loopback() || feed.identity.len() > 128 {
        return Err(MediaError("invalid local recording feed".into()));
    }
    let mut connection = TcpStream::connect_timeout(&feed.address, Duration::from_millis(500))
        .map_err(|_| MediaError("shared recording feed is unavailable".into()))?;
    connection
        .set_nodelay(true)
        .map_err(|_| MediaError("cannot configure shared feed".into()))?;
    connection
        .set_read_timeout(Some(Duration::from_millis(2500)))
        .map_err(|_| MediaError("cannot configure shared feed".into()))?;
    connection
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|_| MediaError("cannot configure shared feed".into()))?;
    let mut connect = || -> std::io::Result<()> {
        connection.write_all(&(feed.identity.len() as u16).to_be_bytes())?;
        connection.write_all(feed.identity.as_bytes())?;
        let mut magic = [0; 8];
        connection.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(std::io::Error::other("invalid shared feed protocol"));
        }
        Ok(())
    };
    connect().map_err(|_| MediaError("cannot attach to shared recording feed".into()))?;
    let source = gstreamer_app::AppSrc::builder()
        .name("shared-camera")
        .format(gst::Format::Time)
        .is_live(true)
        .max_buffers(2)
        .max_bytes(MAX_PAYLOAD as u64)
        .build();
    source.set_callbacks(
        gstreamer_app::AppSrcCallbacks::builder()
            .need_data(move |source, _| match receive(&mut connection) {
                Ok((caps, buffer)) => {
                    source.set_caps(Some(&caps));
                    let _ = source.push_buffer(buffer);
                }
                Err(_) => {
                    let _ = source.end_of_stream();
                }
            })
            .build(),
    );
    Ok(source)
}
fn receive(stream: &mut TcpStream) -> std::io::Result<(gst::Caps, gst::Buffer)> {
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let size = u32::from_be_bytes(length) as usize;
    if size == 0 || size > MAX_HEADER {
        return Err(std::io::Error::other("invalid feed header length"));
    }
    let mut encoded = vec![0; size];
    stream.read_exact(&mut encoded)?;
    let header: Header = serde_json::from_slice(&encoded).map_err(std::io::Error::other)?;
    if header.size > MAX_PAYLOAD {
        return Err(std::io::Error::other("invalid feed payload length"));
    }
    let caps = header
        .caps
        .parse::<gst::Caps>()
        .map_err(std::io::Error::other)?;
    let mut payload = vec![0; header.size];
    stream.read_exact(&mut payload)?;
    let mut buffer = gst::Buffer::from_mut_slice(payload);
    let writable = buffer.get_mut().unwrap();
    writable.set_pts(header.pts.map(gst::ClockTime::from_nseconds));
    writable.set_dts(header.dts.map(gst::ClockTime::from_nseconds));
    writable.set_duration(header.duration.map(gst::ClockTime::from_nseconds));
    writable.set_flags(gst::BufferFlags::from_bits_truncate(header.flags));
    Ok((caps, buffer))
}
