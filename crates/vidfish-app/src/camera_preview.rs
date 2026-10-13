//! Resolve saved camera/profile credentials away from the UI thread.
use std::path::Path;
use vidfish_core::cameras::CameraRecord;
use vidfish_media::{Codec, Source};

pub fn source(
    camera: &CameraRecord,
    profile: Option<&vidfish_onvif::Profile>,
    inventory: &Path,
) -> Result<(Source, Codec), String> {
    let (uri, codec) = if let Some(uri) = &camera.manual_rtsp {
        (
            uri.clone(),
            if camera.manual_h265 {
                Codec::H265
            } else {
                Codec::H264
            },
        )
    } else if let Some(profile) = profile {
        (
            profile.rtsp_uri.clone(),
            match profile.codec.as_str() {
                "H264" => Codec::H264,
                "H265" => Codec::H265,
                _ => return Err("Selected camera profile codec is unsupported".into()),
            },
        )
    } else {
        return Err(
            "Camera has no available RTSP profile; check login or camera connection".into(),
        );
    };
    let source = match &camera.credential_key {
        Some(id) => {
            vidfish_onvif::credentials::load(&vidfish_onvif::credentials::file_path(inventory), id)
                .map_err(|error| error.to_string())?
                .expose(|username, password| Source::AuthenticatedRtsp {
                    uri,
                    username: zeroize::Zeroizing::new(username.into()),
                    password: zeroize::Zeroizing::new(password.into()),
                })
        }
        None => Source::Rtsp(uri),
    };
    Ok((source, codec))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_camera_login_applies_to_rtsp_without_embedding_password_in_uri() {
        let directory = tempfile::tempdir().unwrap();
        let inventory = directory.path().join("cameras.json");
        let mut store = vidfish_core::cameras::CameraStore::default();
        let id = store.add_manual("Bench", "rtsp://127.0.0.1/live").unwrap();
        store.cameras[0].credential_key = Some(id.clone());
        vidfish_onvif::credentials::save(
            &vidfish_onvif::credentials::file_path(&inventory),
            &id,
            &vidfish_onvif::credentials::Credentials::new(
                "fixture-user".into(),
                "fixture-password".into(),
            )
            .unwrap(),
        )
        .unwrap();
        let (
            Source::AuthenticatedRtsp {
                uri,
                username,
                password,
            },
            Codec::H264,
        ) = source(&store.cameras[0], None, &inventory).unwrap()
        else {
            panic!("authenticated source expected")
        };
        assert_eq!(uri, "rtsp://127.0.0.1/live");
        assert_eq!(&*username, "fixture-user");
        assert_eq!(&*password, "fixture-password");
    }
}
