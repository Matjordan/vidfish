use std::io::{BufRead, Write};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (width, height) = if std::env::args().any(|a| a == "--small") {
        (640, 360)
    } else {
        (1920, 1080)
    };
    let fixture = std::sync::Arc::new(vidfish_media::fixtures::RtspFixture::start_codec(
        width,
        height,
        if std::env::args().any(|a| a == "--h265") {
            vidfish_media::Codec::H265
        } else {
            vidfish_media::Codec::H264
        },
    )?);
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
                "clients" => {
                    println!("CLIENTS {}", control.client_count());
                    let _ = std::io::stdout().flush();
                }
                "disconnect-first" => control.disconnect_client(0),
                "disconnect-last" => {
                    if let Some(index) = control.client_count().checked_sub(1) {
                        control.disconnect_client(index);
                    }
                }
                _ => {}
            }
        }
    });
    tokio::signal::ctrl_c().await?;
    drop(fixture);
    Ok(())
}
