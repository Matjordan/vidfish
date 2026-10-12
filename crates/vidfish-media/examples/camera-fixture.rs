#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = vidfish_media::fixtures::RtspFixture::start(1920, 1080)?;
    println!("{}", fixture.uri);
    tokio::signal::ctrl_c().await?;
    drop(fixture);
    Ok(())
}
