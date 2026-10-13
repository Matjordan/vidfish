//! Bounded ONVIF network operations. Run these blocking calls off the UI thread.
use reqwest::{Url, blocking::Client};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    fmt,
    io::Read,
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    time::{Duration, Instant},
};

const DEVICE: &str = "http://www.onvif.org/ver10/device/wsdl";
const MEDIA: &str = "http://www.onvif.org/ver10/media/wsdl";
const MAX_XML: u64 = 1024 * 1024;

/// Errors intentionally exclude camera URLs, server bodies, and credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub &'static str);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Error {}
type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Serialize, PartialEq, Eq)]
pub struct DiscoveredCamera {
    pub endpoint_id: String,
    pub device_url: String,
}

/// Select an IPv4 adapter by its local address; use a unicast destination for fixtures.
/// A finite collection window and 16-device limit bound memory and network activity.
pub fn discover(
    interface: Ipv4Addr,
    destination: SocketAddrV4,
    window: Duration,
) -> Result<Vec<DiscoveredCamera>> {
    if window.is_zero() || window > Duration::from_secs(30) {
        return Err(Error("discovery window must be between 1ms and 30s"));
    }
    let socket = UdpSocket::bind((interface, 0))
        .map_err(|_| Error("cannot bind discovery adapter; select a local IPv4 address"))?;
    socket2::SockRef::from(&socket)
        .set_multicast_if_v4(&interface)
        .map_err(|_| Error("cannot select discovery multicast adapter"))?;
    socket
        .set_multicast_ttl_v4(1)
        .map_err(|_| Error("cannot configure discovery multicast"))?;
    let id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
    let body = format!(
        r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:a="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery" xmlns:dn="http://www.onvif.org/ver10/network/wsdl"><s:Header><a:MessageID>{id}</a:MessageID><a:To>urn:schemas-xmlsoap-org:ws:2005:04:discovery</a:To><a:Action>http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe</a:Action></s:Header><s:Body><d:Probe><d:Types>dn:NetworkVideoTransmitter</d:Types></d:Probe></s:Body></s:Envelope>"#
    );
    socket
        .send_to(body.as_bytes(), destination)
        .map_err(|_| Error("discovery send failed; check adapter and firewall"))?;
    let deadline = Instant::now() + window;
    let mut cameras = Vec::new();
    let mut seen = BTreeSet::new();
    let mut buffer = [0u8; 65536];
    while cameras.len() < 16 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        socket
            .set_read_timeout(Some(remaining))
            .map_err(|_| Error("cannot configure discovery timeout"))?;
        match socket.recv_from(&mut buffer) {
            Ok((count, _)) => {
                let Ok(xml) = std::str::from_utf8(&buffer[..count]) else {
                    continue;
                };
                for camera in parse_probe(xml, &id).unwrap_or_default() {
                    if seen.insert((camera.endpoint_id.clone(), camera.device_url.clone())) {
                        cameras.push(camera);
                    }
                    if cameras.len() == 16 {
                        break;
                    }
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                break;
            }
            Err(_) => return Err(Error("discovery receive failed; check network adapter")),
        }
    }
    Ok(cameras)
}

