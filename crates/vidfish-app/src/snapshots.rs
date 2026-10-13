//! Sequential, bounded still capture independent of recorder ownership.
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use vidfish_core::cameras::CameraRecord;
pub fn capture(
    camera: &CameraRecord,
    profile: Option<&vidfish_onvif::Profile>,
    inventory: &Path,
) -> Result<(Arc<vidfish_media::Frame>, String), String> {
    let attempted_http =
        camera.device_url.is_some() && profile.is_some_and(|p| p.snapshot_uri.is_some());
    if let (Some(device), Some(uri)) = (
        &camera.device_url,
        profile.and_then(|p| p.snapshot_uri.as_ref()),
    ) {
        let http = (|| -> Result<_, Box<dyn std::error::Error>> {
            let client = match &camera.credential_key {
                Some(key) => {
                    vidfish_onvif::OnvifClient::with_credentials(vidfish_onvif::credentials::load(
                        &vidfish_onvif::credentials::file_path(inventory),
                        key,
                    )?)?
                }
                None => vidfish_onvif::OnvifClient::new()?,
            };
            let bytes = client.snapshot(device, uri)?;
            let received_at = Instant::now();
            let mut reader =
                image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(4096);
            limits.max_image_height = Some(2160);
            limits.max_alloc = Some(64 * 1024 * 1024);
            reader.limits(limits);
            let image = reader.decode()?.to_rgb8();
            Ok(Arc::new(vidfish_media::Frame {
                width: image.width() as usize,
                height: image.height() as usize,
                rgb: image.into_raw(),
                number: 1,
                received_at,
            }))
        })();
        if let Ok(frame) = http {
            return Ok((
                frame,
                "ONVIF HTTP snapshot; local receipt time, camera exposure time unavailable".into(),
            ));
        }
    }
    let (source, codec) = crate::camera_preview::source(camera, profile, inventory)?;
    let frame = vidfish_media::snapshot(source, codec, Duration::from_secs(8)).map_err(|_| {
        "Temporary RTSP capture failed or timed out; check camera/network/login".to_string()
    })?;
    Ok((
        frame,
        if attempted_http {"Temporary RTSP fallback after ONVIF snapshot failure; local receipt time, camera exposure time unavailable"} else {"Temporary RTSP frame; local receipt time, camera exposure time unavailable"}.into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hidden_camera_http_and_temporary_rtsp_capture_without_permanent_preview() {
        let rtsp = vidfish_media::fixtures::RtspFixture::start(1920, 1080).unwrap();
        let onvif = vidfish_media::fixtures::OnvifFixture::start(rtsp.uri.clone()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let inventory = dir.path().join("cameras.json");
        let mut store = vidfish_core::cameras::CameraStore::default();
        store.add_onvif("Hidden", &onvif.device_url).unwrap();
        let resolved = vidfish_onvif::OnvifClient::new()
            .unwrap()
            .resolve(&onvif.device_url)
            .unwrap();
        let (frame, method) =
            capture(&store.cameras[0], resolved.profiles.first(), &inventory).unwrap();
        assert_eq!((frame.width, frame.height), (320, 240));
        assert!(method.starts_with("ONVIF"));
        store.add_manual("Fallback", &rtsp.uri).unwrap();
        let (frame, method) = capture(&store.cameras[1], None, &inventory).unwrap();
        assert_eq!((frame.width, frame.height), (1920, 1080));
        assert!(method.starts_with("Temporary"));
        assert_eq!(vidfish_media::preview_worker_counts().0, 0);
    }
}
