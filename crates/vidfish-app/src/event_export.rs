//! Collision-safe portable event bundles. Open media is never copied as complete.
use crate::{
    operator_events::Event,
    sessions::{self, Result},
};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};
#[derive(Clone, Serialize, Deserialize)]
pub struct Item {
    pub source: Option<PathBuf>,
    pub relative: PathBuf,
    pub status: String,
    pub bytes: u64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Report {
    pub id: String,
    pub event_id: String,
    pub created_unix_ms: u64,
    pub complete: bool,
    pub items: Vec<Item>,
    pub directory: PathBuf,
}
fn item(
    report: &mut Report,
    source: Option<&Path>,
    relative: PathBuf,
    reason: Option<&str>,
    reserve: u64,
) -> Result<()> {
    let mut entry = Item {
        source: source.map(Path::to_path_buf),
        relative: relative.clone(),
        status: String::new(),
        bytes: 0,
    };
    if let Some(reason) = reason {
        entry.status = reason.into();
    } else if let Some(source) = source {
        let copied = (|| -> Result<u64> {
            let mut input = std::fs::File::open(source)?;
            let size = input.metadata()?.len();
            let parent = report
                .directory
                .join(&relative)
                .parent()
                .ok_or("Export path has no parent")?
                .to_path_buf();
            std::fs::create_dir_all(&parent)?;
            if fs2::available_space(&parent)?.saturating_sub(reserve) < size {
                return Err("Insufficient export storage above reserved warning margin".into());
            }

            let mut output = tempfile::NamedTempFile::new_in(&parent)?;
            let mut remaining = size;
            let mut copied = 0;
            let mut buffer = vec![0u8; 1024 * 1024];
            while remaining > 0 {
                let limit = remaining.min(buffer.len() as u64) as usize;
                let count = input.read(&mut buffer[..limit])?;
                if count == 0 {
                    break;
                }
                if fs2::available_space(&parent)?.saturating_sub(reserve) < count as u64 {
                    return Err(
                        "Export stopped at reserved free-space margin; source media preserved"
                            .into(),
                    );
                }
                output.write_all(&buffer[..count])?;
                copied += count as u64;
                remaining -= count as u64;
            }
            output.as_file().sync_all()?;
            if copied != size {
                return Err("Source size changed during export; retry when stable".into());
            }
            output
                .persist_noclobber(report.directory.join(&relative))
                .map_err(|e| e.error)?;
            Ok(copied)
        })();
        match copied {
            Ok(bytes) => {
                entry.bytes = bytes;
                entry.status = "Copied".into();
            }
            Err(error) => entry.status = format!("Missing or copy failed: {error}"),
        }
    } else {
        entry.status = "No source reference / recording gap".into();
    }
    report.items.push(entry);
    sessions::write_json(&report.directory.join("export.json"), report)?;
    Ok(())
}
pub fn export(event_directory: &Path, destination: &Path, reserve: u64) -> Result<Report> {
    // A session lease also prevents retention during export of an older session.
    let session_directory = event_directory
        .parent()
        .and_then(Path::parent)
        .filter(|p| p.join("manifest.json").exists());
    let _session_lease = session_directory
        .map(sessions::lease_directory)
        .transpose()?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(event_directory.join("events.lock"))?;
    fs2::FileExt::lock_shared(&lock)?;
    let bytes = std::fs::read(event_directory.join("event.json"))?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("Event metadata too large".into());
    }
    let mut event: Event = serde_json::from_slice(&bytes)?;
    crate::identifiers::validate(&event.id)?;
    for c in &mut event.captures {
        c.recording.resolve();
    }
    std::fs::create_dir_all(destination)?;
    let destination = destination.canonicalize()?;
    // Do not put an export inside a source event/session (including symlink aliases).
    if destination.starts_with(event_directory.canonicalize()?)
        || session_directory.is_some_and(|p| destination.starts_with(p))
    {
        return Err("Choose an export directory outside the source session".into());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let directory = destination.join(format!("event-{}-{id}", event.id));
    std::fs::create_dir(&directory)?;
    let mut report = Report {
        id,
        event_id: event.id.clone(),
        created_unix_ms: crate::operator_events::now(),
        complete: false,
        items: Vec::new(),
        directory,
    };
    sessions::write_json(&report.directory.join("export.json"), &report)?;
    item(
        &mut report,
        Some(&event_directory.join("event.json")),
        "source/event.json".into(),
        None,
        reserve,
    )?;
    item(
        &mut report,
        Some(&event_directory.join("event.csv")),
        "source/event.csv".into(),
        None,
        reserve,
    )?;
    if let Some(root) = session_directory {
        item(
            &mut report,
            Some(&root.join("manifest.json")),
            "source/session.json".into(),
            None,
            reserve,
        )?;
    }
    let mut index_targets = std::collections::BTreeMap::new();
    for (i, capture) in event.captures.iter_mut().enumerate() {
        let image = capture.image.clone();
        let image_relative = PathBuf::from(format!("snapshots/camera-{i}.png"));
        let image_allowed = image.as_ref().is_some_and(|p| {
            p.canonicalize()
                .is_ok_and(|p| p.starts_with(event_directory.canonicalize().unwrap_or_default()))
        });
        if image.is_some() {
            item(
                &mut report,
                image.as_deref(),
                image_relative.clone(),
                if image_allowed {
                    None
                } else {
                    Some("Snapshot missing or outside event directory")
                },
                reserve,
            )?;
            capture.image = Some(image_relative);
        }
        let reference = &mut capture.recording;
        let segment = reference.segment.clone();
        let journal = reference.journal.clone();
        let media_root = PathBuf::from(format!("recordings/camera-{i}"));
        if let Some(index) = &journal {
            item(
                &mut report,
                Some(index),
                format!("source/indices/camera-{i}.jsonl").into(),
                None,
                reserve,
            )?;
            index_targets.insert(media_root.join("segments.jsonl"), segment.clone());
            item(
                &mut report,
                Some(index),
                media_root.join("segments.jsonl"),
                None,
                reserve,
            )?;
            reference.journal = Some(media_root.join("segments.jsonl"));
        }
        let relative = media_root.join(
            segment
                .as_ref()
                .and_then(|p| p.file_name())
                .unwrap_or_else(|| std::ffi::OsStr::new("missing.mkv")),
        );
        let closed = match (&segment, &journal) {
            (Some(path), Some(index)) => crate::library::journal(index).is_ok_and(|rows| {
                rows.iter()
                    .any(|row| row.event == "closed" && &row.location == path)
            }),
            _ => false,
        };
        let inside = match (&segment, &journal) {
            (Some(path), Some(index)) => {
                path.parent() == index.parent() && path.extension().is_some_and(|e| e == "mkv")
            }
            _ => false,
        };
        item(
            &mut report,
            segment.as_deref(),
            relative.clone(),
            if segment.is_none() {
                Some("No recording reference / gap")
            } else if !inside {
                Some("Recording reference outside indexed run")
            } else if !closed {
                Some("Open or unverified segment not copied; retry after close")
            } else {
                None
            },
            reserve,
        )?;
        if segment.is_some() {
            reference.segment = Some(relative);
        }
        if let (Some(root), Some(run)) = (session_directory, reference.run_id.as_ref()) {
            uuid::Uuid::parse_str(&capture.camera)?;
            uuid::Uuid::parse_str(run)?;
            let log = root
                .join("logs")
                .join(format!("{}-{run}.jsonl", capture.camera));
            item(
                &mut report,
                Some(&log),
                PathBuf::from(format!("logs/camera-{i}.jsonl")),
                None,
                reserve,
            )?;
        }
    }
    if let Some(lease) = &_session_lease {
        let mut manifest = lease.session.clone();
        manifest.directory = ".".into();
        manifest.conditions.output_directory = ".".into();
        manifest.runs.retain(|r| {
            event
                .captures
                .iter()
                .any(|c| c.recording.run_id.as_ref() == Some(&r.id))
        });
        for run in &mut manifest.runs {
            if let Some(index) = event
                .captures
                .iter()
                .position(|c| c.recording.run_id.as_ref() == Some(&run.id))
            {
                run.directory = format!("recordings/camera-{index}").into();
                run.conditions.output_directory = ".".into();
            }
        }
        sessions::write_json(&report.directory.join("session.json"), &manifest)?;
    }
    // Segment journals are rewritten so locations resolve relative to this bundle.
    for entry in report.items.iter_mut().filter(|e| {
        e.status == "Copied"
            && e.relative
                .file_name()
                .is_some_and(|n| n == "segments.jsonl")
    }) {
        let path = report.directory.join(&entry.relative);
        let mut rows = match crate::library::journal(&path) {
            Ok(rows) => rows,
            Err(_) => {
                entry.status = "Damaged index copied unchanged; portability unresolved".into();
                continue;
            }
        };
        let mut text = Vec::new();
        rows.retain(|row| {
            index_targets.get(&entry.relative).and_then(|p| p.as_ref()) == Some(&row.location)
        });
        for row in &mut rows {
            row.location = entry.relative.parent().unwrap_or(Path::new(".")).join(
                row.location
                    .file_name()
                    .unwrap_or_else(|| std::ffi::OsStr::new("missing.mkv")),
            );
            serde_json::to_writer(&mut text, &row)?;
            text.write_all(b"\n")?;
        }
        let mut f = tempfile::NamedTempFile::new_in(path.parent().ok_or("Index parent missing")?)?;
        f.write_all(&text)?;
        f.as_file().sync_all()?;
        f.persist(&path).map_err(|e| e.error)?;
        entry.bytes = std::fs::metadata(&path)?.len();
    }
    crate::operator_events::persist_event(&report.directory, &event)?;
    report.complete = event.complete && report.items.iter().all(|e| e.status == "Copied");
    sessions::write_json(&report.directory.join("export.json"), &report)?;
    Ok(report)
}
