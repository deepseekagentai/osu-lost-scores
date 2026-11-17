use std::path::Path;

use lost_scores_app::updates::{self, Download, Installed, Platform, Refusal, Release};

pub const DAY_S: i64 = 24 * 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    NoReleases,
    Latest,
    Updated {
        version: String,
        installed: Installed,
    },
    Manual {
        version: String,
        reason: String,
        page: Option<String>,
    },
    Unverified,
    Failed(String),
}

impl Outcome {
    pub fn text(&self) -> String {
        match self {
            Self::NoReleases => "No releases yet".to_owned(),
            Self::Latest => "You have the latest version".to_owned(),
            Self::Updated { version, .. } => {
                format!("Updated to version {version}; restart the program to use it")
            }
            Self::Manual {
                version, reason, ..
            } => format!("Version {version} is out, but it was not installed: {reason}"),
            Self::Unverified => {
                "An update was found, but it could not be verified, so it was not installed"
                    .to_owned()
            }
            Self::Failed(reason) => format!("Could not check for updates: {reason}"),
        }
    }

    pub fn quiet(&self) -> bool {
        matches!(self, Self::NoReleases | Self::Latest | Self::Failed(_))
    }
}

pub fn official() -> Option<Platform> {
    option_env!("LOST_SCORES_OFFICIAL")
        .filter(|value| !value.is_empty())
        .and(Platform::current())
}

pub fn due(checked_at: Option<i64>, now: i64) -> bool {
    checked_at.is_none_or(|at| at > now || now - at >= DAY_S)
}

pub fn update(
    release: Option<Release>,
    current: &str,
    platform: Option<Platform>,
    program: &Path,
    key: &str,
    download: &mut dyn Download,
) -> Outcome {
    let Some(release) = release else {
        return Outcome::NoReleases;
    };
    if !updates::is_newer(&release.tag, current) {
        return Outcome::Latest;
    }
    let version = release.tag.trim_start_matches(['v', 'V']).to_owned();
    let manual = |reason: &str| Outcome::Manual {
        version: version.clone(),
        reason: reason.to_owned(),
        page: Some(updates::release_page(&release.tag)),
    };
    let Some(platform) = platform else {
        return manual("this copy does not update itself");
    };
    let refused = |refusal: Refusal| match refusal {
        Refusal::Rejected(_) => Outcome::Unverified,
        Refusal::Network(error) => Outcome::Failed(format!("{error:#}")),
    };
    let verified = match updates::verify(&release, platform, key, download) {
        Ok(verified) => verified,
        Err(refusal) => return refused(refusal),
    };
    if !updates::place_of(platform, program).is_some_and(|dir| updates::writable(&dir)) {
        return manual("the program's folder cannot be written");
    }
    let update = match updates::fetch(&verified, download) {
        Ok(update) => update,
        Err(refusal) => return refused(refusal),
    };
    match updates::install(platform, program, &update) {
        Ok(installed) => Outcome::Updated {
            version: update.version,
            installed,
        },
        Err(error) => manual(&format!("{error:#}")),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use anyhow::{Result, bail};
    use lost_scores_app::updates::{Asset, Download, PUBLIC_KEY, Platform, Release};

    use super::{DAY_S, Outcome, due, update};

    struct Nothing;

    impl Download for Nothing {
        fn bytes(&mut self, url: &str, _limit: u64) -> Result<Vec<u8>> {
            bail!("{url} was not expected")
        }
    }

    struct Offline;

    impl Download for Offline {
        fn bytes(&mut self, _url: &str, _limit: u64) -> Result<Vec<u8>> {
            bail!("no network")
        }
    }

    fn release(tag: &str, names: &[&str]) -> Release {
        Release {
            tag: tag.to_owned(),
            url: "https://example.com/elsewhere".to_owned(),
            assets: names
                .iter()
                .map(|name| Asset {
                    name: (*name).to_owned(),
                    url: format!("https://files/{name}"),
                })
                .collect(),
        }
    }

    const PROGRAM: &str = "/nowhere/lost-scores-gui.exe";

    #[test]
    fn no_release_or_the_same_version_changes_nothing() {
        let program = Path::new(PROGRAM);
        assert_eq!(
            update(
                None,
                "1.0.0",
                Some(Platform::Windows),
                program,
                PUBLIC_KEY,
                &mut Nothing
            ),
            Outcome::NoReleases
        );
        assert_eq!(
            update(
                Some(release("v1.0.0", &[])),
                "1.0.0",
                Some(Platform::Windows),
                program,
                PUBLIC_KEY,
                &mut Nothing
            ),
            Outcome::Latest
        );
    }

    #[test]
    fn a_release_that_fails_the_check_is_not_named_or_linked() {
        let outcome = update(
            Some(release(
                "v9.0.0",
                &["lost-scores-9.0.0-windows-x64.exe", "SHA256SUMS"],
            )),
            "1.0.0",
            Some(Platform::Windows),
            Path::new(PROGRAM),
            PUBLIC_KEY,
            &mut Nothing,
        );
        assert_eq!(outcome, Outcome::Unverified);
        assert!(!outcome.text().contains("9.0.0"));
        assert!(!outcome.quiet());
    }

    #[test]
    fn no_network_is_quiet() {
        let outcome = update(
            Some(release(
                "v9.0.0",
                &[
                    "lost-scores-9.0.0-windows-x64.exe",
                    "SHA256SUMS",
                    "SHA256SUMS.minisig",
                ],
            )),
            "1.0.0",
            Some(Platform::Windows),
            Path::new(PROGRAM),
            PUBLIC_KEY,
            &mut Offline,
        );
        assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
        assert!(outcome.quiet());
    }

    #[test]
    fn a_copy_built_from_source_points_at_the_release_page_built_from_the_tag() {
        let outcome = update(
            Some(release("v9.0.0", &[])),
            "1.0.0",
            None,
            Path::new(PROGRAM),
            PUBLIC_KEY,
            &mut Nothing,
        );
        assert_eq!(
            outcome,
            Outcome::Manual {
                version: "9.0.0".to_owned(),
                reason: "this copy does not update itself".to_owned(),
                page: Some(
                    "https://github.com/kz-lemon4ik/osu-lost-scores/releases/tag/v9.0.0".to_owned()
                ),
            }
        );
    }

    #[test]
    fn the_check_runs_once_a_day() {
        let now = 1_800_000_000;
        assert!(due(None, now));
        assert!(!due(Some(now - 3600), now));
        assert!(due(Some(now - DAY_S), now));
        assert!(due(Some(now + 3600), now));
    }
}
