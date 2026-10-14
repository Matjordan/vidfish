//! Finite, opt-in downloads from the official stable release. SHA256 is integrity,
//! not a publisher signature. All filesystem mutation belongs to maintenance.
use crate::installation::{self, Result};
use reqwest::{Url, blocking::Client};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
const API: &str = "https://api.github.com/repos/Matjordan/vidfish/releases/latest";
const MAX_PAYLOAD: u64 = 2 * 1024 * 1024 * 1024;
#[derive(Clone, Debug)]
pub struct Offer {
    pub version: String,
    pub notes: String,
    pub size: u64,
    pub asset: String,
    pub url: String,
    pub checksum_url: String,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    size: u64,
    browser_download_url: String,
}
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    body: Option<String>,
    assets: Vec<Asset>,
}
fn client(timeout: Duration) -> Result<Client> {
    Ok(Client::builder()
        .user_agent("Vidfish-updater")
        .connect_timeout(Duration::from_secs(5))
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let url = attempt.url();
            if attempt.previous().len() < 5
                && url.scheme() == "https"
                && url.port_or_known_default() == Some(443)
                && matches!(
                    url.host_str(),
                    Some(
                        "github.com"
                            | "api.github.com"
                            | "release-assets.githubusercontent.com"
                            | "objects.githubusercontent.com"
                    )
                )
            {
                attempt.follow()
            } else {
                attempt.error("Unexpected update redirect")
            }
        }))
        .build()?)
}
fn bounded(mut input: impl Read, max: u64) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    input.by_ref().take(max + 1).read_to_end(&mut data)?;
    if data.len() as u64 > max {
        return Err("Release response too large".into());
    }
    Ok(data)
}
fn asset_url(url: &str, tag: &str, name: &str) -> Result<()> {
    let url = Url::parse(url)?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != format!("/Matjordan/vidfish/releases/download/{tag}/{name}")
    {
        return Err("Release asset URL is not the expected official GitHub URL".into());
    }
    Ok(())
}
pub fn check(current: &str) -> Result<Option<Offer>> {
    let response = client(Duration::from_secs(15))?
        .get(API)
        .send()?
        .error_for_status()?;
    parse_release(&bounded(response, 1024 * 1024)?, current)
}
pub fn parse_release(bytes: &[u8], current: &str) -> Result<Option<Offer>> {
    if !cfg!(target_arch = "x86_64") || !cfg!(any(target_os = "linux", target_os = "windows")) {
        return Ok(None);
    }
    let release: Release = serde_json::from_slice(bytes)?;
    let version = release
        .tag_name
        .strip_prefix('v')
        .ok_or("Invalid release tag")?;
    installation::version(version)?;
    let next = semver::Version::parse(version)?;
    let current = semver::Version::parse(current)?;
    if release.draft || release.prerelease || !next.pre.is_empty() || next <= current {
        return Ok(None);
    }
    let name = format!("vidfish-{version}-{}-update.zip", installation::platform());
    let assets: Vec<_> = release.assets.iter().filter(|a| a.name == name).collect();
    let sums: Vec<_> = release
        .assets
        .iter()
        .filter(|a| a.name == "SHA256SUMS")
        .collect();
    if assets.is_empty() || sums.is_empty() {
        return Ok(None);
    }
    if assets.len() != 1 || sums.len() != 1 {
        return Err("Duplicate release assets".into());
    }
    let asset = assets[0];
    let sums = sums[0];
    if asset.size == 0 || asset.size > MAX_PAYLOAD || sums.size == 0 || sums.size > 1024 * 1024 {
        return Err("Invalid release asset size".into());
    }
    asset_url(&asset.browser_download_url, &release.tag_name, &name)?;
    asset_url(&sums.browser_download_url, &release.tag_name, "SHA256SUMS")?;
    Ok(Some(Offer {
        version: version.into(),
        notes: release
            .body
            .unwrap_or_default()
            .chars()
            .take(12000)
            .collect(),
        size: asset.size,
        asset: name,
        url: asset.browser_download_url.clone(),
        checksum_url: sums.browser_download_url.clone(),
    }))
}
pub fn checksum(bytes: &[u8], name: &str) -> Result<String> {
    let text = std::str::from_utf8(bytes)?;
    let mut found = None;
    for line in text.lines() {
        let Some((hash, file)) = line.split_once(' ') else {
            return Err("Malformed checksum file".into());
        };
        let file = file.trim_start_matches([' ', '*']);
        if hash.len() != 64 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) || file.is_empty() {
            return Err("Malformed checksum entry".into());
        }
        if file == name {
            if found.is_some() {
                return Err("Duplicate payload checksum".into());
            }
            found = Some(hash.to_ascii_lowercase());
        }
    }
    found.ok_or_else(|| "Required update checksum is missing".into())
}
pub fn download(root: &Path, offer: &Offer, progress: impl Fn(u64, u64)) -> Result<PathBuf> {
    installation::version(&offer.version)?;
    let tag = format!("v{}", offer.version);
    asset_url(&offer.url, &tag, &offer.asset)?;
    asset_url(&offer.checksum_url, &tag, "SHA256SUMS")?;
    let expected_name = format!(
        "vidfish-{}-{}-update.zip",
        offer.version,
        installation::platform()
    );
    if offer.asset != expected_name || offer.size == 0 || offer.size > MAX_PAYLOAD {
        return Err("Invalid update payload selection".into());
    }
    let client = client(Duration::from_secs(20 * 60))?;
    let sums = client.get(&offer.checksum_url).send()?.error_for_status()?;
    let hash = checksum(&bounded(sums, 1024 * 1024)?, &offer.asset)?;
    let response = client.get(&offer.url).send()?.error_for_status()?;
    stage_download(root, offer, response, &hash, progress)
}
fn stage_download(
    root: &Path,
    offer: &Offer,
    mut response: impl Read,
    hash: &str,
    progress: impl Fn(u64, u64),
) -> Result<PathBuf> {
    let staging = root.join("staging");
    fs::create_dir_all(&staging)?;
    let temp = tempfile::tempdir_in(&staging)?;
    let archive = temp.path().join("download.zip");
    let mut file = File::create(&archive)?;
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    loop {
        let n = response.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > offer.size {
            return Err("Download exceeded expected size".into());
        }
        file.write_all(&buffer[..n])?;
        progress(total, offer.size);
    }
    file.sync_all()?;
    drop(file);
    if total != offer.size || installation::digest(&archive)? != hash {
        return Err(
            "Update SHA256/size verification failed; installed application is unchanged".into(),
        );
    }
    let bundle = temp.path().join("bundle");
    fs::create_dir(&bundle)?;
    extract(&archive, &bundle)?;
    let manifest = installation::verify(&bundle)?;
    if manifest.version != offer.version {
        return Err("Payload version differs from release".into());
    }
    fs::remove_file(&archive)?;
    let retained = temp.keep();
    Ok(retained.join("bundle"))
}
pub fn extract(archive: &Path, destination: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(File::open(archive)?)?;
    if zip.len() > 20000 {
        return Err("Too many archive entries".into());
    }
    let mut expanded = 0u64;
    let mut names = BTreeSet::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        let name = file.name().to_owned();
        if name.contains(['\\', ':'])
            || !names.insert(name.to_ascii_lowercase())
            || file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
        {
            return Err("Unsafe/duplicate archive entry".into());
        }
        let path = file.enclosed_name().ok_or("Unsafe archive path")?;
        if path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err("Unsafe archive path".into());
        }
        let path = destination.join(path);
        expanded = expanded
            .checked_add(file.size())
            .ok_or("Archive size overflow")?;
        if expanded > 4 * MAX_PAYLOAD {
            return Err("Expanded archive exceeds limit".into());
        }
        if file.is_dir() {
            fs::create_dir_all(path)?;
            continue;
        }
        fs::create_dir_all(path.parent().ok_or("Missing parent")?)?;
        let mut out = File::create(&path)?;
        let count = std::io::copy(&mut file, &mut out)?;
        if count != file.size() {
            return Err("Truncated archive entry".into());
        }
        out.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if file.unix_mode().is_some_and(|m| m & 0o111 != 0) {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn release(version: &str) -> serde_json::Value {
        let tag = format!("v{version}");
        let name = format!("vidfish-{version}-{}-update.zip", installation::platform());
        serde_json::json!({"tag_name":tag,"draft":false,"prerelease":false,"body":"Notes","assets":[{"name":name,"size":42,"browser_download_url":format!("https://github.com/Matjordan/vidfish/releases/download/{tag}/{name}")},{"name":"SHA256SUMS","size":100,"browser_download_url":format!("https://github.com/Matjordan/vidfish/releases/download/{tag}/SHA256SUMS")} ]})
    }
    #[test]
    fn stable_compatible_releases_only() {
        assert!(
            parse_release(&serde_json::to_vec(&release("0.2.0")).unwrap(), "0.1.0")
                .unwrap()
                .is_some()
        );
        for v in ["0.1.0", "0.0.9", "0.2.0-rc.1"] {
            assert!(
                parse_release(&serde_json::to_vec(&release(v)).unwrap(), "0.1.0")
                    .unwrap()
                    .is_none()
            );
        }
        let mut r = release("0.2.0");
        r["assets"][0]["name"] = "wrong-architecture.zip".into();
        assert!(
            parse_release(&serde_json::to_vec(&r).unwrap(), "0.1.0")
                .unwrap()
                .is_none()
        );
        r = release("0.2.0");
        r["assets"][0]["browser_download_url"] = "https://example.com/update.zip".into();
        assert!(parse_release(&serde_json::to_vec(&r).unwrap(), "0.1.0").is_err());
    }
    #[test]
    fn checksum_requires_exact_single_payload() {
        let h = "a".repeat(64);
        assert_eq!(
            checksum(format!("{h}  payload.zip\n").as_bytes(), "payload.zip").unwrap(),
            h
        );
        for text in [
            format!("{h}  other.zip"),
            format!("{h}  payload.zip\n{h}  payload.zip"),
            "bad  payload.zip".into(),
        ] {
            assert!(checksum(text.as_bytes(), "payload.zip").is_err());
        }
    }
    #[test]
    fn archive_traversal_and_symlinks_rejected() {
        for name in ["../outside", "/outside", "C:/outside", "a\\b"] {
            let tmp = tempfile::tempdir().unwrap();
            let p = tmp.path().join("bad.zip");
            let mut z = zip::ZipWriter::new(File::create(&p).unwrap());
            z.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(b"x").unwrap();
            z.finish().unwrap();
            assert!(extract(&p, &tmp.path().join("out")).is_err());
        }
    }
    #[test]
    fn partial_corrupt_and_invalid_downloads_leave_installation_untouched() {
        use sha2::Digest;
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("current.txt"), b"0.1.0").unwrap();
        let mut offer = parse_release(&serde_json::to_vec(&release("0.2.0")).unwrap(), "0.1.0")
            .unwrap()
            .unwrap();
        for (bytes, size, hash) in [
            (b"abc".as_slice(), 4, "f".repeat(64)),
            (b"abc".as_slice(), 3, "f".repeat(64)),
            (
                b"not a zip".as_slice(),
                9,
                format!("{:x}", sha2::Sha256::digest(b"not a zip")),
            ),
        ] {
            offer.size = size;
            assert!(stage_download(tmp.path(), &offer, bytes, &hash, |_, _| {}).is_err());
            assert_eq!(
                std::fs::read(tmp.path().join("current.txt")).unwrap(),
                b"0.1.0"
            );
            assert_eq!(
                std::fs::read_dir(tmp.path().join("staging"))
                    .unwrap()
                    .count(),
                0
            );
        }
    }
}

