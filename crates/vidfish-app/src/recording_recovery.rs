//! Recovery runs only under the same exclusive camera lock as acquisition.
use crate::{
    library::journal,
    sessions::{Result, Run},
};
use std::{collections::BTreeSet, path::Path};
use vidfish_core::cameras::CameraStore;

/// Scan stopped runs, including files missing from a damaged/absent journal.
/// Originals and event references are immutable; repaired copies have sidecars.
pub fn recover_run(config: &Path, run: &Run) -> Result<Vec<String>> {
    let lock = config
        .with_file_name("acquisition")
        .join(format!("{}.record", run.camera_id));
    let Ok(_guard) = CameraStore::lock(&lock) else {
        return Ok(Vec::new());
    };
    recover_stopped_run(run)
}

/// Caller must hold the camera's acquisition lock for the entire operation.
pub(crate) fn recover_stopped_run(run: &Run) -> Result<Vec<String>> {
    let session = run
        .directory
        .parent()
        .and_then(Path::parent)
        .ok_or("Recording run has no session directory")?;
    let _lease = crate::sessions::lease_directory(session)?;
    let closed: BTreeSet<_> = journal(&run.directory.join("segments.jsonl"))
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.event == "closed")
        .map(|r| r.location)
        .collect();
    let mut warnings = Vec::new();
    let mut attempted = 0;
    let Ok(entries) = std::fs::read_dir(&run.directory) else {
        return Ok(warnings);
    };
    for entry in entries.filter_map(std::result::Result::ok).take(100_000) {
        let path = entry.path();
        if !entry.file_type()?.is_file()
            || !path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("segment-"))
            || !path.extension().is_some_and(|s| s == "mkv" || s == "mp4")
            || path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().contains(".recovered-"))
            || closed.contains(&path)
        {
            continue;
        }
        let report = path.with_extension(format!(
            "{}.recovery.json",
            path.extension().unwrap().to_string_lossy()
        ));
        if report.exists() {
            // Recovery failures remain visible and are not retried on every browse.
            let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&report)?)?;
            warnings.push(format!(
                "Recovery {}: {} (original retained)",
                value["state"].as_str().unwrap_or("unknown"),
                path.display()
            ));
            continue;
        }
        // Bound work per scan; the next refresh handles remaining orphan files.
        if attempted >= 2 {
            warnings.push("More orphan segments remain; refresh to continue recovery".into());
            break;
        }
        if fs2::available_space(&run.directory)?
            < entry.metadata()?.len().saturating_add(64 * 1024 * 1024)
        {
            warnings.push(format!(
                "Recovery needs free disk space: {} (original retained)",
                path.display()
            ));
            continue;
        }
        let output = path.with_file_name(format!(
            "{}.recovered-{}.mkv",
            path.file_stem().unwrap().to_string_lossy(),
            uuid::Uuid::new_v4()
        ));
        let codec = if run.codec == "H265" {
            vidfish_media::Codec::H265
        } else {
            vidfish_media::Codec::H264
        };
        attempted += 1;
        // Do not expose a "recovered" filename until muxing and validation finish.
        // A kill during repair can leave only a hidden partial, never a good-looking copy.
        let pending = run
            .directory
            .join(format!(".recovery-{}.partial", uuid::Uuid::new_v4()));
        let result =
            vidfish_media::recording_tools::remux(&path, &pending, codec, "mkv").and_then(|()| {
                tempfile::TempPath::try_from_path(pending)
                    .and_then(|file| file.persist_noclobber(&output).map_err(|error| error.error))
                    .map_err(|_| {
                        vidfish_media::MediaError("cannot publish verified recovered copy".into())
                    })
            });
        let state = if result.is_ok() {
            "recovered-copy"
        } else {
            "incomplete/recovery-needed"
        };
        let error = result.err().map(|e| e.to_string());
        let mut file = tempfile::NamedTempFile::new_in(&run.directory)?;
        serde_json::to_writer_pretty(
            &mut file,
            &serde_json::json!({"state":state,"original":path,"recovered":output,"error":error}),
        )?;
        file.as_file().sync_all()?;
        file.persist(&report).map_err(|e| e.error)?;
        tracing::warn!(file = %path.display(), state, error, "orphan recording recovery");
        warnings.push(format!(
            "Recovery {state}: {} (original retained)",
            path.display()
        ));
    }
    Ok(warnings)
}
