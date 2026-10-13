//! Small operator action bridge. Scientific frame/segment correlation belongs to Epic 5.
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
#[derive(Clone, Serialize, Deserialize)]
pub struct Capture {
    pub camera: String,
    pub label: String,
    pub image: Option<PathBuf>,
    pub result: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub unix_ms: u64,
    pub elapsed_seconds: u64,
    pub kind: String,
    pub note: String,
    pub captures: Vec<Capture>,
}
pub struct Saved {
    pub event: Event,
    pub thumbnails: Vec<(String, usize, usize, Vec<u8>)>,
}
pub struct Input {
    pub camera: String,
    pub label: String,
    pub frame: Option<Arc<vidfish_media::Frame>>,
}
pub fn save(
    root: &Path,
    kind: &str,
    note: String,
    elapsed: u64,
    inputs: Vec<Input>,
    grab: bool,
) -> Result<Saved, String> {
    if note.len() > 16384 || inputs.len() > 16 {
        return Err("Operator action exceeds limits".into());
    }
    let result = (|| -> Result<Saved, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(root)?;
        let id = uuid::Uuid::new_v4().to_string();
        let directory = root.join(&id);
        std::fs::create_dir(&directory)?;
        let mut event = Event {
            id,
            unix_ms: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64,
            elapsed_seconds: elapsed,
            kind: kind.into(),
            note,
            captures: Vec::new(),
        };
        let mut thumbnails = Vec::new();
        for input in inputs {
            let mut capture = Capture {
                camera: input.camera,
                label: input.label,
                image: None,
                result: if grab {
                    "Not selected or no fresh preview"
                } else {
                    "Mark only"
                }
                .into(),
            };
            if grab
                && let Some(frame) = input
                    .frame
                    .filter(|f| f.received_at.elapsed() < std::time::Duration::from_secs(5))
            {
                let path = directory.join(format!("snapshot-{}.png", event.captures.len()));
                let file = std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&path)?;
                let mut encoder = png::Encoder::new(file, frame.width as u32, frame.height as u32);
                encoder.set_color(png::ColorType::Rgb);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder.write_header()?;
                writer.write_image_data(&frame.rgb)?;
                writer.finish()?;
                std::fs::File::open(&path)?.sync_all()?;
                capture.image = Some(path);
                capture.result = "Latest displayed frame (not synchronized)".into();
                let width = 128;
                let height = (frame.height * width / frame.width).clamp(1, 128);
                let mut rgb = Vec::with_capacity(width * height * 3);
                for y in 0..height {
                    for x in 0..width {
                        let offset = ((y * frame.height / height) * frame.width
                            + x * frame.width / width)
                            * 3;
                        rgb.extend_from_slice(&frame.rgb[offset..offset + 3]);
                    }
                }
                thumbnails.push((capture.label.clone(), width, height, rgb));
            }
            event.captures.push(capture);
        }
        let mut metadata = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(directory.join("event.json"))?;
        serde_json::to_writer_pretty(&mut metadata, &event)?;
        metadata.write_all(b"\n")?;
        metadata.sync_all()?;
        Ok(Saved { event, thumbnails })
    })();
    result.map_err(|_|"Operator event could not be saved; check output permissions and disk space. Partial files retained.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captures_are_playable_pngs_and_stale_unselected_cameras_remain_explicit() {
        let root = tempfile::tempdir().unwrap();
        let frame = Arc::new(vidfish_media::Frame {
            width: 2,
            height: 2,
            rgb: vec![127; 12],
            number: 1,
            received_at: std::time::Instant::now(),
        });
        let stale = Arc::new(vidfish_media::Frame {
            received_at: std::time::Instant::now() - std::time::Duration::from_secs(6),
            ..(*frame).clone()
        });
        let saved = save(
            root.path(),
            "Mark + Grab All",
            "sample note".into(),
            5,
            vec![
                Input {
                    camera: "fresh".into(),
                    label: "Fresh".into(),
                    frame: Some(frame),
                },
                Input {
                    camera: "stale".into(),
                    label: "Stale".into(),
                    frame: Some(stale),
                },
                Input {
                    camera: "unviewed".into(),
                    label: "Unviewed".into(),
                    frame: None,
                },
            ],
            true,
        )
        .unwrap();
        assert_eq!(
            saved
                .event
                .captures
                .iter()
                .filter(|c| c.image.is_some())
                .count(),
            1
        );
        let path = saved.event.captures[0].image.as_ref().unwrap();
        let decoder =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
        let reader = decoder.read_info().unwrap();
        assert_eq!((reader.info().width, reader.info().height), (2, 2));
        let disk: Event = serde_json::from_slice(
            &std::fs::read(path.parent().unwrap().join("event.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(disk.note, "sample note");
        let mark = save(
            root.path(),
            "Mark",
            "second note".into(),
            6,
            Vec::new(),
            false,
        )
        .unwrap();
        assert_ne!(mark.event.id, disk.id);
        assert!(path.exists());
    }
}
