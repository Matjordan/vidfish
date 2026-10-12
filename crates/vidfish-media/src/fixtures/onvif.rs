use crate::{MediaError, Result};
use gst::prelude::*;
use gstreamer as gst;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream, UdpSocket},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// ONVIF device/media SOAP and unicast WS-Discovery fixture, loopback only.
/// Production multicast, authentication, and vendor quirks belong to epic #2.
pub struct OnvifFixture {
    pub device_url: String,
    pub discovery_address: SocketAddr,
    stop: Arc<AtomicBool>,
    workers: Vec<std::thread::JoinHandle<()>>,
}
impl OnvifFixture {
    pub fn start(rtsp_uri: String) -> Result<Self> {
        let jpeg = snapshot()?;
        let listener =
            TcpListener::bind("127.0.0.1:0").map_err(|error| MediaError(error.to_string()))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| MediaError(error.to_string()))?;
        let address = listener
            .local_addr()
            .map_err(|error| MediaError(error.to_string()))?;
        let base = format!("http://{address}");
        let device_url = format!("{base}/onvif/device_service");
        let udp = UdpSocket::bind("127.0.0.1:0").map_err(|error| MediaError(error.to_string()))?;
        udp.set_read_timeout(Some(Duration::from_millis(50)))
            .map_err(|error| MediaError(error.to_string()))?;
        let discovery_address = udp
            .local_addr()
            .map_err(|error| MediaError(error.to_string()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let http = std::thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = serve(&mut stream, &base, &rtsp_uri, &jpeg);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => break,
                }
            }
        });
        let stopping = stop.clone();
        let xaddr = device_url.clone();
        let discovery = std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            while !stopping.load(Ordering::Acquire) {
                if let Ok((size, peer)) = udp.recv_from(&mut buffer) {
                    let body = String::from_utf8_lossy(&buffer[..size]);
                    if !operation(&body, "Probe") {
                        continue;
                    }
                    let id = text(&body, "MessageID").unwrap_or_default();
                    let id = quick_xml::escape::escape(&id);
                    let response = format!(
                        r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:a="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery" xmlns:dn="http://www.onvif.org/ver10/network/wsdl"><s:Header><a:MessageID>urn:uuid:22222222-2222-4222-8222-222222222222</a:MessageID><a:RelatesTo>{id}</a:RelatesTo><a:Action>http://schemas.xmlsoap.org/ws/2005/04/discovery/ProbeMatches</a:Action></s:Header><s:Body><d:ProbeMatches><d:ProbeMatch><a:EndpointReference><a:Address>urn:uuid:11111111-1111-4111-8111-111111111111</a:Address></a:EndpointReference><d:Types>dn:NetworkVideoTransmitter</d:Types><d:Scopes>onvif://www.onvif.org/name/VidfishFixture</d:Scopes><d:XAddrs>{xaddr}</d:XAddrs><d:MetadataVersion>1</d:MetadataVersion></d:ProbeMatch></d:ProbeMatches></s:Body></s:Envelope>"#
                    );
                    let _ = udp.send_to(response.as_bytes(), peer);
                }
            }
        });
        Ok(Self {
            device_url,
            discovery_address,
            stop,
            workers: vec![http, discovery],
        })
    }
}
impl Drop for OnvifFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn operation(xml: &str, name: &str) -> bool {
    let mut reader = quick_xml::Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(
                quick_xml::events::Event::Start(element) | quick_xml::events::Event::Empty(element),
            ) if element.local_name().as_ref() == name.as_bytes() => return true,
            Ok(quick_xml::events::Event::Eof) | Err(_) => return false,
            _ => {}
        }
    }
}
fn text(xml: &str, name: &str) -> Option<String> {
    let mut reader = quick_xml::Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(element))
                if element.local_name().as_ref() == name.as_bytes() =>
            {
                return reader.read_text(element.name()).ok().and_then(|text| {
                    text.decode()
                        .ok()
                        .map(|decoded| decoded.chars().take(256).collect())
                });
            }
            Ok(quick_xml::events::Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

fn serve(stream: &mut TcpStream, base: &str, rtsp: &str, jpeg: &[u8]) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    let mut request = Vec::new();
    let mut buffer = [0u8; 4096];
    let (header_end, length) = loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            return Ok(());
        }
        request.extend_from_slice(&buffer[..count]);
        if request.len() > 65536 {
            return reply(
                stream,
                "413 Payload Too Large",
                "text/plain",
                b"request too large",
            );
        }
        if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&request[..end]);
            let length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            break (end + 4, length);
        }
    };
    if length > 65536 {
        return reply(
            stream,
            "413 Payload Too Large",
            "text/plain",
            b"request too large",
        );
    }
    while request.len() < header_end + length {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            return reply(stream, "400 Bad Request", "text/plain", b"incomplete body");
        }
        request.extend_from_slice(&buffer[..count]);
    }
    let headers = String::from_utf8_lossy(&request[..header_end]);
    if headers.starts_with("GET /snapshot ") {
        return reply(stream, "200 OK", "image/jpeg", jpeg);
    }
    if !headers.starts_with("POST /onvif/") {
        return reply(stream, "404 Not Found", "text/plain", b"not found");
    }
    let xml = String::from_utf8_lossy(&request[header_end..header_end + length]);
    let body = if operation(&xml, "GetDeviceInformation") {
        "<tds:GetDeviceInformationResponse><tds:Manufacturer>Vidfish</tds:Manufacturer><tds:Model>Fixture</tds:Model><tds:FirmwareVersion>1</tds:FirmwareVersion><tds:SerialNumber>fixture-001</tds:SerialNumber><tds:HardwareId>vidfish-fixture</tds:HardwareId></tds:GetDeviceInformationResponse>".into()
    } else if operation(&xml, "GetCapabilities") {
        format!(
            "<tds:GetCapabilitiesResponse><tds:Capabilities><tt:Media><tt:XAddr>{base}/onvif/media_service</tt:XAddr></tt:Media></tds:Capabilities></tds:GetCapabilitiesResponse>"
        )
    } else if operation(&xml, "GetProfiles") {
        "<trt:GetProfilesResponse><trt:Profiles token=\"profile-main\" fixed=\"true\"><tt:Name>Main</tt:Name><tt:VideoEncoderConfiguration token=\"encoder-main\"><tt:Name>Main</tt:Name><tt:UseCount>1</tt:UseCount><tt:Encoding>H264</tt:Encoding><tt:Resolution><tt:Width>1920</tt:Width><tt:Height>1080</tt:Height></tt:Resolution><tt:Quality>1</tt:Quality><tt:RateControl><tt:FrameRateLimit>30</tt:FrameRateLimit><tt:EncodingInterval>1</tt:EncodingInterval><tt:BitrateLimit>2048</tt:BitrateLimit></tt:RateControl></tt:VideoEncoderConfiguration></trt:Profiles></trt:GetProfilesResponse>".into()
    } else if operation(&xml, "GetStreamUri") {
        format!(
            "<trt:GetStreamUriResponse><trt:MediaUri><tt:Uri>{}</tt:Uri><tt:InvalidAfterConnect>false</tt:InvalidAfterConnect><tt:InvalidAfterReboot>false</tt:InvalidAfterReboot><tt:Timeout>PT60S</tt:Timeout></trt:MediaUri></trt:GetStreamUriResponse>",
            quick_xml::escape::escape(rtsp)
        )
    } else if operation(&xml, "GetSnapshotUri") {
        format!(
            "<trt:GetSnapshotUriResponse><trt:MediaUri><tt:Uri>{base}/snapshot</tt:Uri><tt:InvalidAfterConnect>false</tt:InvalidAfterConnect><tt:InvalidAfterReboot>false</tt:InvalidAfterReboot><tt:Timeout>PT60S</tt:Timeout></trt:MediaUri></trt:GetSnapshotUriResponse>"
        )
    } else {
        return reply(stream, "500 Internal Server Error", "application/soap+xml", br#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope"><s:Body><s:Fault><s:Code><s:Value>s:Sender</s:Value></s:Code><s:Reason><s:Text xml:lang="en">Unsupported fixture action</s:Text></s:Reason></s:Fault></s:Body></s:Envelope>"#);
    };
    let response = format!(
        r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:tds="http://www.onvif.org/ver10/device/wsdl" xmlns:trt="http://www.onvif.org/ver10/media/wsdl" xmlns:tt="http://www.onvif.org/ver10/schema"><s:Body>{body}</s:Body></s:Envelope>"#
    );
    reply(
        stream,
        "200 OK",
        "application/soap+xml; charset=utf-8",
        response.as_bytes(),
    )
}
fn reply(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}
fn snapshot() -> Result<Vec<u8>> {
    crate::initialize()?;
    let pipeline = gst::parse::launch("videotestsrc num-buffers=1 ! video/x-raw,width=320,height=240 ! jpegenc ! appsink name=snapshot").map_err(|_| MediaError("fixture requires videotestsrc/jpegenc/appsink".into()))?.downcast::<gst::Pipeline>().map_err(|_| MediaError("snapshot pipeline unavailable".into()))?;
    let sink = pipeline
        .by_name("snapshot")
        .and_then(|element| element.downcast::<gstreamer_app::AppSink>().ok())
        .ok_or_else(|| MediaError("snapshot sink unavailable".into()))?;
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|_| MediaError("cannot start snapshot fixture".into()))?;
    let sample = sink.try_pull_sample(gst::ClockTime::from_seconds(5));
    let _ = pipeline.set_state(gst::State::Null);
    let sample = sample.ok_or_else(|| MediaError("snapshot fixture timed out".into()))?;
    let map = sample
        .buffer()
        .ok_or_else(|| MediaError("snapshot buffer missing".into()))?
        .map_readable()
        .map_err(|_| MediaError("cannot read snapshot fixture".into()))?;
    Ok(map.to_vec())
}
