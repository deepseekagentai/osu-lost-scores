use std::ffi::OsString;
use std::fs;
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
    match (numbers(tag), numbers(current)) {
        (Some(tag), Some(current)) => tag > current,
        _ => tag.trim_start_matches(['v', 'V']) != current.trim_start_matches(['v', 'V']),
    }
}

pub trait Download {
    fn bytes(&mut self, url: &str) -> Result<Vec<u8>>;
}

impl Download for ureq::Agent {
    fn bytes(&mut self, url: &str) -> Result<Vec<u8>> {
        Ok(self
            .get(url)
            .call()?
            .body_mut()
            .with_config()
            .limit(MAX_DOWNLOAD)
            .read_to_vec()?)
    }
}

pub fn verified_sum(sums: &[u8], signature: &str, public_key: &str, file: &str) -> Result<String> {
    let key = PublicKey::from_base64(public_key).map_err(|error| anyhow!("{error}"))?;
    let signature = Signature::decode(signature).map_err(|error| anyhow!("{error}"))?;
    key.verify(sums, &signature, false)
        .map_err(|error| anyhow!("the release signature does not match: {error}"))?;
    std::str::from_utf8(sums)?
        .lines()
        .find_map(|line| {
            let (hash, name) = line.split_once(char::is_whitespace)?;
            (name.trim_start().trim_start_matches('*') == file).then(|| hash.to_ascii_lowercase())
        })
        .with_context(|| format!("the release lists no checksum for {file}"))
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

#[derive(Debug, PartialEq, Eq)]
pub struct Update {
    pub version: String,
    pub bytes: Vec<u8>,
}

pub fn prepare(
    release: &Release,
    current: &str,
    platform: Platform,
    public_key: &str,
    download: &mut dyn Download,
) -> Result<Option<Update>> {
    if !is_newer(&release.tag, current) {
        return Ok(None);
    }
    let version = release.tag.trim_start_matches(['v', 'V']).to_owned();
    let file = platform.file(&version);
    let url = |name: &str| {
        release
            .assets
            .iter()
            .find(|asset| asset.name == name)
            .map(|asset| asset.url.clone())
    };
    let signature_url = url(SIGNATURE).context("the release is not signed")?;
    let sums_url = url(SUMS).context("the release has no checksums")?;
    let file_url = url(&file).with_context(|| format!("the release has no {file}"))?;
    let sums = download.bytes(&sums_url)?;
    let signature = String::from_utf8(download.bytes(&signature_url)?)?;
    let expected = verified_sum(&sums, &signature, public_key, &file)?;
    let bytes = download.bytes(&file_url)?;
    check_sum(&bytes, &expected)?;
    Ok(Some(Update { version, bytes }))
}

fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub fn install_file(target: &Path, bytes: &[u8]) -> Result<()> {
    let fresh = beside(target, ".new");
    let old = beside(target, ".old");
    fs::write(&fresh, bytes).with_context(|| format!("cannot write {}", fresh.display()))?;
    executable(&fresh)?;
    let _ = fs::remove_file(&old);
    fs::rename(target, &old).with_context(|| format!("cannot move {}", target.display()))?;
    if let Err(error) = fs::rename(&fresh, target) {
        let _ = fs::rename(&old, target);
        return Err(anyhow!("cannot put the new program in place: {error}"));
    }
    Ok(())
}

fn unpack(work: &Path, tool: &mut Command) -> Result<()> {
    let status = tool.status().context("cannot start the unpacking tool")?;
    ensure!(
        status.success(),
        "the update could not be unpacked in {}",
        work.display()
    );
    Ok(())
}

pub fn install_from_tar(target: &Path, archive: &[u8], version: &str) -> Result<()> {
    let parent = target.parent().context("the program has no folder")?;
    let work = parent.join(WORK);
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;
    let result = (|| {
        let file = work.join("update.tar.gz");
        fs::write(&file, archive)?;
        unpack(
            &work,
            Command::new("tar")
                .arg("-xzf")
                .arg(&file)
                .arg("-C")
                .arg(&work),
        )?;
        let unpacked = work.join(format!("lost-scores-{version}-linux-x64"));
        install_file(target, &fs::read(unpacked.join("lost-scores-gui"))?)?;
        let cli = parent.join("lost-scores");
        if cli.is_file() {
            install_file(&cli, &fs::read(unpacked.join("lost-scores"))?)?;
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&work);
    result
}

pub fn swap_bundle(bundle: &Path, fresh: &Path) -> Result<()> {
    let old = beside(bundle, ".old");
    let _ = fs::remove_dir_all(&old);
    fs::rename(bundle, &old).with_context(|| format!("cannot move {}", bundle.display()))?;
    if let Err(error) = fs::rename(fresh, bundle) {
        let _ = fs::rename(&old, bundle);
        return Err(anyhow!("cannot put the new program in place: {error}"));
    }
    Ok(())
}

pub fn install_bundle(bundle: &Path, archive: &[u8]) -> Result<()> {
    let parent = bundle.parent().context("the program has no folder")?;
    let work = parent.join(WORK);
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;
    let result = (|| {
        let file = work.join("update.zip");
        fs::write(&file, archive)?;
        unpack(
            &work,
            Command::new("ditto")
                .args(["-x", "-k"])
                .arg(&file)
                .arg(&work),
        )?;
        swap_bundle(bundle, &work.join("Lost Scores.app"))
    })();
    let _ = fs::remove_dir_all(&work);
    result
}

pub fn bundle_of(program: &Path) -> Option<PathBuf> {
    let bundle = program.ancestors().nth(3)?;
    bundle
        .extension()
        .is_some_and(|extension| extension == "app")
        .then(|| bundle.to_path_buf())
}

pub fn install(platform: Platform, program: &Path, update: &Update) -> Result<()> {
    match platform {
        Platform::Windows => install_file(program, &update.bytes),
        Platform::Linux => install_from_tar(program, &update.bytes, &update.version),
        Platform::MacOs => install_bundle(
            &bundle_of(program).context("the program is not inside an .app bundle")?,
            &update.bytes,
        ),
    }
}

pub fn clean_up(target: &Path) {
    let old = beside(target, ".old");
    if old.is_dir() {
        let _ = fs::remove_dir_all(&old);
    } else {
        let _ = fs::remove_file(&old);
    }
    let _ = fs::remove_file(beside(target, ".new"));
    if let Some(parent) = target.parent() {
        let _ = fs::remove_dir_all(parent.join(WORK));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use anyhow::{Result, anyhow};

    use super::{
        Asset, Download, PUBLIC_KEY, Platform, Release, bundle_of, check_sum, clean_up,
        install_file, is_newer, parse_release, prepare, swap_bundle, verified_sum,
    };

    const TEST_KEY: &str = "RWT4qOlmRvAyTEn8Jjsu5Qqzej6v4pYcdTVRWW53SnLBPxbVexasWtfu";
    const SUMS: &str = "4715d0af987417a8f4b79e8d8df2c4a7c69d8b9a8ffc801c058bc9739ef1fac5  lost-scores-1.2.3-windows-x64.exe\n";
    const SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRUT4qOlmRvAyTH9VbayyqDW3XNTgHPw/FDFVsMCLkg1M/tYv3HRPLFenmaSS0K7/NHJVttXTZqTshf6CnfoXH3fiRCd49sb5hgo=\ntrusted comment: lost-scores v1.2.3\nBxrAvi8rINbbVUcZ9suE9m1jR1kQYGbhrBwkEC6W21fJSWqOzoVohOdOccC49uDjAFnVXrQkktQRSKI+voU/Dw==\n";
    const PROGRAM: &[u8] = b"new program 1.2.3\n";
    const EXE: &str = "lost-scores-1.2.3-windows-x64.exe";

    struct Files(HashMap<String, Vec<u8>>);

    impl Download for Files {
        fn bytes(&mut self, url: &str) -> Result<Vec<u8>> {
            self.0
                .get(url)
                .cloned()
                .ok_or_else(|| anyhow!("{url} is missing"))
        }
    }

    fn release(names: &[&str]) -> Release {
        Release {
            tag: "v1.2.3".into(),
            url: "https://github.com/x/y/releases/tag/v1.2.3".into(),
            assets: names
                .iter()
                .map(|name| Asset {
                    name: (*name).into(),
                    url: format!("https://files/{name}"),
                })
                .collect(),
        }
    }

    fn files(program: &[u8]) -> Files {
        Files(HashMap::from([
            (
                "https://files/SHA256SUMS".to_owned(),
                SUMS.as_bytes().to_vec(),
            ),
            (
                "https://files/SHA256SUMS.minisig".to_owned(),
                SIGNATURE.as_bytes().to_vec(),
            ),
            (format!("https://files/{EXE}"), program.to_vec()),
        ]))
    }

    #[test]
    fn a_signed_list_gives_the_checksum_of_a_file() {
        assert_eq!(
            verified_sum(SUMS.as_bytes(), SIGNATURE, TEST_KEY, EXE).unwrap(),
            "4715d0af987417a8f4b79e8d8df2c4a7c69d8b9a8ffc801c058bc9739ef1fac5"
        );
        assert!(verified_sum(SUMS.as_bytes(), SIGNATURE, TEST_KEY, "other.exe").is_err());
    }

    #[test]
    fn a_list_changed_or_signed_by_another_key_is_refused() {
        assert!(verified_sum(SUMS.as_bytes(), SIGNATURE, PUBLIC_KEY, EXE).is_err());
        let changed = SUMS.replace("4715", "0000");
        assert!(verified_sum(changed.as_bytes(), SIGNATURE, TEST_KEY, EXE).is_err());
    }

    #[test]
    fn the_downloaded_file_must_match_its_checksum() {
        let expected = verified_sum(SUMS.as_bytes(), SIGNATURE, TEST_KEY, EXE).unwrap();
        assert!(check_sum(PROGRAM, &expected).is_ok());
        assert!(check_sum(b"something else", &expected).is_err());
    }

    #[test]
    fn a_newer_signed_release_is_prepared_and_anything_else_is_not() {
        let all = [EXE, "SHA256SUMS", "SHA256SUMS.minisig"];
        let update = prepare(
            &release(&all),
            "1.2.2",
            Platform::Windows,
            TEST_KEY,
            &mut files(PROGRAM),
        )
        .unwrap()
        .unwrap();
        assert_eq!(update.version, "1.2.3");
        assert_eq!(update.bytes, PROGRAM);
        assert_eq!(
            prepare(
                &release(&all),
                "1.2.3",
                Platform::Windows,
                TEST_KEY,
                &mut files(PROGRAM)
            )
            .unwrap(),
            None
        );
        let unsigned = prepare(
            &release(&[EXE, "SHA256SUMS"]),
            "1.2.2",
            Platform::Windows,
            TEST_KEY,
            &mut files(PROGRAM),
        );
        assert!(unsigned.unwrap_err().to_string().contains("not signed"));
        assert!(
            prepare(
                &release(&all),
                "1.2.2",
                Platform::Windows,
                TEST_KEY,
                &mut files(b"tampered")
            )
            .is_err()
        );
        assert!(
            prepare(
                &release(&all),
                "1.2.2",
                Platform::Windows,
                PUBLIC_KEY,
                &mut files(PROGRAM)
            )
            .is_err()
        );
        assert!(
            prepare(
                &release(&all),
                "1.2.2",
                Platform::Linux,
                TEST_KEY,
                &mut files(PROGRAM)
            )
            .is_err()
        );
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
    fn versions_compare_by_their_numbers() {
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(!is_newer("v0.7.27", "0.7.27"));
        assert!(!is_newer("0.7.26", "0.7.27"));
    }

    #[test]
    fn the_new_program_takes_the_old_ones_place_until_the_next_start() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("lost-scores-gui.exe");
        fs::write(&program, b"old program").unwrap();
        install_file(&program, PROGRAM).unwrap();
        assert_eq!(fs::read(&program).unwrap(), PROGRAM);
        assert_eq!(
            fs::read(dir.path().join("lost-scores-gui.exe.old")).unwrap(),
            b"old program"
        );
        clean_up(&program);
        assert!(!dir.path().join("lost-scores-gui.exe.old").exists());
        assert!(program.is_file());
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
        swap_bundle(&bundle, &fresh).unwrap();
        assert_eq!(fs::read(&program).unwrap(), b"new");
        assert!(dir.path().join("Lost Scores.app.old").is_dir());
        clean_up(&bundle);
        assert!(!dir.path().join("Lost Scores.app.old").exists());
        assert_eq!(bundle_of(&dir.path().join("lost-scores-gui")), None);
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
        let packed = dir.path().join("pack").join("lost-scores-1.2.3-linux-x64");
        fs::create_dir_all(&packed).unwrap();
        fs::write(packed.join("lost-scores-gui"), b"new window").unwrap();
        fs::write(packed.join("lost-scores"), b"new cli").unwrap();
        let archive = dir.path().join("update.tar.gz");
        let status = std::process::Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(dir.path().join("pack"))
            .arg("lost-scores-1.2.3-linux-x64")
            .status()
            .unwrap();
        assert!(status.success());
        super::install_from_tar(&program, &fs::read(&archive).unwrap(), "1.2.3").unwrap();
        assert_eq!(fs::read(&program).unwrap(), b"new window");
        assert_eq!(fs::read(installed.join("lost-scores")).unwrap(), b"new cli");
        assert!(!installed.join(".lost-scores-update").exists());
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
