//! Owned, versioned application bundles. Operator data never belongs to this manifest.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub const VERSION: &str = match option_env!("VIDFISH_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    pub schema: u32,
    pub version: String,
    pub platform: String,
    pub files: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Install {
    schema: u32,
    files: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    previous: String,
    next: String,
}
pub fn platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows-x86_64"
    } else {
        "linux-x86_64"
    }
}
pub fn root(prefix: &Path) -> PathBuf {
    prefix.join("lib/vidfish")
}
pub fn installed_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let bundle = exe.parent()?;
    let versions = bundle.parent()?;
    let root = versions.parent()?;
    (versions.file_name()? == "versions" && root.join("install.json").is_file())
        .then(|| root.to_owned())
}
pub fn prefix_from_root(root: &Path) -> Result<PathBuf> {
    if root.file_name().is_none_or(|s| s != "vidfish")
        || root
            .parent()
            .and_then(Path::file_name)
            .is_none_or(|s| s != "lib")
    {
        return Err("Invalid managed install root".into());
    }
    Ok(root
        .parent()
        .and_then(Path::parent)
        .ok_or("Invalid install prefix")?
        .to_owned())
}
pub fn version(value: &str) -> Result<()> {
    semver::Version::parse(value)?;
    if value.len() > 64
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".+-".contains(&c))
    {
        return Err("Invalid version".into());
    }
    Ok(())
}
fn relative(value: &str) -> Result<PathBuf> {
    if value.is_empty() || value.contains(['\\', ':', '\n', '\r']) {
        return Err("Invalid bundle path".into());
    }
    let path = Path::new(value);
    if !path
        .components()
        .all(|p| matches!(p, std::path::Component::Normal(_)))
    {
        return Err("Unsafe bundle path".into());
    }
    Ok(path.into())
}
pub fn digest(path: &Path) -> Result<String> {
    let mut input = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or("Missing parent")?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let f = File::open(path)?;
    let mut bytes = Vec::new();
    f.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("Manifest too large".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}
pub fn verify(bundle: &Path) -> Result<Bundle> {
    let manifest: Bundle = read_json(&bundle.join("bundle.json"))?;
    version(&manifest.version)?;
    if manifest.schema != 1
        || manifest.platform != platform()
        || !cfg!(target_arch = "x86_64")
        || manifest.files.is_empty()
        || manifest.files.len() > 20000
    {
        return Err("Unsupported bundle schema or architecture".into());
    }
    for required in [
        if cfg!(windows) {
            "vidfish.exe"
        } else {
            "vidfish"
        },
        if cfg!(windows) {
            "vidfish-maintenance.exe"
        } else {
            "vidfish-maintenance"
        },
        "assets/vidfish.png",
        "assets/vidfish.ico",
    ] {
        if !manifest.files.contains_key(required) {
            return Err(format!("Missing bundle file: {required}").into());
        }
    }
    if cfg!(windows)
        && !manifest
            .files
            .keys()
            .any(|p| p.starts_with("gstreamer/bin/"))
    {
        return Err("Missing bundled GStreamer runtime".into());
    }
    let mut actual = BTreeMap::new();
    inventory(bundle, bundle, &mut actual)?;
    actual.remove("bundle.json");
    if actual != manifest.files {
        return Err("Bundle files do not match SHA256 manifest".into());
    }
    Ok(manifest)
}
fn inventory(base: &Path, path: &Path, out: &mut BTreeMap<String, String>) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_symlink() {
            return Err("Symlinks are not allowed in owned bundles".into());
        }
        if kind.is_dir() {
            inventory(base, &path, out)?;
        } else if kind.is_file() {
            let name = path
                .strip_prefix(base)?
                .to_string_lossy()
                .replace('\\', "/");
            relative(&name)?;
            out.insert(name, digest(&path)?);
        } else {
            return Err("Unsupported bundle file type".into());
        }
    }
    Ok(())
}
pub fn lock(root: &Path, name: &str, exclusive: bool) -> Result<File> {
    fs::create_dir_all(root)?;
    if exclusive && fs::metadata(root)?.permissions().readonly() {
        return Err("Install directory is read-only; use the administrator/package manager for manual updates".into());
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(name))
        .or_else(|error| {
            if !exclusive {
                File::open(root.join(name))
            } else {
                Err(error)
            }
        })?;
    if exclusive {
        file.try_lock()?;
    } else {
        file.try_lock_shared()?;
    }
    Ok(file)
}
/// Held by each recorder through EOS, status, session manifest and event persistence.
/// The updater takes the exclusive side before any pointer change, preventing new starts.
pub fn recording_lease() -> Result<Option<File>> {
    installed_root()
        .map(|r| lock(&r, "recording.lock", false))
        .transpose()
}
pub fn process_lease() -> Result<Option<File>> {
    installed_root()
        .map(|r| lock(&r, "process.lock", false))
        .transpose()
}
pub fn recording_idle(root: &Path) -> bool {
    lock(root, "recording.lock", true).is_ok()
}
fn launcher(prefix: &Path) -> String {
    let r = root(prefix);
    if cfg!(windows) {
        let r = r.display();
        format!(
            r#"@echo off
setlocal
if exist "{r}\transaction.json" (
  call :recover
  if errorlevel 1 exit /b 1
)
set /p VIDFISH_INSTALLED_VERSION=<"{r}\current.txt"
call "{r}\versions\%VIDFISH_INSTALLED_VERSION%\launch-vidfish.cmd" %*
exit /b %errorlevel%
:recover
set /p VIDFISH_OLD_VERSION=<"{r}\previous.txt"
"{r}\versions\%VIDFISH_OLD_VERSION%\vidfish-maintenance.exe" recover --root "{r}"
exit /b %errorlevel%
"#
        )
    } else {
        let quoted = r.to_string_lossy().replace('\'', "'\\''");
        format!(
            r#"#!/bin/sh
set -eu
r='{quoted}'
if [ -f "$r/transaction.json" ]; then
  old=$(cat "$r/previous.txt")
  case "$old" in ''|*[!0-9A-Za-z.+-]*) echo 'Invalid prior version; run install.sh to repair' >&2; exit 1;; esac
  "$r/versions/$old/vidfish-maintenance" recover --root "$r"
fi
v=$(cat "$r/current.txt")
case "$v" in ''|*[!0-9A-Za-z.+-]*) echo 'Invalid installed version; run install.sh to repair' >&2; exit 1;; esac
exec "$r/versions/$v/launch-vidfish.sh" "$@"
"#
        )
    }
}
fn executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}
fn health(bundle: &Path, expected: &str) -> Result<()> {
    let settings = tempfile::tempdir()?;
    let binary = bundle.join(if cfg!(windows) {
        "vidfish.exe"
    } else {
        "vidfish"
    });
    for args in [
        vec!["--version"],
        vec!["--check"],
        vec!["--media-check"],
        vec!["--media-check", "--h265"],
    ] {
        let mut cmd = Command::new(&binary);
        cmd.args(&args)
            .env("VIDFISH_INSTALL_HEALTH", "1")
            .current_dir(bundle);
        if args == ["--check"] {
            cmd.arg("--config").arg(settings.path().join("config.json"));
        }
        if cfg!(windows) {
            let path = std::env::var_os("PATH").unwrap_or_default();
            let mut paths = vec![bundle.join("gstreamer/bin")];
            paths.extend(std::env::split_paths(&path));
            cmd.env("PATH", std::env::join_paths(paths)?).env(
                "GST_PLUGIN_SYSTEM_PATH_1_0",
                bundle.join("gstreamer/lib/gstreamer-1.0"),
            );
        }
        let out = tempfile::NamedTempFile::new()?;
        cmd.stdout(out.reopen()?)
            .stderr(std::process::Stdio::null());
        let mut child = cmd.spawn()?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(s) = child.try_wait()? {
                break s;
            }
            if Instant::now() > deadline {
                child.kill()?;
                child.wait()?;
                return Err("New application health check timed out".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let text = fs::read_to_string(out.path())?;
        if !status.success()
            || (args == ["--version"] && text.trim() != format!("vidfish {expected}"))
        {
            return Err("New application failed version/GStreamer H.264/H.265 health checks; previous version retained".into());
        }
    }
    Ok(())
}
fn current(root: &Path) -> Result<String> {
    let v = fs::read_to_string(root.join("current.txt"))?;
    version(v.trim())?;
    Ok(v.trim().into())
}
fn copy_bundle(bundle: &Path, destination: &Path, manifest: &Bundle) -> Result<()> {
    let parent = destination.parent().ok_or("Missing bundle parent")?;
    let temp = tempfile::tempdir_in(parent)?;
    let staged = temp.path().join("bundle");
    fs::create_dir(&staged)?;
    for name in manifest
        .files
        .keys()
        .chain(std::iter::once(&"bundle.json".into()))
    {
        let path = relative(name)?;
        let dest = staged.join(&path);
        fs::create_dir_all(dest.parent().ok_or("Missing parent")?)?;
        fs::copy(bundle.join(path), &dest)?;
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&dest)?
            .sync_all()?;
        if [
            "vidfish",
            "vidfish-maintenance",
            "launch-vidfish.sh",
            "install.sh",
            "uninstall.sh",
        ]
        .contains(&name.as_str())
        {
            executable(&dest)?;
        }
    }
    verify(&staged)?;
    #[cfg(unix)]
    sync_directories(&staged)?;
    fs::rename(&staged, destination)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}
fn write_integrations(prefix: &Path, owned: &mut Install) -> Result<()> {
    let bin = if cfg!(windows) {
        "bin/vidfish.cmd"
    } else {
        "bin/vidfish"
    };
    let mut files = vec![
        (bin.to_owned(), launcher(prefix).into_bytes()),
        (
            "share/icons/hicolor/256x256/apps/vidfish.png".into(),
            include_bytes!("../../../assets/vidfish.png").to_vec(),
        ),
        (
            "share/icons/vidfish.ico".into(),
            include_bytes!("../../../assets/vidfish.ico").to_vec(),
        ),
    ];
    if !cfg!(windows) {
        let path = prefix
            .join(bin)
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$")
            .replace('%', "%%");
        files.push(("share/applications/vidfish.desktop".into(),format!("[Desktop Entry]\nType=Application\nName=Vidfish\nComment=Scientific camera acquisition\nExec=\"{path}\"\nIcon=vidfish\nTerminal=false\nCategories=Science;AudioVideo;\nStartupWMClass=Vidfish\n").into_bytes()));
    }
    for (name, _) in &files {
        let path = prefix.join(name);
        if path.is_symlink() {
            return Err("Refusing a symlink in install integration files".into());
        }
        if path.exists()
            && owned
                .files
                .get(name)
                .is_none_or(|hash| digest(&path).ok().as_ref() != Some(hash))
        {
            return Err(format!(
                "Refusing to overwrite an unowned or modified file: {}",
                path.display()
            )
            .into());
        }
    }
    // Persist ownership before creating integrations, allowing interrupted fresh installs to retry.
    for (name, bytes) in &files {
        owned
            .files
            .insert(name.clone(), format!("{:x}", Sha256::digest(bytes)));
    }
    atomic(
        &root(prefix).join("install.json"),
        &serde_json::to_vec_pretty(owned)?,
    )?;
    for (name, bytes) in files {
        let path = prefix.join(&name);
        atomic(&path, &bytes)?;
        if name == bin {
            executable(&path)?;
        }
        owned.files.insert(name, digest(&path)?);
    }
    Ok(())
}
pub fn install(prefix: &Path, bundle: &Path) -> Result<()> {
    if prefix.to_string_lossy().contains(['\n', '\r']) {
        return Err("Unsupported newline in install prefix".into());
    }
    let manifest = verify(bundle)?;
    health(bundle, &manifest.version)?;
    let root = root(prefix);
    fs::create_dir_all(&root)?;
    let _operation = lock(&root, "operation.lock", true)?;
    let _recording = lock(&root, "recording.lock", true).map_err(
        |_| "Recording in progress; installation deferred. Stop recordings explicitly and retry.",
    )?;
    let _process = lock(&root, "process.lock", true)
        .map_err(|_| "Close Vidfish and camera services before installation")?;
    let mut owned = if root.join("install.json").exists() {
        read_json::<Install>(&root.join("install.json"))?
    } else {
        Install {
            schema: 1,
            files: BTreeMap::new(),
        }
    };
    if owned.schema != 1 {
        return Err("Unsupported installation manifest".into());
    }
    recover_locked(&root)?;
    let versions = root.join("versions");
    fs::create_dir_all(&versions)?;
    let dest = versions.join(&manifest.version);
    if !dest.exists() {
        copy_bundle(bundle, &dest, &manifest)?;
    } else if verify(&dest)?.files != manifest.files {
        return Err("Installed version differs from this bundle; refusing overwrite".into());
    }
    write_integrations(prefix, &mut owned)?;
    atomic(
        &root.join("install.json"),
        &serde_json::to_vec_pretty(&owned)?,
    )?;
    if root.join("current.txt").exists() {
        apply_locked(&root, &manifest.version)?;
    } else {
        atomic(&root.join("current.txt"), manifest.version.as_bytes())?;
    }
    println!(
        "Installed Vidfish {} in {}. Camera settings and scientific data are untouched.",
        manifest.version,
        prefix.display()
    );
    Ok(())
}
fn apply_locked(root: &Path, next: &str) -> Result<()> {
    version(next)?;
    let previous = current(root)?;
    let transaction = Transaction {
        previous,
        next: next.into(),
    };
    let bundle = root.join("versions").join(next);
    let manifest = verify(&bundle)?;
    if manifest.version != next {
        return Err("Bundle directory/version mismatch".into());
    }
    verify(&root.join("versions").join(&transaction.previous))?;
    atomic(&root.join("previous.txt"), transaction.previous.as_bytes())?;
    atomic(
        &root.join("transaction.json"),
        &serde_json::to_vec(&transaction)?,
    )?;
    // The previous pointer is journaled and synced before replacing it.
    atomic(&root.join("current.txt"), next.as_bytes())?;
    match health(&bundle, next) {
        Ok(()) => {
            atomic(&root.join("previous.txt"), transaction.previous.as_bytes())?;
            atomic(
                &root.join("health.json"),
                &serde_json::to_vec(
                    &serde_json::json!({"version":next,"runtime_checks":"version, configuration startup, H.264, H.265, RTSP plugins"}),
                )?,
            )?;
            if root.join("update-error.txt").is_file() {
                fs::remove_file(root.join("update-error.txt"))?;
            }
            fs::remove_file(root.join("transaction.json"))?;
            #[cfg(windows)]
            update_uninstall_version(next);
            Ok(())
        }
        Err(error) => {
            atomic(&root.join("current.txt"), transaction.previous.as_bytes())?;
            atomic(&root.join("update-error.txt"), error.to_string().as_bytes())?;
            fs::remove_file(root.join("transaction.json"))?;
            Err(error)
        }
    }
}
fn recover_locked(root: &Path) -> Result<()> {
    if root.join("transaction.json").exists() {
        let transaction: Transaction = read_json(&root.join("transaction.json"))?;
        version(&transaction.previous)?;
        version(&transaction.next)?;
        let bundle = root.join("versions").join(&transaction.previous);
        verify(&bundle)?;
        atomic(&root.join("current.txt"), transaction.previous.as_bytes())?;
        atomic(
            &root.join("update-error.txt"),
            b"Interrupted update recovered to the previous verified version. Retry from Settings.",
        )?;
        fs::remove_file(root.join("transaction.json"))?;
    }
    Ok(())
}
pub fn recover(root: &Path) -> Result<()> {
    let _operation = lock(root, "operation.lock", true)?;
    let _recording = lock(root, "recording.lock", true)?;
    let _process = lock(root, "process.lock", true)?;
    recover_locked(root)
}
fn apply_inner(root: &Path, bundle: &Path) -> Result<()> {
    let _operation = lock(root, "operation.lock", true)?;
    let _recording = lock(root, "recording.lock", true)
        .map_err(|_| "Recording is still active. Update canceled; no pipeline was stopped.")?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let _process = loop {
        match lock(root, "process.lock", true) {
            Ok(f) => break f,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => {
                return Err(
                    "Other Vidfish desktops/services are still open. Close them and retry.".into(),
                );
            }
        }
    };
    recover_locked(root)?;
    let manifest = verify(bundle)?;
    let dest = root.join("versions").join(&manifest.version);
    if !dest.exists() {
        copy_bundle(bundle, &dest, &manifest)?;
    } else if verify(&dest)?.files != manifest.files {
        return Err("Conflicting installed version".into());
    }
    let result = apply_locked(root, &manifest.version);
    if let Err(e) = &result {
        atomic(&root.join("update-error.txt"), e.to_string().as_bytes())?;
    }
    drop(_process);
    drop(_recording);
    drop(_operation);

    result
}
pub fn apply(root: &Path, bundle: &Path, relaunch: bool, config: Option<&Path>) -> Result<()> {
    prefix_from_root(root)?;
    let result = apply_inner(root, bundle);
    if let Err(error) = &result {
        let _ = atomic(&root.join("update-error.txt"), error.to_string().as_bytes());
    }
    if relaunch {
        let prefix = prefix_from_root(root)?;
        let launcher = prefix.join(if cfg!(windows) {
            "bin/vidfish.cmd"
        } else {
            "bin/vidfish"
        });
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd.exe");
            c.arg("/C").arg(launcher);
            c
        } else {
            Command::new(launcher)
        };
        if let Some(config) = config {
            cmd.arg("--config").arg(config);
        }
        cmd.spawn()?;
    }
    result
}
pub fn uninstall(prefix: &Path) -> Result<()> {
    let root = root(prefix);
    let owned: Install = read_json(&root.join("install.json"))?;
    if owned.schema != 1 {
        return Err("Unsupported install manifest".into());
    }
    let _operation = lock(&root, "operation.lock", true)?;
    let _recording = lock(&root, "recording.lock", true)?;
    let _process = lock(&root, "process.lock", true)?;
    for (name, hash) in &owned.files {
        let path = prefix.join(relative(name)?);
        if !path.is_symlink() && path.is_file() && digest(&path)? == *hash {
            fs::remove_file(path)?;
        }
    }
    // Remove only verified, manifest-owned files, leaving modified/unrecognized data intact.
    for entry in fs::read_dir(root.join("versions"))? {
        let dir = entry?.path();
        if !dir.is_dir() || dir.is_symlink() {
            continue;
        }
        let Ok(bundle) = verify(&dir) else { continue };
        for name in bundle.files.keys() {
            fs::remove_file(dir.join(relative(name)?))?;
        }
        fs::remove_file(dir.join("bundle.json"))?;
        remove_empty_dirs(&dir)?;
    }
    for name in [
        "current.txt",
        "previous.txt",
        "transaction.json",
        "update-error.txt",
        "install.json",
        "health.json",
    ] {
        let path = root.join(name);
        if path.is_file() {
            fs::remove_file(path)?;
        }
    }
    println!(
        "Uninstalled owned Vidfish files. Settings, credentials, sessions and modified files are retained."
    );
    Ok(())
}
fn remove_empty_dirs(path: &Path) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            remove_empty_dirs(&entry.path())?;
        }
    }
    let _ = fs::remove_dir(path);
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn bundle(base: &Path, v: &str, good: bool) -> PathBuf {
        let path = base.join(v);
        fs::create_dir_all(path.join("assets")).unwrap();
        fs::write(
            path.join("vidfish"),
            format!(
                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'vidfish {v}'; else exit {}; fi\n",
                if good { 0 } else { 1 }
            ),
        )
        .unwrap();
        executable(&path.join("vidfish")).unwrap();
        for p in [
            "vidfish-maintenance",
            "assets/vidfish.png",
            "assets/vidfish.ico",
            "launch-vidfish.sh",
        ] {
            fs::write(path.join(p), b"owned").unwrap();
        }
        let mut files = BTreeMap::new();
        inventory(&path, &path, &mut files).unwrap();
        let b = Bundle {
            schema: 1,
            version: v.into(),
            platform: platform().into(),
            files,
        };
        fs::write(path.join("bundle.json"), serde_json::to_vec(&b).unwrap()).unwrap();
        path
    }
    #[test]
    fn install_upgrade_rollback_uninstall_preserves_unowned_files() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path().join("prefix");
        let first = bundle(tmp.path(), "0.1.1", true);
        let second = bundle(tmp.path(), "0.1.2", true);
        let bad = bundle(tmp.path(), "0.1.3", false);
        install(&prefix, &first).unwrap();
        let r = root(&prefix);
        fs::write(r.join("operator-data.mkv"), b"science").unwrap();
        install(&prefix, &second).unwrap();
        assert_eq!(current(&r).unwrap(), "0.1.2");
        assert!(apply(&r, &bad, false, None).is_err());
        assert_eq!(current(&r).unwrap(), "0.1.2");
        assert!(r.join("versions/0.1.1/vidfish").exists());
        uninstall(&prefix).unwrap();
        assert_eq!(fs::read(r.join("operator-data.mkv")).unwrap(), b"science");
        assert!(!prefix.join("bin/vidfish").exists());
        install(&prefix, &first).unwrap();
    }
    #[test]
    fn recorder_lock_blocks_install_after_desktop_has_gone_and_prevents_new_start() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path().join("p");
        let first = bundle(tmp.path(), "0.1.1", true);
        install(&prefix, &first).unwrap();
        let r = root(&prefix);
        let recorder = lock(&r, "recording.lock", false).unwrap();
        assert!(apply(&r, &first, false, None).is_err());
        assert!(uninstall(&prefix).is_err());
        drop(recorder);
        let updater = lock(&r, "recording.lock", true).unwrap();
        assert!(lock(&r, "recording.lock", false).is_err());
        drop(updater);
        assert!(apply(&r, &first, false, None).is_ok());
    }
    #[test]
    fn interrupted_transaction_recovers_prior_verified_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path().join("p");
        let first = bundle(tmp.path(), "0.1.1", true);
        install(&prefix, &first).unwrap();
        let r = root(&prefix);
        atomic(
            &r.join("transaction.json"),
            br#"{"previous":"0.1.1","next":"0.1.2"}"#,
        )
        .unwrap();
        atomic(&r.join("current.txt"), b"0.1.2").unwrap();
        recover(&r).unwrap();
        assert_eq!(current(&r).unwrap(), "0.1.1");
        assert!(!r.join("transaction.json").exists());
    }
    #[test]
    fn modified_and_unknown_bundle_files_are_not_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path().join("p");
        let first = bundle(tmp.path(), "0.1.1", true);
        install(&prefix, &first).unwrap();
        let r = root(&prefix);
        let file = r.join("versions/0.1.1/science.mkv");
        fs::write(&file, b"science").unwrap();
        let launcher = prefix.join("bin/vidfish");
        fs::write(&launcher, b"custom").unwrap();
        uninstall(&prefix).unwrap();
        assert_eq!(fs::read(file).unwrap(), b"science");
        assert_eq!(fs::read(launcher).unwrap(), b"custom");
    }
    #[test]
    fn corrupted_bundle_and_unowned_launchers_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path().join("p");
        let first = bundle(tmp.path(), "0.1.1", true);
        fs::write(first.join("vidfish"), b"bad").unwrap();
        assert!(install(&prefix, &first).is_err());
        let second = bundle(tmp.path(), "0.1.2", true);
        fs::create_dir_all(prefix.join("bin")).unwrap();
        fs::write(prefix.join("bin/vidfish"), b"user").unwrap();
        assert!(install(&prefix, &second).is_err());
        assert_eq!(fs::read(prefix.join("bin/vidfish")).unwrap(), b"user");
    }
}

#[cfg(windows)]
fn update_uninstall_version(version: &str) {
    let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\VidfishScientificAcquisition_is1";
    if Command::new("reg.exe")
        .args(["query", key])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
    {
        let _ = Command::new("reg.exe")
            .args([
                "add",
                key,
                "/v",
                "DisplayVersion",
                "/t",
                "REG_SZ",
                "/d",
                version,
                "/f",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[cfg(unix)]
fn sync_directories(path: &Path) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_directories(&entry.path())?;
        }
    }
    File::open(path)?.sync_all()
}
