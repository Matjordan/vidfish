//! Durable scientific events, per-camera capture results and derived CSV exports.
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
    #[serde(default)]
    pub recording: crate::event_references::Reference,
    #[serde(default)]
    pub capture_unix_ms: Option<u64>,
    #[serde(default)]
    pub delay_ms: Option<u64>,
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
    #[serde(default)]
    pub elapsed_ns: u64,
    #[serde(default)]
    pub elapsed_scope: String,
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
    save_request(
        root,
        Request {
            kind: kind.into(),
            note,
            elapsed_ns: elapsed.saturating_mul(1_000_000_000),
            unix_ms: now(),
            inputs: inputs
                .into_iter()
                .map(|input| CameraInput {
                    input,
                    camera: None,
                    profile: None,
                    recording: Default::default(),
                })
                .collect(),
            inventory: PathBuf::new(),
            grab,
        },
    )
}
pub struct CameraInput {
    pub input: Input,
    pub camera: Option<vidfish_core::cameras::CameraRecord>,
    pub profile: Option<vidfish_onvif::Profile>,
    pub recording: crate::event_references::Reference,
}
pub struct Request {
    pub kind: String,
    pub note: String,
    pub unix_ms: u64,
    pub elapsed_ns: u64,
    pub inputs: Vec<CameraInput>,
    pub inventory: PathBuf,
    pub grab: bool,
}
pub fn save_request(root: &Path, request: Request) -> Result<Saved, String> {
    let Request {
        kind,
        note,
        unix_ms,
        elapsed_ns,
        inputs,
        inventory,
        grab,
    } = request;
    if note.len() > 16384 || inputs.len() > 16 {
        return Err("Operator action exceeds limits".into());
    }
    let result = (|| -> Result<Saved, Box<dyn std::error::Error>> {
        let _lock = lock(root)?;
        let id = uuid::Uuid::new_v4().to_string();
        let directory = root.join(&id);
        std::fs::create_dir(&directory)?;
        let mut event = Event {
            id,
            unix_ms,
            elapsed_seconds: elapsed_ns / 1_000_000_000,
            elapsed_ns,
            elapsed_scope:
                "Viewer monotonic clock; recording clock offsets are separate per camera".into(),
            kind,
            note,
            captures: inputs
                .iter()
                .map(|i| Capture {
                    camera: i.input.camera.clone(),
                    label: i.input.label.clone(),
                    image: None,
                    result: if grab { "Pending capture" } else { "Mark only" }.into(),
                    recording: i.recording.clone(),
                    capture_unix_ms: None,
                    delay_ms: None,
                })
                .collect(),
            complete: false,
        };
        persist_event(&directory, &event)?;
        let mut thumbnails = Vec::new();
        let action_at = std::time::Instant::now();
        let deadline = action_at + std::time::Duration::from_secs(45);
        for (index, item) in inputs.into_iter().enumerate() {
            let mut capture = event.captures[index].clone();
            let mut frame = item.input.frame;
            if grab && let Some(camera) = &item.camera {
                // Fresh, one-shot all-camera capture. Never reuse an old selected frame.
                if std::time::Instant::now() >= deadline {
                    frame = None;
                    capture.result = "Capture batch deadline reached; camera skipped".into();
                } else {
                    match crate::snapshots::capture(camera, item.profile.as_ref(), &inventory) {
                        Ok((fresh, method)) => {
                            frame = Some(fresh);
                            capture.result = method;
                        }
                        Err(error) => {
                            frame = None;
                            capture.result = error;
                        }
                    }
                }
            } else if grab {
                capture.result = "Not selected or no fresh preview".into();
            }
            capture.delay_ms = grab.then(|| now().saturating_sub(unix_ms));
            if grab
                && let Some(frame) = frame.filter(|f| {
                    action_at.saturating_duration_since(f.received_at)
                        < std::time::Duration::from_secs(5)
                })
            {
                let path = directory.join(format!("snapshot-{}.png", index));
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
                    event.captures[index] = capture;
                    persist_event(&directory, &event)?;
                    continue;
                }
                capture.image = Some(path);
                if item.camera.is_none() {
                    capture.result = "Latest displayed frame (not synchronized)".into();
                }
                capture.capture_unix_ms =
                    Some(now().saturating_sub(frame.received_at.elapsed().as_millis() as u64));
                capture.delay_ms = capture.capture_unix_ms.map(|t| t.saturating_sub(unix_ms));
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
                if thumbnails.len() < 4 {
                    thumbnails.push((capture.label.clone(), width, height, rgb));
                }
            }
            capture.recording.resolve();
            event.captures[index] = capture;
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
    persist_csv(directory, event)?;
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

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn lock(root: &Path) -> Result<std::fs::File, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(root)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("events.lock"))?;
    fs2::FileExt::lock_exclusive(&file)?;
    Ok(file)
}
fn persist_csv(directory: &Path, event: &Event) -> Result<(), Box<dyn std::error::Error>> {
    // Quote every field (including newlines/quotes) using RFC 4180 rules.
    fn quote(s: impl ToString) -> String {
        format!("\"{}\"", s.to_string().replace('"', "\"\""))
    }
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    writeln!(
        file,
        "event_id,utc_unix_ms,monotonic_elapsed_ns,elapsed_scope,kind,note,complete,camera_id,alias,run_id,segment,offset_ns,timing,recording_status,snapshot,capture_unix_ms,delay_ms,capture_status"
    )?;
    for c in &event.captures {
        let r = &c.recording;
        let fields = [
            quote(&event.id),
            quote(event.unix_ms),
            quote(event.elapsed_ns),
            quote(&event.elapsed_scope),
            quote(&event.kind),
            quote(&event.note),
            quote(event.complete),
            quote(&c.camera),
            quote(&c.label),
            quote(r.run_id.as_deref().unwrap_or("")),
            quote(
                r.segment
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            ),
            quote(r.offset_ns.map(|t| t.to_string()).unwrap_or_default()),
            quote(&r.timing),
            quote(&r.state),
            quote(
                c.image
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            ),
            quote(c.capture_unix_ms.map(|t| t.to_string()).unwrap_or_default()),
            quote(c.delay_ms.map(|t| t.to_string()).unwrap_or_default()),
            quote(&c.result),
        ];
        writeln!(file, "{}", fields.join(","))?;
    }
    file.as_file().sync_all()?;
    file.persist(directory.join("event.csv"))
        .map_err(|e| e.error)?;
    Ok(())
}
/// Recover canonical JSON, reconcile pending segment boundaries, and regenerate CSV.
/// Incomplete captures stay visibly incomplete; restart never repeats camera requests.
pub fn load(root: &Path) -> Result<Vec<Event>, String> {
    let result = (|| -> Result<_, Box<dyn std::error::Error>> {
        let _lock = lock(root)?;
        let mut paths: Vec<_> = std::fs::read_dir(root)?
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .collect();
        paths.sort_by_key(|e| std::cmp::Reverse(e.metadata().and_then(|m| m.modified()).ok()));
        let mut events = Vec::new();
        for path in paths.into_iter().take(200) {
            let file = path.path().join("event.json");
            if !file.exists() {
                continue;
            }
            if std::fs::metadata(&file)?.len() > 2 * 1024 * 1024 {
                return Err("Event metadata too large".into());
            }
            let mut event: Event = serde_json::from_slice(&std::fs::read(file)?)?;
            for c in &mut event.captures {
                c.recording.resolve();
            }
            persist_event(&path.path(), &event)?;
            events.push(event);
        }
        events.sort_by_key(|e| std::cmp::Reverse(e.unix_ms));
        Ok(events)
    })();
    result.map_err(|_| {
        "Event history could not be read or reconciled; original files retained".into()
    })
}
pub fn edit_note(root: &Path, id: &str, note: String) -> Result<Saved, String> {
    let result = (|| -> Result<_, Box<dyn std::error::Error>> {
        uuid::Uuid::parse_str(id)?;
        if note.len() > 16384 {
            return Err("Note exceeds limit".into());
        }
        let _lock = lock(root)?;
        let directory = root.join(id);
        let mut event: Event =
            serde_json::from_slice(&std::fs::read(directory.join("event.json"))?)?;
        event.note = note;
        for c in &mut event.captures {
            c.recording.resolve();
        }
        persist_event(&directory, &event)?;
        Ok(Saved {
            event,
            thumbnails: Vec::new(),
        })
    })();
    result.map_err(|_| "Note could not be saved; original event retained".into())
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn notes_csv_and_incomplete_events_survive_restart_without_recapture() {
        let root = tempfile::tempdir().unwrap();
        let saved = save(
            root.path(),
            "Mark",
            "original".into(),
            2,
            vec![Input {
                camera: "stable-id".into(),
                label: "Original alias".into(),
                frame: None,
            }],
            false,
        )
        .unwrap();
        let note = "comma, quote \" and\nnew line";
        edit_note(root.path(), &saved.event.id, note.into()).unwrap();
        let dir = root.path().join(&saved.event.id);
        let mut event: Event =
            serde_json::from_slice(&std::fs::read(dir.join("event.json")).unwrap()).unwrap();
        event.complete = false;
        event.captures[0].result = "Pending capture".into();
        persist_event(&dir, &event).unwrap();
        // CSV is a derived export: simulate a crash leaving it absent/stale.
        std::fs::remove_file(dir.join("event.csv")).unwrap();
        let events = load(root.path()).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].note, note);
        assert!(!events[0].complete);
        assert!(events[0].captures[0].image.is_none());
        assert_eq!(events[0].captures[0].camera, "stable-id");
        let csv = std::fs::read_to_string(dir.join("event.csv")).unwrap();
        assert!(csv.contains("\"comma, quote \"\" and\nnew line\""));
        assert!(csv.contains(&saved.event.id));
        assert!(csv.contains("monotonic_elapsed_ns"));
        assert!(edit_note(root.path(), "../../bad", "bad".into()).is_err());
    }
}