/// Kept outside AppConfig so an older binary can still read settings after rollback.
#[derive(Clone, Debug, serde::Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub schema_version: u32,
    pub automatic_checks: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            schema_version: 1,
            automatic_checks: true,
        }
    }
}
impl Preferences {
    pub fn load(path: &Path) -> Result<Self> {
        let preferences: Self = match File::open(path) {
            Ok(file) => serde_json::from_slice(&bounded(file, 16384)?)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(e.into()),
        };
        if preferences.schema_version != 1 {
            return Err("Unsupported update preference schema".into());
        }
        Ok(preferences)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        if self.schema_version != 1 {
            return Err("Unsupported update preference schema".into());
        }
        installation::atomic(path, &serde_json::to_vec_pretty(self)?)
    }
}
#[cfg(test)]
mod preference_tests {
    use super::*;
    #[test]
    fn preferences_are_separate_and_corrupt_files_are_preserved() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("config.json");
        std::fs::write(&config, br#"{"auto_record":true}"#).unwrap();
        let path = tmp.path().join("updates.json");
        assert!(Preferences::load(&path).unwrap().automatic_checks);
        Preferences {
            automatic_checks: false,
            ..Default::default()
        }
        .save(&path)
        .unwrap();
        assert!(!Preferences::load(&path).unwrap().automatic_checks);
        assert_eq!(std::fs::read(&config).unwrap(), br#"{"auto_record":true}"#);
        for bytes in [
            b"{".as_slice(),
            br#"{"schema_version":2}"#,
            br#"{"unknown":true}"#,
        ] {
            std::fs::write(&path, bytes).unwrap();
            assert!(Preferences::load(&path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
}
