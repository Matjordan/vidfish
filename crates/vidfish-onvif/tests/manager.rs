use std::{
    net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket},
    time::{Duration, Instant},
};
use vidfish_core::cameras::CameraStore;
use vidfish_media::fixtures::OnvifFixture;
use vidfish_onvif::manager::{Command, Manager, Options, Snapshot, State};

fn wait(snapshot: &tokio::sync::watch::Receiver<Snapshot>, predicate: impl Fn(&Snapshot) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate(&snapshot.borrow()) {
        assert!(
            Instant::now() < deadline,
            "camera manager did not reach expected state"
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
    let mut options = Options::new(path.clone(), None);
    options.poll_interval = Duration::from_millis(200);
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
