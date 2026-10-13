//! Small recording-policy bridge. Recorder children survive a desktop crash/restart.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;
use vidfish_core::{
    cameras::CameraStore,
    config::{AppConfig, DiskPolicy},
};
use vidfish_onvif::manager::{Snapshot as Cameras, State as CameraState};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Starting,
    Recording,
    Stopping,
    Stopped,
    Failed,
}
impl State {
    pub fn active(self) -> bool {
        matches!(self, Self::Starting | Self::Recording | Self::Stopping)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub camera_id: String,
    pub run_id: String,
    pub state: State,
    pub output: Option<PathBuf>,
    pub bytes_written: u64,
    pub heartbeat_unix_ms: u64,
    pub error: Option<String>,
    #[serde(default)]
    pub warning: Option<String>,
    #[serde(default)]
    pub disk_available_bytes: Option<u64>,
}
#[derive(Clone, Default)]
pub struct Snapshot {
    pub cameras: Vec<Status>,
    pub error: Option<String>,
    pub stop_all_ack: bool,
}
pub enum Control {
    AutoRecord(bool),
    Record(String),
    Stop(String),
    RecordAll,
    StopAll,
}
#[derive(Clone)]
pub struct Controller {
    sender: mpsc::SyncSender<Control>,
}
impl Controller {
    pub fn command(&self, command: Control) -> std::result::Result<(), &'static str> {
        self.sender
            .try_send(command)
            .map_err(|_| "Recording controller is busy or stopped")
    }
}
pub struct Supervisor {
    controller: Controller,
    stop: Arc<AtomicBool>,
    snapshot: watch::Receiver<Snapshot>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Supervisor {
    pub fn start(
        path: PathBuf,
        config: AppConfig,
        cameras: watch::Receiver<Cameras>,
    ) -> Result<Self> {
        Self::start_with_executable(path, config, cameras, std::env::current_exe()?)
    }
    pub fn start_with_executable(
        path: PathBuf,
        config: AppConfig,
        cameras: watch::Receiver<Cameras>,
        executable: PathBuf,
    ) -> Result<Self> {
        let (sender, commands) = mpsc::sync_channel(32);
        let (status, snapshot) = watch::channel(Snapshot::default());
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = thread::Builder::new()
            .name("vidfish-record-policy".into())
            .spawn(move || {
                supervise(
                    path, config, cameras, commands, status, stopping, executable,
                )
            })?;
        Ok(Self {
            controller: Controller { sender },
            stop,
            snapshot,
            worker: Some(worker),
        })
    }
    pub fn snapshot(&self) -> watch::Receiver<Snapshot> {
        self.snapshot.clone()
    }
    pub fn controller(&self) -> Controller {
        self.controller.clone()
    }
    pub fn join(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
    pub fn finish_all(self) -> Result<()> {
        self.controller.command(Control::StopAll)?;
        let deadline = Instant::now() + Duration::from_secs(75);
        while !self.snapshot.borrow().stop_all_ack
            || self
                .snapshot
                .borrow()
                .cameras
                .iter()
                .any(|s| s.state.active())
        {
            if Instant::now() > deadline {
                self.join();
                return Err(
                    "Recording shutdown timed out; files and child processes retained for recovery"
                        .into(),
                );
            }
            thread::sleep(Duration::from_millis(50));
        }
        self.join();
        Ok(())
    }
}
impl Drop for Supervisor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn directory(config: &Path) -> PathBuf {
    config
        .parent()
        .unwrap_or(Path::new("."))
        .join("acquisition")
}
fn status_path(config: &Path, id: &str) -> PathBuf {
    directory(config).join(format!("{id}.json"))
}
fn stop_path(config: &Path, id: &str) -> PathBuf {
    directory(config).join(format!("{id}.stop"))
}
fn lock_path(config: &Path, id: &str) -> PathBuf {
    directory(config).join(format!("{id}.record"))
}
fn write_status(config: &Path, status: &Status) -> Result<()> {
    let directory = directory(config);
    std::fs::create_dir_all(&directory)?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(&mut file, status)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(status_path(config, &status.camera_id))
        .map_err(|e| e.error)?;
    Ok(())
}
fn read_status(config: &Path, id: &str) -> Option<Status> {
    let bytes = std::fs::read(status_path(config, id)).ok()?;
    if bytes.len() > 16384 {
        return None;
    }
    let status: Status = serde_json::from_slice(&bytes).ok()?;
    (status.camera_id == id && uuid::Uuid::parse_str(&status.run_id).is_ok()).then_some(status)
}
fn active(config: &Path, id: &str) -> bool {
    let mut path = lock_path(config, id).into_os_string();
    path.push(".lock");
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .is_ok_and(|file| file.try_lock().is_err())
}
fn stop_recording(config: &Path, status: &Status) -> Result<()> {
    std::fs::create_dir_all(directory(config))?;
    std::fs::write(stop_path(config, &status.camera_id), &status.run_id)?;
    Ok(())
}
struct Running {
    child: Child,
    status: Status,
}
fn supervise(
    path: PathBuf,
    config: AppConfig,
    mut cameras: watch::Receiver<Cameras>,
    commands: mpsc::Receiver<Control>,
    snapshots: watch::Sender<Snapshot>,
    stop: Arc<AtomicBool>,
    executable: PathBuf,
) {
    let mut auto_record = config.auto_record;
    let mut suppressed = BTreeSet::new();
    let mut stop_all = false;
    let mut manual = BTreeSet::new();
    let mut running = BTreeMap::<String, Running>::new();
    let mut retries = BTreeMap::<String, (u32, Instant)>::new();
    let mut statuses = BTreeMap::<String, Status>::new();
    let mut error = None;
    let mut logged = BTreeMap::new();
    while !stop.load(Ordering::Acquire) {
        let devices = cameras.borrow_and_update().cameras.clone();
        for camera in &devices {
            if let Some(status) = read_status(&path, &camera.camera.id) {
                statuses.insert(camera.camera.id.clone(), status);
            }
        }
        for (id, entry) in &mut running {
            match entry.child.try_wait() {
                Ok(Some(exit)) => {
                    let status = statuses
                        .entry(id.clone())
                        .or_insert_with(|| entry.status.clone());
                    if status.run_id == entry.status.run_id
                        && status.state.active()
                        && !active(&path, id)
                    {
                        status.state = if exit.success() {
                            State::Stopped
                        } else {
                            State::Failed
                        };
                        status.error = (!exit.success()).then(|| "Recording process ended; check camera, runtime, log, and output permissions".into());
                    }
                    let failures = retries
                        .get(id)
                        .map_or(1, |(count, _)| count.saturating_add(1));
                    retries.insert(
                        id.clone(),
                        (
                            failures,
                            Instant::now()
                                + Duration::from_secs(
                                    (1u64 << failures.saturating_sub(1).min(6)).min(60),
                                ),
                        ),
                    );
                }
                Ok(None) => {
                    if statuses
                        .get(id)
                        .is_none_or(|s| s.run_id != entry.status.run_id)
                    {
                        statuses.insert(id.clone(), entry.status.clone());
                    }
                }
                Err(_) => error = Some("Cannot inspect recording child process".into()),
            }
        }
        running.retain(|_, entry| !entry.child.try_wait().is_ok_and(|exit| exit.is_some()));
        while let Ok(command) = commands.try_recv() {
            match command {
                Control::AutoRecord(enabled) => auto_record = enabled,
                Control::Record(id) => {
                    suppressed.remove(&id);
                    manual.insert(id.clone());
                    retries.remove(&id);
                }
                Control::RecordAll => {
                    stop_all = false;
                    suppressed.clear();
                    retries.clear();
                    manual.extend(devices.iter().map(|c| c.camera.id.clone()));
                }
                Control::Stop(id) => {
                    suppressed.insert(id.clone());
                    manual.remove(&id);
                    if let Some(status) = statuses.get_mut(&id)
                        && status.state.active()
                    {
                        match stop_recording(&path,status) { Ok(()) => status.state=State::Stopping, Err(_) => error=Some("Cannot request recording stop; check configuration directory permissions".into()) }
                    }
                }
                Control::StopAll => {
                    stop_all = true;
                    manual.clear();
                    for status in statuses.values_mut().filter(|s| s.state.active()) {
                        match stop_recording(&path,status) { Ok(()) => status.state=State::Stopping, Err(_) => error=Some("Cannot request recording stop; check configuration directory permissions".into()) }
                    }
                }
            }
        }
        for camera in &devices {
            let id = &camera.camera.id;
            if !matches!(
                camera.state,
                CameraState::Available | CameraState::ManualConfigured
            ) || running.contains_key(id)
                || active(&path, id)
            {
                continue;
            }
            if suppressed.contains(id)
                || !(manual.contains(id) || auto_record && !stop_all)
                || retries
                    .get(id)
                    .is_some_and(|(_, deadline)| Instant::now() < *deadline)
            {
                continue;
            }
            if running.len()
                + statuses
                    .iter()
                    .filter(|(id, s)| {
                        s.state.active() && active(&path, id) && !running.contains_key(*id)
                    })
                    .count()
                >= config.max_cameras
            {
                continue;
            }
            let run_id = uuid::Uuid::new_v4().to_string();
            let status = Status {
                camera_id: id.clone(),
                run_id: run_id.clone(),
                state: State::Starting,
                output: None,
                bytes_written: 0,
                heartbeat_unix_ms: now(),
                error: None,
                warning: None,
                disk_available_bytes: None,
            };
            let launched = (|| -> Result<Child> {
                std::fs::create_dir_all(directory(&path))?;
                let log = std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(directory(&path).join(format!("{id}-{run_id}.log")))?;
                let mut child = Command::new(&executable);
                child
                    .arg("--record-camera")
                    .arg(id)
                    .arg("--record-run-id")
                    .arg(&run_id)
                    .arg("--config")
                    .arg(&path)
                    .stdin(Stdio::null())
                    .stdout(log.try_clone()?)
                    .stderr(log);
                #[cfg(unix)]
                {
                    use std::os::unix::process::CommandExt;
                    child.process_group(0);
                }
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    child.creation_flags(0x08000000);
                }
                Ok(child.spawn()?)
            })();
            match launched {
                Ok(child) => {
                    statuses.insert(id.clone(), status.clone());
                    running.insert(id.clone(), Running { child, status });
                }
                Err(_) => {
                    let mut failed = status;
                    failed.state = State::Failed;
                    failed.error=Some("Cannot launch recording worker; check executable, runtime, and output permissions".into());
                    statuses.insert(id.clone(), failed);
                    retries.insert(id.clone(), (1, Instant::now() + Duration::from_secs(60)));
                }
            }
        }
        for (id, status) in &mut statuses {
            if status.state.active() && !running.contains_key(id) && !active(&path, id) {
                status.state = State::Failed;
                status.error = Some(
                    "Recording worker is no longer active; retained output may need recovery"
                        .into(),
                );
            }
            if status.state == State::Recording
                && status.bytes_written > 0
                && running
                    .get(id)
                    .is_some_and(|r| now().saturating_sub(r.status.heartbeat_unix_ms) > 10000)
            {
                retries.remove(id);
            }
        }
        for status in statuses.values() {
            let event = (status.run_id.clone(), status.state);
            if logged.get(&status.camera_id) != Some(&event) {
                let result = (|| -> Result<()> {
                    std::fs::create_dir_all(directory(&path))?;
                    let mut file = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(directory(&path).join("events.jsonl"))?;
                    serde_json::to_writer(
                        &mut file,
                        &serde_json::json!({"unix_ms":now(),"camera_id":status.camera_id,"run_id":status.run_id,"state":status.state,"output":status.output,"error":status.error}),
                    )?;
                    file.write_all(b"\n")?;
                    file.sync_data()?;
                    Ok(())
                })();
                if result.is_err() {
                    error=Some("Cannot save acquisition event log; check configuration directory permissions".into());
                }
                logged.insert(status.camera_id.clone(), event);
            }
        }
        snapshots.send_replace(Snapshot {
            cameras: statuses.values().cloned().collect(),
            error: error.clone(),
            stop_all_ack: stop_all,
        });
        thread::sleep(Duration::from_millis(100));
    }
    // Dropping Child handles leaves the independent recorder processes alive.
}

pub fn record_camera(
    config_path: &Path,
    config: &AppConfig,
    id: &str,
    run_id: &str,
    interrupted: &AtomicBool,
) -> Result<()> {
    uuid::Uuid::parse_str(id)?;
    uuid::Uuid::parse_str(run_id)?;
    // A second desktop or restart cannot create a duplicate recorder for this camera.
    let _lock = CameraStore::lock(&lock_path(config_path, id))?;
    let mut status = Status {
        camera_id: id.into(),
        run_id: run_id.into(),
        state: State::Starting,
        output: None,
        bytes_written: 0,
        heartbeat_unix_ms: now(),
        error: None,
        warning: None,
        disk_available_bytes: None,
    };
    if let Some(mut previous) = read_status(config_path, id)
        && previous.state.active()
    {
        previous.error =
            Some("Recording worker ended abruptly; open segment retained for recovery".into());
        record_event(config_path, &previous, "gap_started_after_worker_loss")?;
    }
    write_status(config_path, &status)?;
    record_event(config_path, &status, "attempt")?;
    let result = (|| -> Result<()> {
        let inventory = config_path.with_file_name("cameras.json");
        let store = CameraStore::load(&inventory)?;
        let camera = store
            .cameras
            .iter()
            .find(|c| c.id == id)
            .ok_or("Unknown camera ID")?;
        let resolved = match &camera.device_url {
            Some(address) => {
                let client = match &camera.credential_key {
                    Some(key) => vidfish_onvif::OnvifClient::with_credentials(
                        vidfish_onvif::credentials::load(
                            &vidfish_onvif::credentials::file_path(&inventory),
                            key,
                        )?,
                    )?,
                    None => vidfish_onvif::OnvifClient::new()?,
                };
                Some(client.resolve(address)?)
            }
            None => None,
        };
        if let (Some(expected), Some(actual)) = (&camera.hardware, &resolved)
            && (expected.manufacturer != actual.manufacturer
                || expected.serial_number != actual.serial_number)
        {
            return Err(
                "Camera hardware identity changed; recording blocked until identity is confirmed"
                    .into(),
            );
        }
        let (source, codec) = crate::camera_preview::source(
            camera,
            resolved.as_ref().and_then(|c| c.profiles.first()),
            &inventory,
        )
        .map_err(std::io::Error::other)?;
        let output_root = if config.output_directory.is_absolute() {
            config.output_directory.clone()
        } else {
            std::env::current_dir()?.join(&config.output_directory)
        };
        std::fs::create_dir_all(&output_root)?;
        let available = fs2::available_space(&output_root)?;
        update_disk(&mut status, config, available);
        if config.disk_policy == DiskPolicy::Stop && available <= config.disk_stop_bytes {
            return Err(
                "Disk capacity below configured stop threshold; no recording started".into(),
            );
        }
        let run_directory = output_root.join(format!("camera-{id}-{}-{run_id}", now()));
        let mut recording = vidfish_media::SegmentedRecording::start(
            &source,
            codec,
            &run_directory,
            config.segment_seconds,
        )?;
        recording.wait_started(Duration::from_secs(15))?;
        status.output = recording.current_output();
        status.state = State::Recording;
        record_event(config_path, &status, "recording_started")?;
        status.heartbeat_unix_ms = now();
        write_status(config_path, &status)?;
        let mut heartbeat = Instant::now();
        let mut progress = Instant::now();
        while !interrupted.load(Ordering::Acquire)
            && !std::fs::read_to_string(stop_path(config_path, id))
                .is_ok_and(|request| request.trim() == run_id)
        {
            recording.poll(Duration::from_millis(100))?;
            if heartbeat.elapsed() >= Duration::from_secs(1) {
                let size = recording.bytes_written()?;
                if let Some(output) = recording.current_output() {
                    status.output = Some(output);
                }
                let available = fs2::available_space(&output_root)?;
                let was_low = status.warning.is_some();
                update_disk(&mut status, config, available);
                if status.warning.is_some() && !was_low {
                    record_event(config_path, &status, "disk_low")?;
                }
                if config.disk_policy == DiskPolicy::Stop && available <= config.disk_stop_bytes {
                    status.bytes_written = recording.finish()?;
                    return Err("Disk capacity below configured stop threshold; recording finalized and stopped".into());
                }
                if size > status.bytes_written {
                    progress = Instant::now();
                }
                if progress.elapsed() > Duration::from_secs(config.stall_seconds) {
                    return Err("Recording file made no progress before the configured stall deadline; stream may be stalled".into());
                }
                status.bytes_written = size;
                status.heartbeat_unix_ms = now();
                write_status(config_path, &status)?;
                heartbeat = Instant::now();
            }
        }
        status.state = State::Stopping;
        status.heartbeat_unix_ms = now();
        write_status(config_path, &status)?;
        status.bytes_written = recording.finish()?;
        Ok(())
    })();
    status.heartbeat_unix_ms = now();
    match &result {
        Ok(()) => status.state = State::Stopped,
        Err(error) => {
            status.state = State::Failed;
            status.error = Some(error.to_string());
        }
    }
    record_event(
        config_path,
        &status,
        if result.is_ok() {
            "recording_stopped"
        } else {
            "gap_started"
        },
    )?;
    write_status(config_path, &status)?;
    result
}

fn update_disk(status: &mut Status, config: &AppConfig, available: u64) {
    status.disk_available_bytes = Some(available);
    status.warning = (available <= config.disk_warning_bytes)
        .then(|| "Disk capacity is below the configured warning threshold".into());
}
fn record_event(config: &Path, status: &Status, event: &str) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory(config).join(format!("{}.events.jsonl", status.camera_id)))?;
    serde_json::to_writer(
        &mut file,
        &serde_json::json!({"unix_ms":now(),"event":event,"camera_id":status.camera_id,"run_id":status.run_id,"output":status.output,"error":status.error,"last_heartbeat_unix_ms":status.heartbeat_unix_ms}),
    )?;
    file.write_all(b"\n")?;
    file.sync_data()?;
    Ok(())
}
