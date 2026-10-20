//! Catalog work and locked orphan recovery run off the UI/acquisition threads.
use crate::{
    operator_events::Event,
    sessions::{self, Result, Session},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub session: Option<String>,
    pub camera: Option<String>,
    pub from: String,
    pub to: String,
    pub event: String,
}
#[derive(Clone, Serialize)]
pub struct EventRow {
    pub directory: PathBuf,
    pub event: Event,
    pub recording_present: Vec<bool>,
    pub snapshots_present: Vec<bool>,
}
#[derive(Clone, Serialize)]
pub struct Segment {
    pub session_id: String,
    pub camera_id: String,
    pub alias: String,
    pub run_id: String,
    pub path: PathBuf,
    pub start_ns: Option<u64>,
    pub end_ns: Option<u64>,
    pub unix_ms: u64,
    pub exists: bool,
    pub bytes: u64,
}
#[derive(Clone, Serialize)]
pub struct Summary {
    pub id: String,
    pub label: String,
    pub directory: PathBuf,
    pub created_unix_ms: u64,
    pub closed_unix_ms: Option<u64>,
}
impl From<&Session> for Summary {
    fn from(s: &Session) -> Self {
        Self {
            id: s.id.clone(),
            label: s.label.clone(),
            directory: s.directory.clone(),
            created_unix_ms: s.created_unix_ms,
            closed_unix_ms: s.closed_unix_ms,
        }
    }
}
#[derive(Default, Serialize)]
pub struct Catalog {
    pub sessions: Vec<Summary>,
    #[serde(skip)]
    pub view_lease: Option<sessions::Lease>,
    pub events: Vec<EventRow>,
    pub segments: Vec<Segment>,
    pub cameras: Vec<(String, String)>,
    pub warnings: Vec<String>,
    pub truncated: bool,
}
pub fn date(ms: u64) -> String {
    time::OffsetDateTime::from_unix_timestamp((ms / 1000) as i64)
        .map(|t| t.date().to_string())
        .unwrap_or_else(|_| "Unknown date".into())
}
fn valid_date(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    let parts: Vec<_> = s.split('-').collect();
    if s.len() != 10 || parts.len() != 3 {
        return false;
    }
    let parsed = (|| {
        Some((
            parts[0].parse::<i32>().ok()?,
            time::Month::try_from(parts[1].parse::<u8>().ok()?).ok()?,
            parts[2].parse::<u8>().ok()?,
        ))
    })();
    parsed.is_some_and(|(y, m, d)| time::Date::from_calendar_date(y, m, d).is_ok())
}
impl Filter {
    pub fn validate(&self) -> Result<()> {
        if !valid_date(&self.from)
            || !valid_date(&self.to)
            || (!self.from.is_empty() && !self.to.is_empty() && self.from > self.to)
        {
            return Err("Use UTC dates YYYY-MM-DD with start before end".into());
        }
        Ok(())
    }
    fn includes(&self, ms: u64) -> bool {
        let d = date(ms);
        (self.from.is_empty() || d >= self.from) && (self.to.is_empty() || d <= self.to)
    }
}
pub fn journal(path: &Path) -> Result<Vec<vidfish_media::segmented::FragmentEvent>> {
    let f = std::fs::File::open(path)?;
    let mut rows = Vec::new();
    let mut lines = BufReader::new(f).lines().take(100_001).peekable();
    while let Some(line) = lines.next() {
        if rows.len() == 100_000 {
            return Err("Segment index limit reached; narrow/archive the recording run".into());
        }
        let line = line?;
        if !line.trim().is_empty() {
            match serde_json::from_str(&line) {
                Ok(row) => rows.push(row),
                Err(_) if lines.peek().is_none() => break,
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(rows)
}
fn event_rows(root: &Path, filter: &Filter, catalog: &mut Catalog) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    let mut directories: Vec<_> = std::fs::read_dir(root)?
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .collect();
    directories.sort_by_key(|e| std::cmp::Reverse(e.metadata().and_then(|m| m.modified()).ok()));
    for entry in directories {
        if catalog.events.len() >= 2000 {
            catalog.truncated = true;
            break;
        }
        let directory = entry.path();
        let path = directory.join("event.json");
        if !path.exists() {
            continue;
        }
        if std::fs::metadata(&path)?.len() > 2 * 1024 * 1024 {
            catalog
                .warnings
                .push(format!("Oversized event metadata: {}", path.display()));
            continue;
        }
        let event: Event = match serde_json::from_slice(&std::fs::read(&path)?) {
            Ok(e) => e,
            Err(_) => {
                catalog
                    .warnings
                    .push(format!("Unreadable event metadata: {}", path.display()));
                continue;
            }
        };
        if !filter.includes(event.unix_ms)
            || filter
                .camera
                .as_ref()
                .is_some_and(|id| !event.captures.iter().any(|c| &c.camera == id))
        {
            continue;
        }
        if !filter.event.is_empty()
            && !format!("{} {} {}", event.id, event.kind, event.note)
                .to_lowercase()
                .contains(&filter.event.to_lowercase())
        {
            continue;
        }
        // Resolve against actual journals on a copy; browsing never mutates event data.
        let mut event = event;
        for c in &mut event.captures {
            c.recording.resolve();
        }
        let recording_present = event
            .captures
            .iter()
            .map(|c| c.recording.segment.as_ref().is_some_and(|p| p.is_file()))
            .collect();
        let snapshots_present = event
            .captures
            .iter()
            .map(|c| c.image.as_ref().is_some_and(|p| p.is_file()))
            .collect();
        catalog.events.push(EventRow {
            directory,
            event,
            recording_present,
            snapshots_present,
        });
    }
    Ok(())
}
pub fn load(config: &Path, filter: &Filter) -> Result<Catalog> {
    filter.validate()?;
    let mut catalog = Catalog::default();
    let mut cameras = BTreeMap::new();
    for entry in sessions::registry(config)?.sessions {
        let session = match sessions::read(&entry.directory) {
            Ok(session) => session,
            Err(_) => {
                catalog.warnings.push(format!(
                    "Session missing or unreadable: {}",
                    entry.directory.display()
                ));
                continue;
            }
        };
        catalog.sessions.push(Summary::from(&session));
        for run in &session.runs {
            cameras.insert(run.camera_id.clone(), run.alias.clone());
        }
        if filter.session.as_ref().is_some_and(|id| id != &session.id) {
            continue;
        }
        let _lease = match sessions::lease_directory(&session.directory) {
            Ok(lease) => lease,
            Err(_) => {
                catalog.warnings.push(format!(
                    "Session unavailable: {}",
                    session.directory.display()
                ));
                continue;
            }
        };
        event_rows(&session.directory.join("events"), filter, &mut catalog)?;
        for run in &session.runs {
            if filter
                .camera
                .as_ref()
                .is_some_and(|id| id != &run.camera_id)
            {
                continue;
            }
            match crate::recording_recovery::recover_run(config, run) {
                Ok(warnings) => catalog.warnings.extend(warnings),
                Err(error) => catalog
                    .warnings
                    .push(format!("Recording recovery could not complete: {error}")),
            }
            let index = run.directory.join("segments.jsonl");
            let rows = match journal(&index) {
                Ok(rows) => rows,
                Err(_) => {
                    catalog.warnings.push(format!(
                        "Missing or damaged segment index: {}",
                        index.display()
                    ));
                    Vec::new()
                }
            };
            let mut files = BTreeMap::new();
            for row in rows {
                let segment = files.entry(row.location.clone()).or_insert(Segment {
                    session_id: session.id.clone(),
                    camera_id: run.camera_id.clone(),
                    alias: run.alias.clone(),
                    run_id: run.id.clone(),
                    path: row.location,
                    start_ns: None,
                    end_ns: None,
                    unix_ms: row.observed_unix_ms,
                    exists: false,
                    bytes: 0,
                });
                if row.event == "opened" {
                    segment.start_ns = Some(row.running_time_ns);
                    segment.unix_ms = row.observed_unix_ms;
                } else if row.event == "closed" {
                    segment.end_ns = Some(row.running_time_ns);
                }
            }
            if let Ok(entries) = std::fs::read_dir(&run.directory) {
                for file in entries.filter_map(std::result::Result::ok).filter(|e| {
                    e.path()
                        .extension()
                        .is_some_and(|e| e == "mkv" || e == "mp4")
                }) {
                    files.entry(file.path()).or_insert(Segment {
                        session_id: session.id.clone(),
                        camera_id: run.camera_id.clone(),
                        alias: run.alias.clone(),
                        run_id: run.id.clone(),
                        path: file.path(),
                        start_ns: None,
                        end_ns: None,
                        unix_ms: run.attempt_unix_ms,
                        exists: false,
                        bytes: 0,
                    });
                }
            }
            for mut segment in files.into_values() {
                if !filter.includes(segment.unix_ms) {
                    continue;
                }
                if catalog.segments.len() >= 2000 {
                    catalog.truncated = true;
                    break;
                }
                if let Ok(meta) = std::fs::metadata(&segment.path) {
                    segment.exists = meta.is_file();
                    segment.bytes = meta.len();
                }
                catalog.segments.push(segment);
            }
        }
    }
    if filter.session.as_deref().is_none_or(|id| id == "legacy") {
        event_rows(
            &config.with_file_name("operator-events"),
            filter,
            &mut catalog,
        )?;
    }
    for row in &catalog.events {
        for c in &row.event.captures {
            cameras.entry(c.camera.clone()).or_insert(c.label.clone());
        }
    }
    if let Ok(store) =
        vidfish_core::cameras::CameraStore::load(&config.with_file_name("cameras.json"))
    {
        for c in store.cameras {
            cameras.insert(c.id, c.alias);
        }
    }
    catalog
        .sessions
        .sort_by_key(|s| std::cmp::Reverse(s.created_unix_ms));
    catalog.cameras = cameras.into_iter().collect();
    catalog
        .cameras
        .sort_by_key(|(_, alias)| alias.to_lowercase());
    catalog
        .events
        .sort_by_key(|e| std::cmp::Reverse(e.event.unix_ms));
    catalog
        .segments
        .sort_by_key(|s| std::cmp::Reverse(s.unix_ms));
    Ok(catalog)
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Storage {
    pub directory: PathBuf,
    pub free_bytes: Option<u64>,
    pub bytes_per_second: Option<f64>,
    pub seconds_to_warning: Option<u64>,
    pub warning: Option<String>,
}
pub fn storage(config: &Path, settings: &vidfish_core::config::AppConfig) -> Result<Storage> {
    let directory = sessions::current(config)?
        .map(|s| s.directory)
        .unwrap_or_else(|| settings.output_directory.clone());
    let mut report = Storage {
        directory: directory.clone(),
        ..Default::default()
    };
    report.free_bytes = fs2::available_space(&directory).ok();
    let now = crate::operator_events::now();
    let mut rate = 0.0;
    if let Ok(entries) = std::fs::read_dir(config.with_file_name("acquisition")) {
        for file in entries
            .filter_map(std::result::Result::ok)
            .filter(|e| e.path().extension().is_some_and(|e| e == "json"))
        {
            if file.metadata().is_ok_and(|m| m.len() > 16384) {
                continue;
            }
            if let Ok(bytes) = std::fs::read(file.path())
                && let Ok(status) = serde_json::from_slice::<crate::acquisition::Status>(&bytes)
                && status.session_directory.as_ref() == Some(&directory)
                && status.state == crate::acquisition::State::Recording
                && now.saturating_sub(status.heartbeat_unix_ms) < 3000
                && status.started_unix_ms > 0
            {
                let elapsed = now.saturating_sub(status.started_unix_ms) as f64 / 1000.0;
                if elapsed >= 1.0 {
                    rate += status.bytes_written as f64 / elapsed;
                }
            }
        }
    }
    if rate > 0.0 {
        report.bytes_per_second = Some(rate);
        report.seconds_to_warning = report
            .free_bytes
            .map(|free| (free.saturating_sub(settings.disk_warning_bytes) as f64 / rate) as u64);
    }
    report.warning = match report.free_bytes {
        None => Some("Output storage unavailable; check path and permissions".into()),
        Some(free) if free <= settings.disk_warning_bytes => {
            Some("Output storage below configured warning threshold".into())
        }
        _ => None,
    };
    Ok(report)
}
/// Prefer VLC for an estimated seek; use the OS handler with a manual seek
/// instruction when VLC is unavailable. All arguments bypass shell parsing.
pub fn open_file_at(path: &Path, offset_ns: Option<u64>) -> Result<String> {
    if !path.is_file() {
        return Err("Referenced file is missing".into());
    }
    let path = path.canonicalize()?;
    let vlc_name = if cfg!(windows) { "vlc.exe" } else { "vlc" };
    let mut vlc = None;
    if let Some(search) = std::env::var_os("PATH") {
        vlc = std::env::split_paths(&search)
            .map(|d| d.join(vlc_name))
            .find(|p| p.is_file());
    }
    #[cfg(windows)]
    if vlc.is_none() {
        for name in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(root) = std::env::var_os(name) {
                let candidate = PathBuf::from(root).join("VideoLAN/VLC/vlc.exe");
                if candidate.is_file() {
                    vlc = Some(candidate);
                    break;
                }
            }
        }
    }
    if let (Some(player), Some(offset)) = (vlc, offset_ns) {
        std::process::Command::new(player)
            .arg(format!("--start-time={:.3}", offset as f64 / 1e9))
            .arg(&path)
            .spawn()?;
        return Ok(format!(
            "Opened {} in VLC at estimated {:.3}s",
            path.display(),
            offset as f64 / 1e9
        ));
    }
    #[cfg(windows)]
    let mut command = std::process::Command::new("explorer.exe");
    #[cfg(not(windows))]
    let mut command = std::process::Command::new("xdg-open");
    let status = command.arg(&path).status()?;
    if !status.success() {
        return Err(
            "File handler failed; install a video player or open the displayed path manually"
                .into(),
        );
    }
    Ok(format!(
        "Opened {}. Seek manually to the displayed estimated time.",
        path.display()
    ))
}
