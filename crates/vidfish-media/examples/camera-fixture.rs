use std::io::BufRead;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (width, height) = if std::env::args().any(|a| a == "--small") {
        (640, 360)
    } else {
        (1920, 1080)
    };
    let fixture = std::sync::Arc::new(vidfish_media::fixtures::RtspFixture::start(width, height)?);
    println!("{}", fixture.uri);
    let onvif = vidfish_media::fixtures::OnvifFixture::start(fixture.uri.clone())?;
    println!("ONVIF {}", onvif.device_url);
    println!("WS-Discovery (unicast fixture) {}", onvif.discovery_address);
    let control = fixture.clone();
    std::thread::spawn(move || {
        for command in std::io::stdin().lock().lines().map_while(Result::ok) {
            match command.trim() {
                "pause" => control.set_paused(true),
                "resume" => control.set_paused(false),
                _ => {}
            }
        }
    });
    tokio::signal::ctrl_c().await?;
    drop(fixture);
    Ok(())
}
