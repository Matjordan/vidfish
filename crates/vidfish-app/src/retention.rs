//! Automatic deletion is disabled by default. Only registered, closed, old,
//! unused session folders can be removed; legacy media is never a candidate.
use crate::sessions::{self, Result};
use serde::{Deserialize, Serialize};
use std::{io::Write, path::Path};
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Report {
    pub deleted: Vec<String>,
    pub skipped: Vec<String>,
}
pub fn apply(config: &Path, settings: &vidfish_core::config::AppConfig) -> Result<Report> {
    let Some(days) = settings.retention_days else {
        return Ok(Report::default());
    };
    settings.validate()?;
    let registry_guard = sessions::lock_registry(config)?;
    let mut registry = sessions::registry_unlocked(config)?;
    let cutoff = crate::operator_events::now().saturating_sub(u64::from(days) * 86400 * 1000);
    let mut report = Report::default();
    for entry in &registry.sessions {
        if registry.active.as_ref() == Some(&entry.id) {
            continue;
        }
        let session = match sessions::read(&entry.directory) {
            Ok(s) => s,
            Err(_) => {
                report
                    .skipped
                    .push(format!("{}: missing/unreadable manifest", entry.id));
                continue;
            }
        };
        if session.closed_unix_ms.is_none_or(|t| t > cutoff) {
            continue;
        }
        // Refuse roots, symlinks, mismatched IDs and directories outside the recorded
        // storage root. The default policy never considers unregistered folders.
        let safe = session.id == entry.id
            && entry.directory.file_name().is_some_and(|n| {
                let n = n.to_string_lossy();
                n.starts_with("session-") && n.ends_with(&session.id)
            })
            && entry.directory.parent() == Some(session.conditions.output_directory.as_path())
            && std::fs::symlink_metadata(&entry.directory)
                .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink());
        if !safe {
            report.skipped.push(format!(
                "{}: unsafe or mismatched session location",
                entry.id
            ));
            continue;
        }
        let guard = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(entry.directory.join("session.lease"))?;
        if fs2::FileExt::try_lock_exclusive(&guard).is_err() {
            report.skipped.push(format!(
                "{}: recording/capture/export lease active",
                entry.id
            ));
            continue;
        }
        // Preserve metadata and an intent record before any deletion. If cleanup
        // fails, report it; never hide partial deletion as a successful export/archive.
        let audit_dir = config.with_file_name("retention-audit");
        std::fs::create_dir_all(&audit_dir)?;
        sessions::write_json(&audit_dir.join(format!("{}.json", session.id)), &session)?;
        let mut audit = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(audit_dir.join("actions.jsonl"))?;
        serde_json::to_writer(
            &mut audit,
            &serde_json::json!({"unix_ms":crate::operator_events::now(),"session":session.id,"action":"delete_intent","retention_days":days}),
        )?;
        audit.write_all(b"\n")?;
        audit.sync_all()?;
        match std::fs::remove_dir_all(&entry.directory) {
            Ok(()) => {
                report.deleted.push(entry.id.clone());
                serde_json::to_writer(
                    &mut audit,
                    &serde_json::json!({"unix_ms":crate::operator_events::now(),"session":entry.id,"action":"deleted"}),
                )?;
                audit.write_all(b"\n")?;
                audit.sync_all()?;
            }
            Err(error) => report
                .skipped
                .push(format!("{}: cleanup failed: {error}", entry.id)),
        }
    }
    registry
        .sessions
        .retain(|e| !report.deleted.contains(&e.id));
    sessions::write_json(&config.with_file_name("sessions.json"), &registry)?;
    drop(registry_guard);
    Ok(report)
}
