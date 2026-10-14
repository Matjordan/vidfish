use std::{
    net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket},
    time::{Duration, Instant},
};
use vidfish_core::cameras::CameraStore;
use vidfish_media::fixtures::OnvifFixture;
use vidfish_onvif::manager::{Command, Manager, Options, Snapshot, State};

fn wait(snapshot: &tokio::sync::watch::Receiver<Snapshot>, predicate: impl Fn(&Snapshot) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !predicate(&snapshot.borrow()) {
        assert!(
            Instant::now() < deadline,
            "camera manager did not reach expected state: cameras={:?}, inventory={:?}, discovery={:?}",
            snapshot
                .borrow()
                .cameras
                .iter()
                .map(|c| (&c.camera.alias, c.state, &c.error, c.retry_seconds))
                .collect::<Vec<_>>(),
            snapshot.borrow().inventory_error,
            snapshot.borrow().discovery_error,
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[test]
fn sixteen_camera_workers_isolate_failure_and_persist_rename() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cameras.json");
    let mut inventory = CameraStore::default();
    let mut fixtures = Vec::new();
    for number in 1..=15 {
        let camera = OnvifFixture::start_at(
            "rtsp://127.0.0.1/live".into(),
            number,
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();
        inventory
            .add_onvif(&format!("Camera {number}"), &camera.device_url)
            .unwrap();
        fixtures.push(camera);
    }
    let reserved = TcpListener::bind("127.0.0.1:0").unwrap();
    let failed = inventory
        .add_onvif(
            "Unavailable",
            &format!("http://{}/onvif", reserved.local_addr().unwrap()),
        )
        .unwrap();
    drop(reserved);
    let healthy = inventory.cameras[0].id.clone();
    inventory.save(&path).unwrap();
    let options = Options::new(path.clone(), None);
    let manager = Manager::start(options).unwrap();
    let snapshot = manager.snapshot();
    wait(&snapshot, |s| {
        s.cameras
            .iter()
            .filter(|c| c.state == State::Available)
            .count()
            == 15
            && s.cameras
                .iter()
                .any(|c| c.camera.id == failed && c.state == State::Offline)
    });
    manager
        .command(Command::Rename {
            id: healthy.clone(),
            alias: "Port side".into(),
        })
        .unwrap();
    wait(&snapshot, |s| {
        s.cameras
            .iter()
            .any(|c| c.camera.id == healthy && c.camera.alias == "Port side")
    });
    assert_eq!(
        CameraStore::load(&path).unwrap().cameras[0].alias,
        "Port side"
    );
    assert_eq!(snapshot.borrow().cameras.len(), 16);
    manager.join();
    assert!(
        snapshot
            .borrow()
            .cameras
            .iter()
            .all(|c| c.state == State::Stopped)
    );
}
#[test]
fn hot_arrival_disappearance_and_reconnection_need_no_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cameras.json");
    let http = TcpListener::bind("127.0.0.1:0").unwrap();
    let http_address = http.local_addr().unwrap();
    drop(http);
    let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let target = udp.local_addr().unwrap();
    drop(udp);
    let SocketAddr::V4(target_v4) = target else {
        panic!()
    };
    let mut options = Options::new(path.clone(), Some(Ipv4Addr::LOCALHOST));
    options.target = target_v4;
    options.discovery_interval = Duration::from_millis(200);
    options.poll_interval = Duration::from_millis(200);
    options.retry_max = Duration::from_secs(1);
    let manager = Manager::start(options).unwrap();
    let snapshot = manager.snapshot();
    let camera =
        OnvifFixture::start_at("rtsp://127.0.0.1/live".into(), 10, http_address, target).unwrap();
    wait(&snapshot, |s| {
        s.cameras.len() == 1 && s.cameras[0].state == State::Available
    });
    let id = snapshot.borrow().cameras[0].camera.id.clone();
    drop(camera);
    wait(&snapshot, |s| s.cameras[0].state == State::Offline);
    let restarted = OnvifFixture::start_at(
        "rtsp://127.0.0.1/live".into(),
        10,
        "127.0.0.1:0".parse().unwrap(),
        target,
    )
    .unwrap();
    wait(&snapshot, |s| s.cameras[0].state == State::Available);
    assert_eq!(snapshot.borrow().cameras[0].camera.id, id);
    assert_eq!(
        CameraStore::load(&path).unwrap().cameras[0]
            .device_url
            .as_deref(),
        Some(restarted.device_url.as_str())
    );
    manager.join();
}

#[test]
fn inventory_lock_rejects_a_second_manager_without_losing_camera_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cameras.json");
    let mut inventory = CameraStore::default();
    inventory
        .add_manual("Bench", "rtsp://127.0.0.1/live")
        .unwrap();
    inventory.save(&path).unwrap();
    let first = Manager::start(Options::new(path.clone(), None)).unwrap();
    wait(&first.snapshot(), |s| s.cameras.len() == 1);
    let second = Manager::start(Options::new(path.clone(), None)).unwrap();
    wait(&second.snapshot(), |s| s.inventory_error.is_some());
    assert!(
        second
            .snapshot()
            .borrow()
            .inventory_error
            .as_ref()
            .unwrap()
            .contains("in use")
    );
    second.join();
    assert_eq!(CameraStore::load(&path).unwrap().cameras[0].alias, "Bench");
    first.join();
    assert!(CameraStore::lock(&path).is_ok());
}

#[test]
fn runtime_login_creates_storage_and_survives_manager_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cameras.json");
    let mut inventory = CameraStore::default();
    let id = inventory
        .add_manual("Bench", "rtsp://127.0.0.1/live")
        .unwrap();
    inventory.save(&path).unwrap();
    let manager = Manager::start(Options::new(path.clone(), None)).unwrap();
    manager
        .command(Command::SaveCredentials {
            id: id.clone(),
            credentials: vidfish_onvif::credentials::Credentials::new(
                "fixture-user".into(),
                "fixture-password".into(),
            )
            .unwrap(),
        })
        .unwrap();
    wait(&manager.snapshot(), |s| {
        s.cameras
            .first()
            .is_some_and(|c| c.camera.credential_key.as_deref() == Some(&id))
    });
    let saved = vidfish_onvif::credentials::file_path(&path);
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&saved).unwrap()).unwrap();
    assert_eq!(json["cameras"][&id]["username"], "fixture-user");
    assert_eq!(json["cameras"][&id]["password"], "fixture-password");
    assert!(
        !serde_json::to_string(&*manager.snapshot().borrow())
            .unwrap()
            .contains("fixture-password")
    );
    manager.join();
    let restarted = Manager::start(Options::new(path, None)).unwrap();
    wait(&restarted.snapshot(), |s| {
        s.cameras
            .first()
            .is_some_and(|c| c.camera.credential_key.as_deref() == Some(&id))
    });
    assert!(vidfish_onvif::credentials::load(&saved, &id).is_ok());
    restarted.join();
}

