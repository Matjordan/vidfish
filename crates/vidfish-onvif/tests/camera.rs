use std::{
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};
use vidfish_onvif::{OnvifClient, discover};

#[test]
fn discovers_and_resolves_real_fixture_services() {
    let rtsp = vidfish_media::fixtures::RtspFixture::start(1920, 1080).unwrap();
    let fixture = vidfish_media::fixtures::OnvifFixture::start(rtsp.uri.clone()).unwrap();
    let SocketAddr::V4(destination) = fixture.discovery_address else {
        panic!("IPv4 fixture expected")
    };
    let found = discover(Ipv4Addr::LOCALHOST, destination, Duration::from_millis(150)).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].device_url, fixture.device_url);
    assert_eq!(
        found[0].endpoint_id,
        "urn:uuid:11111111-1111-4111-8111-111111111111"
    );
    let camera = OnvifClient::new()
        .unwrap()
        .resolve(&found[0].device_url)
        .unwrap();
    assert_eq!(camera.serial_number, "fixture-001");
    assert_eq!(camera.profiles.len(), 1);
    let profile = &camera.profiles[0];
    assert_eq!(profile.rtsp_uri, rtsp.uri);
    assert_eq!(profile.codec, "H264");
    assert_eq!((profile.width, profile.height), (1920, 1080));
    assert!(
        profile
            .snapshot_uri
            .as_ref()
            .unwrap()
            .ends_with("/snapshot")
    );
}
