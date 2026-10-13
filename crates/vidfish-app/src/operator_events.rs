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
    #[serde(default)]
    pub complete: bool,
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
            complete: false,
        };
        persist_event(&directory, &event)?;
        let mut thumbnails = Vec::new();
        let action_at = std::time::Instant::now();
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
                && let Some(frame) = input.frame.filter(|f| {
                    action_at.saturating_duration_since(f.received_at)
                        < std::time::Duration::from_secs(5)
                })
            {
                let path = directory.join(format!("snapshot-{}.png", event.captures.len()));
                let written = (|| -> Result<(), Box<dyn std::error::Error>> {
                    if !(1..=4096).contains(&frame.width)
                        || !(1..=2160).contains(&frame.height)
                        || frame.rgb.len() != frame.width * frame.height * 3
                    {
                        return Err("Invalid snapshot dimensions".into());
                    }
                    let file = std::fs::OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(&path)?;
                    let mut encoder =
                        png::Encoder::new(file, frame.width as u32, frame.height as u32);
                    encoder.set_color(png::ColorType::Rgb);
                    encoder.set_depth(png::BitDepth::Eight);
                    let mut writer = encoder.write_header()?;
                    writer.write_image_data(&frame.rgb)?;
                    writer.finish()?;
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(&path)?
                        .sync_all()?;
                    Ok(())
                })();
                if written.is_err() {
                    capture.result = "Snapshot write failed; partial image retained".into();
                    event.captures.push(capture);
                    persist_event(&directory, &event)?;
                    continue;
                }
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
            persist_event(&directory, &event)?;
        }
        event.complete = true;
        persist_event(&directory, &event)?;
        Ok(Saved { event, thumbnails })
    })();
    result.map_err(|_|"Operator event could not be saved; check output permissions and disk space. Partial files retained.".into())
}

fn persist_event(directory: &Path, event: &Event) -> Result<(), Box<dyn std::error::Error>> {
    let mut metadata = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer_pretty(&mut metadata, event)?;
    metadata.write_all(b"\n")?;
    metadata.as_file().sync_all()?;
    metadata
        .persist(directory.join("event.json"))
        .map_err(|e| e.error)?;
    Ok(())
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

#[cfg(test)]
mod failure_tests {
    use super::*;
    #[test]
    fn failed_snapshot_preserves_mark_note_and_other_capture_results() {
        let root = tempfile::tempdir().unwrap();
        let bad = Arc::new(vidfish_media::Frame {
            width: 2,
            height: 2,
            rgb: vec![],
            number: 1,
            received_at: std::time::Instant::now(),
        });
        let good = Arc::new(vidfish_media::Frame {
            rgb: vec![64; 12],
            ..(*bad).clone()
        });
        let saved = save(
            root.path(),
            "Mark + Grab All",
            "critical note".into(),
            5,
            vec![
                Input {
                    camera: "bad".into(),
                    label: "Bad".into(),
                    frame: Some(bad),
                },
                Input {
                    camera: "good".into(),
                    label: "Good".into(),
                    frame: Some(good),
                },
            ],
            true,
        )
        .unwrap();
        let disk: Event = serde_json::from_slice(
            &std::fs::read(root.path().join(&saved.event.id).join("event.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(disk.note, "critical note");
        assert!(disk.complete);
        assert!(disk.captures[0].result.contains("failed"));
        assert!(disk.captures[0].image.is_none());
        assert!(disk.captures[1].image.is_some());
    }
}