#[test]
fn manual_camera_can_be_added_during_a_run_with_codec_and_alias_persisted() {
    let directory = tempfile::tempdir().unwrap();
    let inventory = directory.path().join("cameras.json");
    let manager = Manager::start(Options::new(inventory.clone(), None)).unwrap();
    manager
        .command(Command::AddCamera {
            alias: "H265 bench".into(),
            address: "rtsp://127.0.0.1/live".into(),
            onvif: false,
            h265: true,
        })
        .unwrap();
    wait(&manager.snapshot(), |s| {
        s.cameras.len() == 1 && s.cameras[0].state == State::ManualConfigured
    });
    let store = CameraStore::load(&inventory).unwrap();
    assert_eq!(store.cameras[0].alias, "H265 bench");
    assert!(store.cameras[0].manual_h265);
    manager.join();
}

/// Authentication gate checks WS-Security password digests, then forwards to the
/// real ONVIF fixture. This keeps credential fallback tests on actual HTTP/SOAP.
struct LoginGate {
    address: String,
    requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl LoginGate {
    fn start(upstream: &str) -> Self {
        use base64::Engine;
        use sha1::{Digest, Sha1};
        use std::io::{Read, Write};
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = format!(
            "http://{}/onvif/device_service",
            listener.local_addr().unwrap()
        );
        let upstream = reqwest::Url::parse(upstream).unwrap();
        let upstream = format!(
            "{}:{}",
            upstream.host_str().unwrap(),
            upstream.port().unwrap()
        );
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let stopping = stop.clone();
        let count = requests.clone();
        let worker = std::thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    let n = stream.read(&mut buffer).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length = headers
                            .lines()
                            .filter_map(|line| line.split_once(':'))
                            .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                    if bytes.len() > 65536 {
                        break;
                    }
                }
                count.fetch_add(1, Ordering::Relaxed);
                let request = String::from_utf8_lossy(&bytes);
                let body = request.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
                let allowed = (|| -> Option<bool> {
                    let doc = roxmltree::Document::parse(body).ok()?;
                    let value = |name| {
                        doc.descendants()
                            .find(|n| n.is_element() && n.tag_name().name() == name)
                            .and_then(|n| n.text())
                    };
                    let nonce = base64::engine::general_purpose::STANDARD
                        .decode(value("Nonce")?)
                        .ok()?;
                    let mut digest = Sha1::new();
                    digest.update(nonce);
                    digest.update(value("Created")?.as_bytes());
                    digest.update(b"gate-password");
                    Some(
                        value("Username")? == "gate-user"
                            && value("Password")?
                                == base64::engine::general_purpose::STANDARD
                                    .encode(digest.finalize()),
                    )
                })();
                if allowed == Some(true) {
                    if let Ok(mut peer) = std::net::TcpStream::connect(&upstream) {
                        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                        peer.write_all(&bytes).unwrap();
                        let mut response = Vec::new();
                        let _ = peer.read_to_end(&mut response);
                        let _ = stream.write_all(&response);
                    }
                } else {
                    let _=stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Digest realm=\"camera\", nonce=\"fixed\", algorithm=MD5, qop=\"auth\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                }
            }
        });
        Self {
            address,
            requests,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for LoginGate {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}
#[test]
fn shared_login_fallback_pauses_wrong_password_retries_and_preserves_overrides() {
    use vidfish_onvif::credentials::{self, Credentials};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cameras.json");
    let fixture = OnvifFixture::start("rtsp://127.0.0.1/live".into()).unwrap();
    let gate = LoginGate::start(&fixture.device_url);
    let mut store = CameraStore::default();
    let id = store.add_onvif("Login camera", &gate.address).unwrap();
    store
        .add_onvif("Healthy peer", &fixture.device_url)
        .unwrap();
    store.save(&path).unwrap();
    let manager = Manager::start(Options::new(path.clone(), None)).unwrap();
    let snapshot = manager.snapshot();
    let auth = |s: &Snapshot| {
        s.cameras
            .iter()
            .any(|c| c.camera.id == id && c.state == State::AuthenticationRequired)
    };
    wait(&snapshot, |s| {
        auth(s)
            && s.cameras
                .iter()
                .any(|c| c.camera.alias == "Healthy peer" && c.state == State::Available)
    });
    manager
        .command(Command::SaveDefaults {
            credentials: Some(
                Credentials::new("gate-user".into(), "wrong-password".into()).unwrap(),
            ),
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    wait(&snapshot, auth);
    let count = gate.requests.load(std::sync::atomic::Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(1300));
    assert_eq!(
        gate.requests.load(std::sync::atomic::Ordering::Relaxed),
        count,
        "bad login retried without operator action"
    );
    manager
        .command(Command::SaveDefaults {
            credentials: Some(
                Credentials::new("gate-user".into(), "gate-password".into()).unwrap(),
            ),
        })
        .unwrap();
    wait(&snapshot, |s| {
        s.cameras.iter().any(|c| {
            c.camera.id == id && c.state == State::Available && c.camera.use_default_credentials
        })
    });
    manager
        .command(Command::SaveCredentials {
            id: id.clone(),
            credentials: Credentials::new("bad-override".into(), "wrong-override".into()).unwrap(),
        })
        .unwrap();
    wait(&snapshot, |s| {
        s.cameras.iter().any(|c| {
            c.camera.id == id
                && c.state == State::Available
                && c.camera.credential_key.is_some()
                && c.camera.use_default_credentials
        })
    });
    let disk = CameraStore::load(&path).unwrap();
    let camera = disk.cameras.iter().find(|c| c.id == id).unwrap();
    assert!(
        credentials::effective(&credentials::file_path(&path), camera)
            .unwrap()
            .unwrap()
            .expose(|u, p| u == "gate-user" && p == "gate-password")
    );
    let serialized = serde_json::to_string(&*snapshot.borrow()).unwrap();
    for secret in [
        "gate-password",
        "wrong-password",
        "wrong-override",
        "bad-override",
    ] {
        assert!(!serialized.contains(secret));
    }
    manager.join();
    let manager = Manager::start(Options::new(path.clone(), None)).unwrap();
    let snapshot = manager.snapshot();
    wait(&snapshot, |s| {
        s.cameras
            .iter()
            .any(|c| c.camera.id == id && c.state == State::Available)
    });
    manager
        .command(Command::SaveCredentials {
            id: id.clone(),
            credentials: Credentials::new("gate-user".into(), "gate-password".into()).unwrap(),
        })
        .unwrap();
    wait(&snapshot, |s| {
        s.cameras
            .iter()
            .any(|c| c.camera.id == id && !c.camera.use_default_credentials)
    });
    manager
        .command(Command::SaveDefaults { credentials: None })
        .unwrap();
    assert!(credentials::load(&credentials::file_path(&path), &id).is_ok());
    manager
        .command(Command::ClearCredentials { id: id.clone() })
        .unwrap();
    wait(&snapshot, auth);
    assert!(credentials::load(&credentials::file_path(&path), &id).is_err());
    manager.join();
}

#[test]
fn forgetting_retires_worker_and_rediscovery_assigns_new_identity_with_shared_policy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cameras.json");
    let camera = OnvifFixture::start_at(
        "rtsp://127.0.0.1/live".into(),
        136,
        "127.0.0.1:0".parse().unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let mut options = Options::new(path.clone(), Some(Ipv4Addr::LOCALHOST));
    let SocketAddr::V4(target) = camera.discovery_address else {
        panic!("IPv4 required")
    };
    options.target = target;
    options.discovery_interval = Duration::from_millis(200);
    options.poll_interval = Duration::from_millis(200);
    let credentials_path = vidfish_onvif::credentials::file_path(&path);
    vidfish_onvif::credentials::set_defaults(
        &credentials_path,
        Some(
            vidfish_onvif::credentials::Credentials::new("shared".into(), "shared-password".into())
                .unwrap(),
        ),
    )
    .unwrap();
    let manager = Manager::start(options.clone()).unwrap();
    let snapshot = manager.snapshot();
    wait(&snapshot, |s| {
        s.cameras.len() == 1 && s.cameras[0].state == State::Available
    });
    let old = snapshot.borrow().cameras[0].camera.id.clone();
    manager
        .command(Command::Rename {
            id: old.clone(),
            alias: "Obsolete alias".into(),
        })
        .unwrap();
    wait(&snapshot, |s| s.cameras[0].camera.alias == "Obsolete alias");
    vidfish_onvif::credentials::save(
        &credentials_path,
        &old,
        &vidfish_onvif::credentials::Credentials::new(
            "override".into(),
            "override-password".into(),
        )
        .unwrap(),
    )
    .unwrap();
    let guard =
        CameraStore::lock(&dir.path().join("acquisition").join(format!("{old}.record"))).unwrap();
    manager
        .command(Command::Forget { id: old.clone() })
        .unwrap();
    wait(&snapshot, |s| s.inventory_error.is_some());
    assert!(
        CameraStore::load(&path)
            .unwrap()
            .cameras
            .iter()
            .any(|c| c.id == old)
    );
    drop(guard);
    manager
        .command(Command::Forget { id: old.clone() })
        .unwrap();
    wait(&snapshot, |s| {
        s.cameras.len() == 1
            && s.cameras[0].camera.id != old
            && s.cameras[0].state == State::Available
    });
    let new = snapshot.borrow().cameras[0].camera.clone();
    assert_ne!(new.alias, "Obsolete alias");
    assert!(new.credential_key.is_none());
    assert!(new.use_default_credentials);
    assert!(vidfish_onvif::credentials::load(&credentials_path, &old).is_err());
    assert!(
        vidfish_onvif::credentials::defaults(&credentials_path)
            .unwrap()
            .is_some()
    );
    manager.join();
    let manager = Manager::start(options).unwrap();
    wait(&manager.snapshot(), |s| {
        s.cameras
            .iter()
            .any(|c| c.camera.id == new.id && c.state == State::Available)
    });
    manager.join();
}
