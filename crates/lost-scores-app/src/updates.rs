use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail, ensure};
use minisign_verify::{PublicKey, Signature};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const LATEST: &str = "https://api.github.com/repos/kz-lemon4ik/osu-lost-scores/releases/latest";
pub const PUBLIC_KEY: &str = "RWTdGCBD/sd6R9S9//74p+bhwfQvkEoXgmSdC0yoxrBOOo/IcTOfMyud";
const SUMS: &str = "SHA256SUMS";
const SIGNATURE: &str = "SHA256SUMS.minisig";
const MAX_DOWNLOAD: u64 = 256 * 1024 * 1024;
const MAX_LIST: u64 = 64 * 1024;
const WORK: &str = ".lost-scores-update";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub url: String,
    pub assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct LatestAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct Latest {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<LatestAsset>,
}

pub fn latest_release(agent: &ureq::Agent) -> Result<Option<Release>> {
    let response = agent
        .get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .call();
    let (status, body) = match response {
        Ok(mut response) => (
            response.status().as_u16(),
            response.body_mut().read_to_string()?,
        ),
        Err(ureq::Error::StatusCode(status)) => (status, String::new()),
        Err(error) => return Err(error.into()),
    };
    parse_release(status, &body)
}

pub fn parse_release(status: u16, body: &str) -> Result<Option<Release>> {
    match status {
        404 => Ok(None),
        200 => {
            let latest: Latest = serde_json::from_str(body)?;
            Ok(Some(Release {
                tag: latest.tag_name,
                url: latest.html_url,
                assets: latest
                    .assets
                    .into_iter()
                    .map(|asset| Asset {
                        name: asset.name,
                        url: asset.browser_download_url,
                    })
                    .collect(),
            }))
        }
        other => bail!("the release lookup answered with status {other}"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    MacOs,
}

impl Platform {
    pub fn current() -> Option<Self> {
        if cfg!(all(windows, target_arch = "x86_64")) {
            Some(Self::Windows)
        } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            Some(Self::Linux)
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Some(Self::MacOs)
        } else {
            None
        }
    }

    pub fn file(self, version: &str) -> String {
        match self {
            Self::Windows => format!("lost-scores-{version}-windows-x64.exe"),
            Self::Linux => format!("lost-scores-{version}-linux-x64.tar.gz"),
            Self::MacOs => format!("lost-scores-{version}-macos-arm64.zip"),
        }
    }
}

fn numbers(version: &str) -> Option<Vec<u64>> {
    version
        .trim_start_matches(['v', 'V'])
        .split('.')
        .map(|part| part.parse().ok())
        .collect()
}

pub fn is_newer(tag: &str, current: &str) -> bool {
    matches!((numbers(tag), numbers(current)), (Some(tag), Some(current)) if tag > current)
}

pub fn release_page(tag: &str) -> String {
    format!("https://github.com/kz-lemon4ik/osu-lost-scores/releases/tag/{tag}")
}

pub trait Download {
    fn bytes(&mut self, url: &str, limit: u64) -> Result<Vec<u8>>;
}

impl Download for ureq::Agent {
    fn bytes(&mut self, url: &str, limit: u64) -> Result<Vec<u8>> {
        Ok(self
            .get(url)
            .call()?
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()?)
    }
}

#[derive(Debug)]
pub enum Refusal {
    Network(anyhow::Error),
    Rejected(String),
}

pub fn verified_sum(
    sums: &[u8],
    signature: &str,
    public_key: &str,
    tag: &str,
    file: &str,
) -> Result<String> {
    let key = PublicKey::from_base64(public_key).map_err(|error| anyhow!("{error}"))?;
    let signature = Signature::decode(signature).map_err(|error| anyhow!("{error}"))?;
    key.verify(sums, &signature, false)
        .map_err(|error| anyhow!("the release signature does not match: {error}"))?;
    ensure!(
        signature.trusted_comment() == format!("lost-scores {tag}"),
        "the signature belongs to another release"
    );
    let mut found = None;
    for line in std::str::from_utf8(sums)?.lines() {
        let (hash, name) = line
            .split_once("  ")
            .or_else(|| line.split_once(" *"))
            .context("the checksum list has a malformed line")?;
        ensure!(
            hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "the checksum list has a malformed line"
        );
        if name == file {
            ensure!(found.is_none(), "the checksum list names {file} twice");
            found = Some(hash.to_ascii_lowercase());
        }
    }
    found.with_context(|| format!("the release lists no checksum for {file}"))
}

pub fn check_sum(bytes: &[u8], expected: &str) -> Result<()> {
    let found: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    ensure!(
        found == expected,
        "the downloaded file does not match its checksum"
    );
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    pub version: String,
    file_url: String,
    sum: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Update {
    pub version: String,
    pub bytes: Vec<u8>,
}

pub fn verify(
    release: &Release,
    platform: Platform,
    public_key: &str,
    download: &mut dyn Download,
) -> Result<Verified, Refusal> {
    let version = release.tag.trim_start_matches(['v', 'V']).to_owned();
    let file = platform.file(&version);
    let url = |name: &str| {
        release
            .assets
            .iter()
            .find(|asset| asset.name == name)
            .map(|asset| asset.url.clone())
            .ok_or_else(|| Refusal::Rejected(format!("the release has no {name}")))
    };
    let signature_url = url(SIGNATURE)?;
    let sums_url = url(SUMS)?;
    let file_url = url(&file)?;
    let sums = download
        .bytes(&sums_url, MAX_LIST)
        .map_err(Refusal::Network)?;
    let signature = download
        .bytes(&signature_url, MAX_LIST)
        .map_err(Refusal::Network)?;
    let signature = String::from_utf8(signature)
        .map_err(|_| Refusal::Rejected("the signature is not text".to_owned()))?;
    let sum = verified_sum(&sums, &signature, public_key, &release.tag, &file)
        .map_err(|error| Refusal::Rejected(format!("{error:#}")))?;
    Ok(Verified {
        version,
        file_url,
        sum,
    })
}

pub fn fetch(verified: &Verified, download: &mut dyn Download) -> Result<Update, Refusal> {
    let bytes = download
        .bytes(&verified.file_url, MAX_DOWNLOAD)
        .map_err(Refusal::Network)?;
    check_sum(&bytes, &verified.sum).map_err(|error| Refusal::Rejected(format!("{error:#}")))?;
    Ok(Update {
        version: verified.version.clone(),
        bytes,
    })
}

pub fn place_of(platform: Platform, program: &Path) -> Option<PathBuf> {
    match platform {
        Platform::Windows | Platform::Linux => program.parent().map(Path::to_path_buf),
        Platform::MacOs => bundle_of(program)?.parent().map(Path::to_path_buf),
    }
}

pub fn writable(dir: &Path) -> bool {
    tempfile::NamedTempFile::new_in(dir).is_ok()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub old_program: PathBuf,
    pub leftovers: Vec<PathBuf>,
}

fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn stage(target: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let fresh = beside(target, ".new");
    let mut file =
        fs::File::create(&fresh).with_context(|| format!("cannot write {}", fresh.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fresh, fs::Permissions::from_mode(0o755))?;
    }
    Ok(fresh)
}

fn swap_in(target: &Path, fresh: &Path) -> Result<PathBuf> {
    let old = beside(target, ".old");
    let _ = fs::remove_file(&old);
    #[cfg(unix)]
    {
        fs::hard_link(target, &old).with_context(|| format!("cannot keep {}", target.display()))?;
        if let Err(error) = fs::rename(fresh, target) {
            let _ = fs::remove_file(&old);
            return Err(anyhow!("cannot put the new program in place: {error}"));
        }
    }
    #[cfg(not(unix))]
    {
        fs::rename(target, &old).with_context(|| format!("cannot move {}", target.display()))?;
        if let Err(error) = fs::rename(fresh, target) {
            let _ = fs::rename(&old, target);
            return Err(anyhow!("cannot put the new program in place: {error}"));
        }
    }
    Ok(old)
}

pub fn install_file(target: &Path, bytes: &[u8]) -> Result<Installed> {
    let fresh = stage(target, bytes)?;
    let old = swap_in(target, &fresh).inspect_err(|_| {
        let _ = fs::remove_file(&fresh);
    })?;
    Ok(Installed {
        old_program: old.clone(),
        leftovers: vec![old],
    })
}

fn work_folder(parent: &Path) -> Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .prefix(WORK)
        .tempdir_in(parent)
        .with_context(|| format!("cannot unpack the update in {}", parent.display()))
}

fn unpack(tool: &mut Command) -> Result<()> {
    let status = tool.status().context("cannot start the unpacking tool")?;
    ensure!(status.success(), "the update could not be unpacked");
    Ok(())
}

pub fn install_from_tar(target: &Path, archive: &[u8], version: &str) -> Result<Installed> {
    let parent = target.parent().context("the program has no folder")?;
    let work = work_folder(parent)?;
    let file = work.path().join("update.tar.gz");
    fs::write(&file, archive)?;
    unpack(
        Command::new("tar")
            .arg("-xzf")
            .arg(&file)
            .arg("-C")
            .arg(work.path()),
    )?;
    let unpacked = work.path().join(format!("lost-scores-{version}-linux-x64"));
    let window = fs::read(unpacked.join("lost-scores-gui"))?;
    let cli_target = parent.join("lost-scores");
    let cli = if cli_target.is_file() {
        Some(fs::read(unpacked.join("lost-scores"))?)
    } else {
        None
    };
    let fresh_window = stage(target, &window)?;
    let fresh_cli = match cli.map(|bytes| stage(&cli_target, &bytes)).transpose() {
        Ok(fresh) => fresh,
        Err(error) => {
            let _ = fs::remove_file(&fresh_window);
            return Err(error);
        }
    };
    let old_window = swap_in(target, &fresh_window).inspect_err(|_| {
        let _ = fs::remove_file(&fresh_window);
        if let Some(fresh) = &fresh_cli {
            let _ = fs::remove_file(fresh);
        }
    })?;
    let mut leftovers = vec![old_window.clone()];
    if let Some(fresh) = fresh_cli {
        match swap_in(&cli_target, &fresh) {
            Ok(old_cli) => leftovers.push(old_cli),
            Err(error) => {
                let _ = fs::remove_file(&fresh);
                let _ = fs::rename(&old_window, target);
                return Err(error);
            }
        }
    }
    Ok(Installed {
        old_program: old_window,
        leftovers,
    })
}

pub fn swap_bundle(bundle: &Path, fresh: &Path) -> Result<PathBuf> {
    let old = beside(bundle, ".old");
    let _ = fs::remove_dir_all(&old);
    fs::rename(bundle, &old).with_context(|| format!("cannot move {}", bundle.display()))?;
    if let Err(error) = fs::rename(fresh, bundle) {
        let _ = fs::rename(&old, bundle);
        return Err(anyhow!("cannot put the new program in place: {error}"));
    }
    Ok(old)
}

pub fn install_bundle(bundle: &Path, archive: &[u8]) -> Result<Installed> {
    let parent = bundle.parent().context("the program has no folder")?;
    let work = work_folder(parent)?;
    let file = work.path().join("update.zip");
    fs::write(&file, archive)?;
    unpack(
        Command::new("ditto")
            .args(["-x", "-k"])
            .arg(&file)
            .arg(work.path()),
    )?;
    let old = swap_bundle(bundle, &work.path().join("Lost Scores.app"))?;
    Ok(Installed {
        old_program: old.join("Contents").join("MacOS").join("lost-scores-gui"),
        leftovers: vec![old],
    })
}

pub fn bundle_of(program: &Path) -> Option<PathBuf> {
    let bundle = program.ancestors().nth(3)?;
    bundle
        .extension()
        .is_some_and(|extension| extension == "app")
        .then(|| bundle.to_path_buf())
}

pub fn install(platform: Platform, program: &Path, update: &Update) -> Result<Installed> {
    match platform {
        Platform::Windows => install_file(program, &update.bytes),
        Platform::Linux => install_from_tar(program, &update.bytes, &update.version),
        Platform::MacOs => install_bundle(
            &bundle_of(program).context("the program is not inside an .app bundle")?,
            &update.bytes,
        ),
    }
}

pub fn remove_leftover(path: &Path) {
    if path.is_dir() {
        let _ = fs::remove_dir_all(path);
    } else {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use anyhow::{Result, anyhow, ensure};

    use super::{
        Asset, Download, PUBLIC_KEY, Platform, Refusal, Release, bundle_of, check_sum, fetch,
        install_file, is_newer, parse_release, place_of, remove_leftover, swap_bundle,
        verified_sum, verify, writable,
    };

    const TEST_KEY: &str = "RWT4qOlmRvAyTEn8Jjsu5Qqzej6v4pYcdTVRWW53SnLBPxbVexasWtfu";
    const SUMS: &str = "4715d0af987417a8f4b79e8d8df2c4a7c69d8b9a8ffc801c058bc9739ef1fac5  lost-scores-1.2.3-windows-x64.exe\n";
    const SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRUT4qOlmRvAyTH9VbayyqDW3XNTgHPw/FDFVsMCLkg1M/tYv3HRPLFenmaSS0K7/NHJVttXTZqTshf6CnfoXH3fiRCd49sb5hgo=\ntrusted comment: lost-scores v1.2.3\nBxrAvi8rINbbVUcZ9suE9m1jR1kQYGbhrBwkEC6W21fJSWqOzoVohOdOccC49uDjAFnVXrQkktQRSKI+voU/Dw==\n";
    const PROGRAM: &[u8] = b"new program 1.2.3\n";
    const EXE: &str = "lost-scores-1.2.3-windows-x64.exe";

    struct Files {
        files: HashMap<String, Vec<u8>>,
        limits: Vec<(String, u64)>,
    }

    impl Download for Files {
        fn bytes(&mut self, url: &str, limit: u64) -> Result<Vec<u8>> {
            self.limits.push((url.to_owned(), limit));
            let bytes = self
                .files
                .get(url)
                .cloned()
                .ok_or_else(|| anyhow!("{url} is missing"))?;
            ensure!(bytes.len() as u64 <= limit, "{url} is too big");
            Ok(bytes)
        }
    }

    fn release(tag: &str, names: &[&str]) -> Release {
        Release {
            tag: tag.into(),
            url: format!("https://github.com/x/y/releases/tag/{tag}"),
            assets: names
                .iter()
                .map(|name| Asset {
                    name: (*name).into(),
                    url: format!("https://files/{name}"),
                })
                .collect(),
        }
    }

    fn files(sums: &[u8], program: &[u8]) -> Files {
        Files {
            files: HashMap::from([
                ("https://files/SHA256SUMS".to_owned(), sums.to_vec()),
                (
                    "https://files/SHA256SUMS.minisig".to_owned(),
                    SIGNATURE.as_bytes().to_vec(),
                ),
                (format!("https://files/{EXE}"), program.to_vec()),
            ]),
            limits: Vec::new(),
        }
    }

    const ALL: [&str; 3] = [EXE, "SHA256SUMS", "SHA256SUMS.minisig"];

    #[test]
    fn a_signed_list_gives_the_checksum_of_a_file() {
        assert_eq!(
            verified_sum(SUMS.as_bytes(), SIGNATURE, TEST_KEY, "v1.2.3", EXE).unwrap(),
            "4715d0af987417a8f4b79e8d8df2c4a7c69d8b9a8ffc801c058bc9739ef1fac5"
        );
        assert!(verified_sum(SUMS.as_bytes(), SIGNATURE, TEST_KEY, "v1.2.3", "other.exe").is_err());
    }

    #[test]
    fn a_list_changed_signed_by_another_key_or_for_another_tag_is_refused() {
        assert!(verified_sum(SUMS.as_bytes(), SIGNATURE, PUBLIC_KEY, "v1.2.3", EXE).is_err());
        let changed = SUMS.replace("4715", "0000");
        assert!(verified_sum(changed.as_bytes(), SIGNATURE, TEST_KEY, "v1.2.3", EXE).is_err());
        let other = verified_sum(SUMS.as_bytes(), SIGNATURE, TEST_KEY, "v1.2.4", EXE).unwrap_err();
        assert!(other.to_string().contains("another release"), "{other}");
    }

    #[test]
    fn the_downloaded_file_must_match_its_checksum() {
        let expected = verified_sum(SUMS.as_bytes(), SIGNATURE, TEST_KEY, "v1.2.3", EXE).unwrap();
        assert!(check_sum(PROGRAM, &expected).is_ok());
        assert!(check_sum(b"something else", &expected).is_err());
    }

    #[test]
    fn a_newer_signed_release_is_verified_then_fetched() {
        let mut download = files(SUMS.as_bytes(), PROGRAM);
        let verified = verify(
            &release("v1.2.3", &ALL),
            Platform::Windows,
            TEST_KEY,
            &mut download,
        )
        .unwrap();
        assert_eq!(verified.version, "1.2.3");
        let update = fetch(&verified, &mut download).unwrap();
        assert_eq!(update.bytes, PROGRAM);
        assert_eq!(download.limits[0].1, 64 * 1024);
        assert_eq!(download.limits[1].1, 64 * 1024);
        assert_eq!(download.limits[2].1, 256 * 1024 * 1024);
    }

    #[test]
    fn unsigned_tampered_or_foreign_releases_are_rejected() {
        let rejected =
            |result: Result<super::Verified, Refusal>| matches!(result, Err(Refusal::Rejected(_)));
        let unsigned = release("v1.2.3", &[EXE, "SHA256SUMS"]);
        assert!(rejected(verify(
            &unsigned,
            Platform::Windows,
            TEST_KEY,
            &mut files(SUMS.as_bytes(), PROGRAM)
        )));
        assert!(rejected(verify(
            &release("v1.2.3", &ALL),
            Platform::Windows,
            PUBLIC_KEY,
            &mut files(SUMS.as_bytes(), PROGRAM)
        )));
        assert!(rejected(verify(
            &release("v1.2.3", &ALL),
            Platform::Linux,
            TEST_KEY,
            &mut files(SUMS.as_bytes(), PROGRAM)
        )));
        let mut download = files(SUMS.as_bytes(), b"tampered");
        let verified = verify(
            &release("v1.2.3", &ALL),
            Platform::Windows,
            TEST_KEY,
            &mut download,
        )
        .unwrap();
        assert!(matches!(
            fetch(&verified, &mut download),
            Err(Refusal::Rejected(_))
        ));
        let huge = vec![b'a'; 64 * 1024 + 1];
        assert!(matches!(
            verify(
                &release("v1.2.3", &ALL),
                Platform::Windows,
                TEST_KEY,
                &mut files(&huge, PROGRAM)
            ),
            Err(Refusal::Network(_))
        ));
    }

    #[test]
    fn each_platform_has_its_release_file() {
        assert_eq!(
            Platform::Windows.file("1.0.0"),
            "lost-scores-1.0.0-windows-x64.exe"
        );
        assert_eq!(
            Platform::Linux.file("1.0.0"),
            "lost-scores-1.0.0-linux-x64.tar.gz"
        );
        assert_eq!(
            Platform::MacOs.file("1.0.0"),
            "lost-scores-1.0.0-macos-arm64.zip"
        );
    }

    #[test]
    fn only_a_higher_plain_version_is_newer() {
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(!is_newer("v0.7.27", "0.7.27"));
        assert!(!is_newer("0.7.26", "0.7.27"));
        assert!(!is_newer("v1.9.1", "2.0.0-rc.1"));
        assert!(!is_newer("v0.9.0-beta", "1.4.0"));
        assert!(!is_newer("v2.0.0-rc.1", "1.0.0"));
    }

    #[test]
    fn the_new_program_takes_the_old_ones_place_and_the_old_one_stays_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("lost-scores-gui.exe");
        fs::write(&program, b"old program").unwrap();
        let installed = install_file(&program, PROGRAM).unwrap();
        assert_eq!(fs::read(&program).unwrap(), PROGRAM);
        assert_eq!(
            installed.old_program,
            dir.path().join("lost-scores-gui.exe.old")
        );
        assert_eq!(fs::read(&installed.old_program).unwrap(), b"old program");
        assert!(!dir.path().join("lost-scores-gui.exe.new").exists());
        assert_eq!(
            place_of(Platform::Windows, &program).as_deref(),
            Some(dir.path())
        );
        assert!(writable(dir.path()));
        fs::write(dir.path().join("my backup.old"), b"keep").unwrap();
        for leftover in &installed.leftovers {
            remove_leftover(leftover);
        }
        assert!(!installed.old_program.exists());
        assert!(dir.path().join("my backup.old").exists());
    }

    #[test]
    fn a_new_app_bundle_replaces_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Lost Scores.app");
        let program = bundle
            .join("Contents")
            .join("MacOS")
            .join("lost-scores-gui");
        fs::create_dir_all(program.parent().unwrap()).unwrap();
        fs::write(&program, b"old").unwrap();
        let fresh = dir.path().join("work").join("Lost Scores.app");
        fs::create_dir_all(fresh.join("Contents").join("MacOS")).unwrap();
        fs::write(
            fresh.join("Contents").join("MacOS").join("lost-scores-gui"),
            b"new",
        )
        .unwrap();
        assert_eq!(bundle_of(&program).as_deref(), Some(bundle.as_path()));
        assert_eq!(
            place_of(Platform::MacOs, &program).as_deref(),
            Some(dir.path())
        );
        let old = swap_bundle(&bundle, &fresh).unwrap();
        assert_eq!(fs::read(&program).unwrap(), b"new");
        assert_eq!(old, dir.path().join("Lost Scores.app.old"));
        remove_leftover(&old);
        assert!(!old.exists());
        assert_eq!(bundle_of(&dir.path().join("lost-scores-gui")), None);
    }

    #[cfg(target_os = "linux")]
    fn linux_archive(dir: &std::path::Path, with_cli: bool) -> Vec<u8> {
        let packed = dir.join("pack").join("lost-scores-1.2.3-linux-x64");
        fs::create_dir_all(&packed).unwrap();
        fs::write(packed.join("lost-scores-gui"), b"new window").unwrap();
        if with_cli {
            fs::write(packed.join("lost-scores"), b"new cli").unwrap();
        }
        let archive = dir.join("update.tar.gz");
        let status = std::process::Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(dir.join("pack"))
            .arg("lost-scores-1.2.3-linux-x64")
            .status()
            .unwrap();
        assert!(status.success());
        fs::read(&archive).unwrap()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_linux_archive_replaces_the_window_and_the_command_line() {
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("bin");
        fs::create_dir_all(&installed).unwrap();
        let program = installed.join("lost-scores-gui");
        fs::write(&program, b"old window").unwrap();
        fs::write(installed.join("lost-scores"), b"old cli").unwrap();
        let archive = linux_archive(dir.path(), true);
        let done = super::install_from_tar(&program, &archive, "1.2.3").unwrap();
        assert_eq!(fs::read(&program).unwrap(), b"new window");
        assert_eq!(fs::read(installed.join("lost-scores")).unwrap(), b"new cli");
        assert_eq!(fs::read(&done.old_program).unwrap(), b"old window");
        assert_eq!(done.leftovers.len(), 2);
        let names: Vec<String> = fs::read_dir(&installed)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !names
                .iter()
                .any(|name| name.starts_with(".lost-scores-update")),
            "{names:?}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_linux_archive_without_the_command_line_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("bin");
        fs::create_dir_all(&installed).unwrap();
        let program = installed.join("lost-scores-gui");
        fs::write(&program, b"old window").unwrap();
        fs::write(installed.join("lost-scores"), b"old cli").unwrap();
        let archive = linux_archive(dir.path(), false);
        assert!(super::install_from_tar(&program, &archive, "1.2.3").is_err());
        assert_eq!(fs::read(&program).unwrap(), b"old window");
        assert_eq!(fs::read(installed.join("lost-scores")).unwrap(), b"old cli");
        assert!(!installed.join("lost-scores-gui.new").exists());
    }

    #[test]
    fn a_missing_release_is_none() {
        assert_eq!(
            parse_release(404, "{\"message\":\"Not Found\"}").unwrap(),
            None
        );
    }

    #[test]
    fn a_release_gives_its_tag_and_page() {
        let body = r#"{"tag_name":"v1.5.0","html_url":"https://github.com/x/y/releases/tag/v1.5.0","draft":false}"#;
        let release = parse_release(200, body).unwrap().unwrap();
        assert_eq!(release.tag, "v1.5.0");
        assert_eq!(release.url, "https://github.com/x/y/releases/tag/v1.5.0");
    }

    #[test]
    fn other_statuses_are_errors() {
        assert!(parse_release(500, "").is_err());
        assert!(parse_release(200, "not json").is_err());
    }
}