fn parse_probe(xml: &str, request_id: &str) -> Result<Vec<DiscoveredCamera>> {
    let doc = document(xml)?;
    if text(doc.root(), "RelatesTo") != Some(request_id) {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    for node in doc.descendants().filter(|n| {
        n.has_tag_name((
            "http://schemas.xmlsoap.org/ws/2005/04/discovery",
            "ProbeMatch",
        ))
    }) {
        let Some(id) = text(node, "Address").filter(|s| !s.is_empty() && s.len() <= 256) else {
            continue;
        };
        for address in text(node, "XAddrs")
            .unwrap_or_default()
            .split_whitespace()
            .take(16)
        {
            if validate_url(address, &["http", "https"]).is_ok() {
                result.push(DiscoveredCamera {
                    endpoint_id: id.into(),
                    device_url: address.into(),
                });
            }
        }
    }
    Ok(result)
}

#[derive(Clone, Serialize)]
pub struct Profile {
    pub token: String,
    pub name: String,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub rtsp_uri: String,
    pub snapshot_uri: Option<String>,
}
#[derive(Clone, Serialize)]
pub struct Camera {
    pub manufacturer: String,
    pub model: String,
    pub serial_number: String,
    pub profiles: Vec<Profile>,
}

pub struct OnvifClient {
    http: Client,
}
impl OnvifClient {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: Client::builder()
                .timeout(Duration::from_secs(5))
                .connect_timeout(Duration::from_secs(3))
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()
                .map_err(|_| Error("cannot initialize ONVIF HTTP client"))?,
        })
    }
    fn soap(&self, address: &str, namespace: &str, operation: &str, inner: &str) -> Result<String> {
        validate_url(address, &["http", "https"])?;
        let body = format!(
            r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:t="{namespace}" xmlns:tt="http://www.onvif.org/ver10/schema"><s:Body><t:{operation}>{inner}</t:{operation}></s:Body></s:Envelope>"#
        );
        let response = self
            .http
            .post(address)
            .header(
                "Content-Type",
                format!("application/soap+xml; charset=utf-8; action=\"{namespace}/{operation}\""),
            )
            .body(body)
            .send()
            .map_err(|_| {
                Error("ONVIF request failed; check camera address, network, and TLS certificate")
            })?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error(
                "camera requires authentication; authenticated ONVIF is not implemented yet; use manual RTSP fallback",
            ));
        }
        if !response.status().is_success() {
            return Err(Error(
                "camera rejected ONVIF operation; check service support and authentication",
            ));
        }
        let mut bytes = Vec::new();
        response
            .take(MAX_XML + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error("cannot read ONVIF response"))?;
        if bytes.len() as u64 > MAX_XML {
            return Err(Error("ONVIF response exceeds 1MiB limit"));
        }
        let xml = String::from_utf8(bytes).map_err(|_| Error("ONVIF response is not UTF-8"))?;
        let doc = document(&xml)?;
        if doc
            .descendants()
            .any(|n| n.is_element() && n.tag_name().name() == "Fault")
        {
            return Err(Error(
                "camera returned an ONVIF fault; check service support and credentials",
            ));
        }
        Ok(xml)
    }
    pub fn resolve(&self, address: &str) -> Result<Camera> {
        let info = self.soap(address, DEVICE, "GetDeviceInformation", "")?;
        let info = document(&info)?;
        let caps = self.soap(
            address,
            DEVICE,
            "GetCapabilities",
            "<t:Category>Media</t:Category>",
        )?;
        let caps = document(&caps)?;
        let media = caps
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "Media")
            .and_then(|n| text(n, "XAddr"))
            .ok_or(Error(
                "camera has no ONVIF media service; use manual RTSP fallback",
            ))?;
        validate_url(media, &["http", "https"])?;
        let profiles = self.soap(media, MEDIA, "GetProfiles", "")?;
        let profiles = document(&profiles)?;
        let mut resolved = Vec::new();
        for node in profiles
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "Profiles")
            .take(16)
        {
            let codec = text(node, "Encoding").unwrap_or_default();
            if !matches!(codec, "H264" | "H265") {
                continue;
            }
            let token = node
                .attribute("token")
                .filter(|t| !t.is_empty() && t.len() <= 256)
                .ok_or(Error("ONVIF profile token missing or invalid"))?;
            let inner = format!(
                "<t:StreamSetup><tt:Stream>RTP-Unicast</tt:Stream><tt:Transport><tt:Protocol>RTSP</tt:Protocol></tt:Transport></t:StreamSetup><t:ProfileToken>{}</t:ProfileToken>",
                escape(token)
            );
            let stream = self.soap(media, MEDIA, "GetStreamUri", &inner)?;
            let stream = document(&stream)?;
            let uri =
                text(stream.root(), "Uri").ok_or(Error("camera did not return an RTSP URI"))?;
            validate_url(uri, &["rtsp", "rtsps"])?;
            let snapshot = self
                .soap(
                    media,
                    MEDIA,
                    "GetSnapshotUri",
                    &format!("<t:ProfileToken>{}</t:ProfileToken>", escape(token)),
                )
                .ok()
                .and_then(|xml| {
                    document(&xml)
                        .ok()
                        .and_then(|doc| text(doc.root(), "Uri").map(str::to_owned))
                })
                .filter(|uri| validate_url(uri, &["http", "https"]).is_ok());
            resolved.push(Profile {
                token: token.into(),
                name: text(node, "Name").unwrap_or(token).into(),
                codec: codec.into(),
                width: text(node, "Width")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
                height: text(node, "Height")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
                rtsp_uri: uri.into(),
                snapshot_uri: snapshot,
            });
        }
        if resolved.is_empty() {
            return Err(Error(
                "no H.264/H.265 ONVIF profiles found; use manual RTSP fallback",
            ));
        }
        Ok(Camera {
            manufacturer: text(info.root(), "Manufacturer").unwrap_or_default().into(),
            model: text(info.root(), "Model").unwrap_or_default().into(),
            serial_number: text(info.root(), "SerialNumber").unwrap_or_default().into(),
            profiles: resolved,
        })
    }
}
fn document(xml: &str) -> Result<roxmltree::Document<'_>> {
    roxmltree::Document::parse(xml).map_err(|_| Error("invalid ONVIF XML response"))
}
fn text<'a>(node: roxmltree::Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.descendants()
        .find(|n| n.is_element() && n.tag_name().name() == name)
        .and_then(|n| n.text())
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
pub fn validate_url(address: &str, schemes: &[&str]) -> Result<()> {
    let url = Url::parse(address).map_err(|_| Error("invalid camera URL"))?;
    if !schemes.contains(&url.scheme())
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || address.len() > 4096
    {
        return Err(Error(
            "camera URL must use a supported scheme, include a host, and exclude embedded credentials",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_requires_request_correlation_and_rejects_dtd() {
        let xml = r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery"><s:Header><RelatesTo>other</RelatesTo></s:Header><s:Body><d:ProbeMatch><Address>camera</Address><XAddrs>http://127.0.0.1/onvif</XAddrs></d:ProbeMatch></s:Body></s:Envelope>"#;
        assert!(parse_probe(xml, "mine").unwrap().is_empty());
        assert_eq!(
            parse_probe(&xml.replace("other", "mine"), "mine")
                .unwrap()
                .len(),
            1
        );
        assert!(document("<!DOCTYPE x [<!ENTITY a 'b'>]><x>&a;</x>").is_err());
    }
    #[test]
    fn camera_urls_and_errors_do_not_expose_credentials() {
        for uri in [
            "http://user:secret@localhost/onvif",
            "file:///etc/passwd",
            "rtsp://user:secret@localhost/live",
        ] {
            let err = validate_url(uri, &["http", "https", "rtsp"]).unwrap_err();
            assert!(!format!("{err:?} {err}").contains("secret"));
        }
    }
}
