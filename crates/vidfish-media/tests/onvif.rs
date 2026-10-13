#![cfg(feature = "fixtures")]
use std::{
    io::{Read, Write},
    net::{TcpStream, UdpSocket},
    time::Duration,
};

fn request(address: &str, action: &str) -> (String, Vec<u8>) {
    let url = url::Url::parse(address).unwrap();
    let mut socket = TcpStream::connect((url.host_str().unwrap(), url.port().unwrap())).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let body = format!(
        r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:t="http://www.onvif.org/ver10/media/wsdl"><s:Body><t:{action}/></s:Body></s:Envelope>"#
    );
    write!(
        socket,
        "POST {} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
        url.path(),
        body.len(),
        body
    )
    .unwrap();
    let mut response = Vec::new();
    socket.read_to_end(&mut response).unwrap();
    let split = response
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .unwrap();
    (
        String::from_utf8(response[..split].to_vec()).unwrap(),
        response[split + 4..].to_vec(),
    )
}

#[test]
fn discovery_and_media_service_fixture_expose_consistent_camera() {
    let rtsp = vidfish_media::fixtures::RtspFixture::start(1920, 1080).unwrap();
    let camera = vidfish_media::fixtures::OnvifFixture::start(rtsp.uri.clone()).unwrap();
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let probe = br#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:a="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery"><s:Header><a:MessageID>urn:uuid:test-probe</a:MessageID></s:Header><s:Body><d:Probe/></s:Body></s:Envelope>"#;
    socket.send_to(probe, camera.discovery_address).unwrap();
    let mut buffer = [0u8; 8192];
    let count = socket.recv(&mut buffer).unwrap();
    let discovery = String::from_utf8(buffer[..count].to_vec()).unwrap();
    assert!(discovery.contains("ProbeMatches"));
    assert!(discovery.contains("<a:RelatesTo>urn:uuid:test-probe</a:RelatesTo>"));
    assert!(discovery.contains(&camera.device_url));
    for (action, expected) in [
        ("GetDeviceInformation", "fixture-001"),
        ("GetCapabilities", "/onvif/media_service"),
        ("GetProfiles", "profile-main"),
        ("GetStreamUri", rtsp.uri.as_str()),
        ("GetSnapshotUri", "/snapshot"),
    ] {
        let (headers, body) = request(&camera.device_url, action);
        assert!(headers.starts_with("HTTP/1.1 200 OK"));
        let response = String::from_utf8(body).unwrap();
        assert!(response.contains(expected), "{response}");
        let mut reader = quick_xml::Reader::from_str(&response);
        loop {
            if reader.read_event().unwrap() == quick_xml::events::Event::Eof {
                break;
            }
        }
    }
    let (headers, body) = request(&camera.device_url, "UnsupportedAction");
    assert!(headers.starts_with("HTTP/1.1 500"));
    assert!(String::from_utf8(body).unwrap().contains("Fault"));

    let url = url::Url::parse(&camera.device_url).unwrap();
    let mut client = TcpStream::connect((url.host_str().unwrap(), url.port().unwrap())).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    client
        .write_all(b"GET /snapshot HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).unwrap();
    let split = response
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .unwrap();
    assert!(String::from_utf8_lossy(&response[..split]).contains("image/jpeg"));
    assert_eq!(&response[split + 4..split + 6], &[0xff, 0xd8]);
    assert_eq!(&response[response.len() - 2..], &[0xff, 0xd9]);
}
