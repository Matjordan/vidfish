#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = vidfish_media::fixtures::RtspFixture::start(1920, 1080)?;
    println!("{}", fixture.uri);
    let onvif = vidfish_media::fixtures::OnvifFixture::start(fixture.uri.clone())?;
    println!("ONVIF {}", onvif.device_url);
    println!("WS-Discovery (unicast fixture) {}", onvif.discovery_address);
    tokio::signal::ctrl_c().await?;
    drop(fixture);
    Ok(())
}
