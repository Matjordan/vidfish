//! Independent network workers with bounded channels and retry timers.
use crate::{Camera, Error, Profile, credentials, discovery};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddrV4},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;
use vidfish_core::cameras::{CameraRecord, CameraStore, HardwareIdentity};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum State {
    Connecting,
    Available,
    ManualConfigured,
    Reconnecting,
    Offline,
    IdentityConflict,
    AuthenticationRequired,
    Stopped,
}
#[derive(Clone, Serialize)]
pub struct CameraStatus {
    pub camera: CameraRecord,
    pub state: State,
    pub profiles: Vec<Profile>,
    pub error: Option<String>,
    pub retry_seconds: Option<u64>,
    pub last_success_unix_seconds: Option<u64>,
}
#[derive(Clone, Default, Serialize)]
pub struct Snapshot {
    pub cameras: Vec<CameraStatus>,
    pub discovery_error: Option<String>,
    pub inventory_error: Option<String>,
}
#[derive(Clone)]
pub struct Options {
    pub inventory: PathBuf,
    pub interface: Option<Ipv4Addr>,
    pub target: SocketAddrV4,
    pub discovery_interval: Duration,
    pub poll_interval: Duration,
    pub retry_max: Duration,
}
impl Options {
    pub fn new(inventory: PathBuf, interface: Option<Ipv4Addr>) -> Self {
        Self {
            inventory,
            interface,
            target: SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 250), 3702),
            discovery_interval: Duration::from_secs(10),
            poll_interval: Duration::from_secs(10),
            retry_max: Duration::from_secs(60),
        }
    }
}
pub enum Command {
    SaveDefaults {
        credentials: Option<credentials::Credentials>,
    },
    ClearCredentials {
        id: String,
    },
    AddCamera {
        alias: String,
        address: String,
        onvif: bool,
        h265: bool,
    },
    Rename {
        id: String,
        alias: String,
    },
    SaveCredentials {
        id: String,
        credentials: credentials::Credentials,
    },
}
#[derive(Clone)]
pub struct Controller {
    commands: mpsc::SyncSender<Command>,
}
impl Controller {
    pub fn command(&self, command: Command) -> Result<(), Error> {
        self.commands
            .try_send(command)
            .map_err(|_| Error("camera manager is busy or stopped; retry the operation"))
    }
}
pub struct Manager {
    stop: Arc<AtomicBool>,
    commands: mpsc::SyncSender<Command>,
    snapshot: watch::Receiver<Snapshot>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Manager {
    pub fn start(options: Options) -> Result<Self, Error> {
        if options.discovery_interval < Duration::from_millis(100)
            || options.poll_interval < Duration::from_millis(100)
            || !(Duration::from_secs(1)..=Duration::from_secs(300)).contains(&options.retry_max)
        {
            return Err(Error(
                "camera poll intervals must be at least 100ms and maximum retry between 1s and 300s",
            ));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (commands, receiver) = mpsc::sync_channel(32);
        let (status, snapshot) = watch::channel(Snapshot::default());
        let stopping = stop.clone();
        let worker = thread::Builder::new()
            .name("vidfish-cameras".into())
            .spawn(move || run(options, stopping, receiver, status))
            .map_err(|_| Error("cannot start camera manager worker"))?;
        Ok(Self {
            stop,
            commands,
            snapshot,
            worker: Some(worker),
        })
    }
    pub fn snapshot(&self) -> watch::Receiver<Snapshot> {
        self.snapshot.clone()
    }
    pub fn controller(&self) -> Controller {
        Controller {
            commands: self.commands.clone(),
        }
    }
    pub fn command(&self, command: Command) -> Result<(), Error> {
        self.commands
            .try_send(command)
            .map_err(|_| Error("camera manager is busy or stopped; retry the operation"))
    }
    /// Request stop without blocking the UI. Join after the desktop closes.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
    pub fn join(mut self) {
        self.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for Manager {
    fn drop(&mut self) {
        self.stop();
    }
}
struct Worker {
    record: watch::Sender<CameraRecord>,
    paused: Arc<AtomicBool>,
    thread: thread::JoinHandle<()>,
}
enum Event {
    Connecting(String),
    Resolved(Box<CameraRecord>, Box<Camera>),
    Failed(String, String, u32, u64),
    Manual(String),
    Authentication(String, String),
}
fn status(record: CameraRecord) -> CameraStatus {
    CameraStatus {
        camera: record,
        state: State::Connecting,
        profiles: Vec::new(),
        error: None,
        retry_seconds: None,
        last_success_unix_seconds: None,
    }
}
fn publish(
    sender: &watch::Sender<Snapshot>,
    statuses: &BTreeMap<String, CameraStatus>,
    discovery_error: &Option<String>,
    inventory_error: &Option<String>,
) {
    sender.send_replace(Snapshot {
        cameras: statuses.values().cloned().collect(),
        discovery_error: discovery_error.clone(),
        inventory_error: inventory_error.clone(),
    });
}
fn run(
    options: Options,
    stop: Arc<AtomicBool>,
    commands: mpsc::Receiver<Command>,
    snapshots: watch::Sender<Snapshot>,
) {
    let _lock = match CameraStore::lock(&options.inventory) {
        Ok(lock) => lock,
        Err(error) => {
            snapshots.send_replace(Snapshot {
                inventory_error: Some(error.to_string()),
                ..Default::default()
            });
            return;
        }
    };
    let mut store = match CameraStore::load(&options.inventory) {
        Ok(store) => store,
        Err(error) => {
            snapshots.send_replace(Snapshot {
                inventory_error: Some(format!("Camera inventory not loaded: {error}")),
                ..Default::default()
            });
            return;
        }
    };
    let mut statuses = BTreeMap::new();
    let mut workers = BTreeMap::new();
    let (events, receiver) = mpsc::sync_channel(64);
    let mut discovery_error = None;
    let mut inventory_error = None;
    for camera in &store.cameras {
        start_worker(
            camera.clone(),
            &options,
            &stop,
            &events,
            &mut workers,
            &mut statuses,
        );
    }
    let mut next_discovery = Instant::now();
    publish(&snapshots, &statuses, &discovery_error, &inventory_error);
    while !stop.load(Ordering::Acquire) {
        let mut changed = false;
        while let Ok(command) = commands.try_recv() {
            changed = true;
            match command {
                Command::AddCamera {
                    alias,
                    address,
                    onvif,
                    h265,
                } => {
                    let alias = if alias.trim().is_empty() {
                        format!("Camera {}", store.cameras.len() + 1)
                    } else {
                        alias
                    };
                    let mut updated = store.clone();
                    let result = if onvif {
                        let address = if address.contains("://") {
                            address
                        } else {
                            format!("http://{address}")
                        };
                        match reqwest::Url::parse(&address) {
                            Ok(mut url) => {
                                if url.path() == "/" {
                                    url.set_path("/onvif/device_service");
                                }
                                updated.add_onvif(&alias, url.as_str())
                            }
                            Err(_) => Err(std::io::Error::other("Invalid ONVIF address")),
                        }
                    } else {
                        updated.add_manual(&alias, &address)
                    };
                    match result.and_then(|id| {
                        if let Some(record) = updated.cameras.iter_mut().find(|c| c.id == id) {
                            record.manual_h265 = h265;
                        }
                        updated.save(&options.inventory)?;
                        Ok(id)
                    }) {
                        Ok(id) => {
                            store = updated;
                            inventory_error = None;
                            let record = store.cameras.iter().find(|c| c.id == id).unwrap().clone();
                            start_worker(
                                record,
                                &options,
                                &stop,
                                &events,
                                &mut workers,
                                &mut statuses,
                            );
                        }
                        Err(error) => inventory_error = Some(format!("Camera not added: {error}")),
                    }
                }
                Command::SaveDefaults { credentials } => {
                    match credentials::set_defaults(
                        &credentials::file_path(&options.inventory),
                        credentials,
                    ) {
                        Ok(()) => {
                            inventory_error = None;
                            for worker in workers.values() {
                                let mut candidate = worker.record.borrow().clone();
                                candidate.use_default_credentials = false;
                                worker.record.send_replace(candidate);
                            }
                        }
                        Err(error) => inventory_error = Some(error.to_string()),
                    }
                }
                Command::ClearCredentials { id } => {
                    let mut updated = store.clone();
                    let result = (|| -> Result<(), Error> {
                        let camera = updated
                            .cameras
                            .iter_mut()
                            .find(|camera| camera.id == id)
                            .ok_or(Error("Unknown camera ID"))?;
                        credentials::remove(&credentials::file_path(&options.inventory), &id)?;
                        camera.credential_key = None;
                        camera.use_default_credentials = false;
                        updated
                            .save(&options.inventory)
                            .map_err(|_| Error("Cannot save cleared camera login"))
                    })();
                    match result {
                        Ok(()) => {
                            store = updated;
                            inventory_error = None;
                            if let Some(camera) =
                                store.cameras.iter().find(|camera| camera.id == id)
                            {
                                if let Some(status) = statuses.get_mut(&id) {
                                    status.camera = camera.clone();
                                }
                                if let Some(worker) = workers.get(&id) {
                                    worker.record.send_replace(camera.clone());
                                }
                            }
                        }
                        Err(error) => inventory_error = Some(error.to_string()),
                    }
                }
                Command::SaveCredentials { id, credentials } => {
                    let mut updated = store.clone();
                    let result = match updated.cameras.iter().position(|c| c.id == id) {
                        Some(index) => {
                            credentials::save(&credentials::file_path(&options.inventory), &id, &credentials).and_then(|()| {
                                updated.cameras[index].credential_key = Some(id.clone());
                                updated.cameras[index].use_default_credentials=false;
                                updated.save(&options.inventory).map_err(|_| Error("camera login saved but inventory reference could not be saved; retry"))
                            })
                        }
                        None => Err(Error("unknown camera ID")),
                    };
                    match result {
                        Ok(()) => {
                            store = updated;
                            inventory_error = None;
                            if let Some(s) = statuses.get_mut(&id) {
                                s.camera.credential_key = Some(id.clone());
                                s.camera.use_default_credentials = false;
                            }
                            if let Some(w) = workers.get(&id) {
                                let mut candidate = w.record.borrow().clone();
                                candidate.credential_key = Some(id);
                                candidate.use_default_credentials = false;
                                w.record.send_replace(candidate);
                            }
                        }
                        Err(error) => inventory_error = Some(error.to_string()),
                    }
                }
                Command::Rename { id, alias } => {
                    let mut updated = store.clone();
                    match updated
                        .rename(&id, &alias)
                        .and_then(|()| updated.save(&options.inventory))
                    {
                        Ok(()) => {
                            store = updated;
                            inventory_error = None;
                            if let Some(camera) = store.cameras.iter().find(|c| c.id == id) {
                                if let Some(s) = statuses.get_mut(&id) {
                                    s.camera = camera.clone();
                                }
                                if let Some(w) = workers.get(&id) {
                                    let mut candidate = w.record.borrow().clone();
                                    candidate.alias = camera.alias.clone();
                                    w.record.send_replace(candidate);
                                }
                            }
                        }
                        Err(error) => {
                            inventory_error = Some(format!("Camera rename not saved: {error}"))
                        }
                    }
                }
            }
        }
        if next_discovery <= Instant::now() {
            next_discovery = Instant::now() + options.discovery_interval;
            {
                changed = true;
                match discovery::selected(
                    options.interface,
                    options.target,
                    Duration::from_millis(500),
                ) {
                    Err(error) => discovery_error = Some(error.to_string()),
                    Ok(report) => {
                        discovery_error =
                            (!report.warnings.is_empty()).then(|| report.warnings.join("; "));
                        let devices = report.cameras;
                        let mut identities = BTreeMap::<String, Vec<String>>::new();
                        for device in &devices {
                            identities
                                .entry(device.endpoint_id.clone())
                                .or_default()
                                .push(device.device_url.clone());
                        }
                        for device in devices {
                            if identities[&device.endpoint_id].len() > 1 {
                                discovery_error = Some("Duplicate ONVIF endpoint at multiple addresses; affected cameras quarantined".into());
                                if let Some(camera) = store
                                    .cameras
                                    .iter()
                                    .find(|c| c.endpoint_id.as_deref() == Some(&device.endpoint_id))
                                    && let Some(s) = statuses.get_mut(&camera.id)
                                {
                                    s.state = State::IdentityConflict;
                                    s.error = discovery_error.clone();
                                    s.profiles.clear();
                                    if let Some(w) = workers.get(&camera.id) {
                                        w.paused.store(true, Ordering::Release);
                                    }
                                }
                                continue;
                            }
                            if let Some(camera) = store.cameras.iter().find(|c| {
                                c.endpoint_id.as_deref() == Some(&device.endpoint_id)
                                    || c.device_url.as_deref() == Some(&device.device_url)
                            }) {
                                if let Some(worker) = workers.get(&camera.id)
                                    && (worker.record.borrow().device_url.as_deref()
                                        != Some(&device.device_url)
                                        || worker.record.borrow().endpoint_id.as_deref()
                                            != Some(&device.endpoint_id))
                                {
                                    let mut candidate = camera.clone();
                                    candidate.device_url = Some(device.device_url);
                                    candidate.endpoint_id = Some(device.endpoint_id);
                                    worker.record.send_replace(candidate);
                                }
                            } else {
                                match store.observe(&device.endpoint_id, &device.device_url, None) {
                                    Ok(_) => {
                                        let camera = store.cameras.last().unwrap().clone();
                                        if let Err(error) = store.save(&options.inventory) {
                                            inventory_error = Some(format!(
                                                "Camera inventory not saved: {error}"
                                            ));
                                        }
                                        start_worker(
                                            camera,
                                            &options,
                                            &stop,
                                            &events,
                                            &mut workers,
                                            &mut statuses,
                                        );
                                    }
                                    Err(error) => discovery_error = Some(error.to_string()),
                                }
                            }
                        }
                    }
                }
            }
        }
        while let Ok(event) = receiver.try_recv() {
            changed = true;
            match event {
                Event::Connecting(id) => {
                    if let Some(s) = statuses.get_mut(&id)
                        && !matches!(s.state, State::IdentityConflict | State::Available)
                    {
                        s.state = if s.error.is_some() {
                            State::Reconnecting
                        } else {
                            State::Connecting
                        };
                    }
                }
                Event::Authentication(id, error) => {
                    if let Some(s) = statuses.get_mut(&id) {
                        s.state = State::AuthenticationRequired;
                        s.error = Some(error);
                        s.retry_seconds = None;
                        s.profiles.clear();
                    }
                }
                Event::Manual(id) => {
                    if let Some(s) = statuses.get_mut(&id) {
                        s.state = State::ManualConfigured;
                    }
                }
                Event::Failed(id, error, failures, retry) => {
                    if let Some(s) = statuses.get_mut(&id)
                        && s.state != State::IdentityConflict
                    {
                        if s.state == State::Available {
                            tracing::warn!(camera_id = %id, "camera became unavailable; peers continue");
                        }
                        s.state = if failures >= 3 {
                            State::Offline
                        } else {
                            State::Reconnecting
                        };
                        s.error = Some(error);
                        s.retry_seconds = Some(retry);
                        s.profiles.clear();
                    }
                }
                Event::Resolved(candidate, camera) => {
                    let Some(s) = statuses.get_mut(&candidate.id) else {
                        continue;
                    };
                    if s.state == State::IdentityConflict
                        || workers
                            .get(&candidate.id)
                            .is_none_or(|w| w.record.borrow().device_url != candidate.device_url)
                    {
                        continue;
                    }
                    let hardware = (!camera.manufacturer.is_empty()
                        && !camera.serial_number.is_empty())
                    .then_some(HardwareIdentity {
                        manufacturer: camera.manufacturer,
                        serial_number: camera.serial_number,
                    });
                    let mut updated = store.clone();
                    if let Some(record) = updated.cameras.iter_mut().find(|c| c.id == candidate.id)
                    {
                        record.use_default_credentials = candidate.use_default_credentials;
                    }
                    let observation = if let Some(endpoint) = &candidate.endpoint_id {
                        updated
                            .observe(
                                endpoint,
                                candidate.device_url.as_deref().unwrap_or_default(),
                                hardware,
                            )
                            .map(|_| ())
                    } else {
                        match updated.cameras.iter_mut().find(|c| c.id == candidate.id) {
                            Some(record)
                                if record.hardware.is_none() || record.hardware == hardware =>
                            {
                                record.hardware = hardware;
                                Ok(())
                            }
                            _ => Err(std::io::Error::other(
                                "camera hardware identity changed; confirm identity explicitly",
                            )),
                        }
                    };
                    if let Err(error) = observation {
                        s.state = State::IdentityConflict;
                        s.error = Some(error.to_string());
                        s.profiles.clear();
                        if let Some(w) = workers.get(&candidate.id) {
                            w.paused.store(true, Ordering::Release);
                        }
                        continue;
                    }
                    if updated != store {
                        match updated.save(&options.inventory) {
                            Ok(()) => {
                                store = updated;
                                inventory_error = None;
                            }
                            Err(error) => {
                                inventory_error =
                                    Some(format!("Camera identity not saved: {error}"))
                            }
                        }
                    }
                    if let Some(record) = store.cameras.iter().find(|c| c.id == candidate.id) {
                        s.camera = record.clone();
                    }
                    if s.state != State::Available {
                        tracing::info!(camera_id = %candidate.id, "camera available; acquisition policy may start recording independently");
                    }
                    if let Some(worker) = workers.get(&candidate.id)
                        && worker.record.borrow().use_default_credentials
                            != candidate.use_default_credentials
                    {
                        let mut record = worker.record.borrow().clone();
                        record.use_default_credentials = candidate.use_default_credentials;
                        worker.record.send_replace(record);
                    }
                    s.state = State::Available;
                    s.profiles = camera.profiles;
                    s.error = None;
                    s.retry_seconds = None;
                    s.last_success_unix_seconds = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs());
                }
            }
        }
        if changed {
            publish(&snapshots, &statuses, &discovery_error, &inventory_error);
        }
        thread::sleep(Duration::from_millis(20));
    }
    for (_, worker) in workers {
        let _ = worker.thread.join();
    }
    for s in statuses.values_mut() {
        s.state = State::Stopped;
        s.profiles.clear();
    }
    publish(&snapshots, &statuses, &discovery_error, &inventory_error);
}
fn start_worker(
    record: CameraRecord,
    options: &Options,
    stop: &Arc<AtomicBool>,
    events: &mpsc::SyncSender<Event>,
    workers: &mut BTreeMap<String, Worker>,
    statuses: &mut BTreeMap<String, CameraStatus>,
) {
    let id = record.id.clone();
    statuses.insert(id.clone(), status(record.clone()));
    let (sender, mut source) = watch::channel(record);
    let stop = stop.clone();
    let paused = Arc::new(AtomicBool::new(false));
    let worker_paused = paused.clone();
    let events = events.clone();
    let poll = options.poll_interval;
    let max = options.retry_max;
    let credentials_path = credentials::file_path(&options.inventory);
    let worker = thread::spawn(move || {
        let mut failures = 0u32;
        while !stop.load(Ordering::Acquire) {
            if worker_paused.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(20));
                continue;
            }
            let record = source.borrow_and_update().clone();
            let Some(_address) = &record.device_url else {
                let _ = events.try_send(Event::Manual(record.id));
                wait(&stop, &mut source, poll);
                continue;
            };
            let _ = events.try_send(Event::Connecting(record.id.clone()));
            let delay = match credentials::resolve(&credentials_path, &record) {
                Ok((camera, used_default)) => {
                    failures = 0;
                    let mut record = record;
                    record.use_default_credentials = used_default;
                    let _ = events.try_send(Event::Resolved(Box::new(record), Box::new(camera)));
                    poll
                }
                Err(error) if error.authentication_required() => {
                    let _ = events.try_send(Event::Authentication(record.id, error.to_string()));
                    Duration::from_secs(86400)
                }
                Err(error) => {
                    failures = failures.saturating_add(1);
                    let delay = retry_delay(failures, max);
                    let _ = events.try_send(Event::Failed(
                        record.id,
                        error.to_string(),
                        failures,
                        delay.as_secs(),
                    ));
                    delay
                }
            };
            wait(&stop, &mut source, delay);
        }
    });
    workers.insert(
        id,
        Worker {
            record: sender,
            paused,
            thread: worker,
        },
    );
}
fn retry_delay(failures: u32, max: Duration) -> Duration {
    Duration::from_secs(1u64 << failures.saturating_sub(1).min(8)).min(max)
}
fn wait(stop: &AtomicBool, source: &mut watch::Receiver<CameraRecord>, duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline
        && !stop.load(Ordering::Acquire)
        && !source.has_changed().unwrap_or(true)
    {
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_timer_is_bounded_even_after_many_failures() {
        let max = Duration::from_secs(60);
        assert_eq!(retry_delay(1, max), Duration::from_secs(1));
        assert_eq!(retry_delay(4, max), Duration::from_secs(8));
        assert_eq!(retry_delay(u32::MAX, max), max);
    }
}
