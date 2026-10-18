//! UTC-based public IDs remain sortable across local DST transitions.
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Timestamp {
    pub utc: String,
    pub local: String,
    pub offset_seconds: i32,
}
pub fn timestamp(ms: u64) -> Timestamp {
    let utc =
        DateTime::<Utc>::from_timestamp_millis(ms.min(i64::MAX as u64) as i64).unwrap_or_default();
    let local = utc.with_timezone(&Local);
    Timestamp {
        utc: utc.to_rfc3339(),
        local: local.to_rfc3339(),
        offset_seconds: local.offset().local_minus_utc(),
    }
}
pub fn display(ms: u64) -> String {
    timestamp(ms).local
}
pub fn public_id(id: &str, ms: u64) -> String {
    if uuid::Uuid::parse_str(id).is_ok() {
        let utc = DateTime::<Utc>::from_timestamp_millis(ms.min(i64::MAX as u64) as i64)
            .unwrap_or_default();
        format!(
            "{}-{}",
            utc.format("%Y%m%d-%H%M%S"),
            id.chars().take(6).collect::<String>()
        )
    } else {
        id.into()
    }
}
pub fn validate(id: &str) -> Result<(), &'static str> {
    if uuid::Uuid::parse_str(id).is_ok() {
        return Ok(());
    }
    let parts: Vec<_> = id.split('-').collect();
    if !(2..=4).contains(&parts.len())
        || parts[0].len() != 8
        || parts[1].len() != 6
        || !parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        || chrono::NaiveDateTime::parse_from_str(
            &format!("{}-{}", parts[0], parts[1]),
            "%Y%m%d-%H%M%S",
        )
        .is_err()
    {
        return Err("Invalid session/event identifier");
    }
    Ok(())
}
/// create_dir is the cross-process reservation; never merge or reuse a folder.
pub fn reserve(
    root: &Path,
    ms: u64,
    millis: bool,
    prefix: &str,
) -> std::io::Result<(String, PathBuf)> {
    let time =
        DateTime::<Utc>::from_timestamp_millis(ms.min(i64::MAX as u64) as i64).unwrap_or_default();
    let mut base = time.format("%Y%m%d-%H%M%S").to_string();
    if millis {
        base.push_str(&format!("-{:03}", ms % 1000));
    }
    for n in 1..=100_000 {
        let id = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n:02}")
        };
        let path = root.join(format!("{prefix}{id}"));
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok((id, path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("Identifier collision limit reached"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collisions_rollover_restart_and_legacy_paths() {
        let root = tempfile::tempdir().unwrap();
        let a = reserve(root.path(), 86_399_999, true, "").unwrap();
        let b = reserve(root.path(), 86_399_999, true, "").unwrap();
        assert_eq!(a.0, "19700101-235959-999");
        assert_eq!(b.0, "19700101-235959-999-02");
        assert_eq!(
            reserve(root.path(), 86_400_000, true, "").unwrap().0,
            "19700102-000000-000"
        );
        assert_eq!(
            reserve(root.path(), 86_399_999, true, "").unwrap().0,
            "19700101-235959-999-03"
        );
        for id in [&a.0, &b.0, &uuid::Uuid::new_v4().to_string()] {
            validate(id).unwrap();
        }
        assert!(validate("../../escape").is_err());
        assert!(validate("20261340-999999").is_err());
        assert!(timestamp(0).utc.ends_with("+00:00"));
    }
}
