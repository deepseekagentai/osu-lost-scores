use std::path::{Path, PathBuf};

use lost_scores_app::updates::{self, Download, Platform, Release};

const RELEASES_URL: &str = "https://github.com/kz-lemon4ik/osu-lost-scores/releases/";
pub const DAY_S: i64 = 24 * 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    NoReleases,
    Latest,
    Updated(String),
    Manual {
        version: String,
        reason: String,
        page: Option<String>,
    },
}

impl Outcome {
    pub fn text(&self) -> String {
        match self {
            Self::NoReleases => "No releases yet".to_owned(),
            Self::Latest => "You have the latest version".to_owned(),
            Self::Updated(version) => {
                format!("Updated to version {version}; restart the program to use it")
            }
            Self::Manual {
                version, reason, ..
            } => format!("Version {version} is out, but it was not installed: {reason}"),
        }
    }
}

pub fn official() -> Option<Platform> {
    option_env!("LOST_SCORES_OFFICIAL").and(Platform::current())
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
    let page = release
        .url
        .starts_with(RELEASES_URL)
        .then(|| release.url.clone());
    let manual = |reason: String| Outcome::Manual {
        version: version.clone(),
        reason,
        page: page.clone(),
    };
    let Some(platform) = platform else {
        return manual("this copy does not update itself".to_owned());
    };
    match updates::prepare(&release, current, platform, key, download) {
        Ok(Some(update)) => match updates::install(platform, program, &update) {
            Ok(()) => Outcome::Updated(update.version),
            Err(error) => manual(format!("{error:#}")),
        },
        Ok(None) => Outcome::Latest,
        Err(error) => manual(format!("{error:#}")),
    }
}

pub fn leftovers(platform: Option<Platform>, program: &Path) -> Vec<PathBuf> {
    match platform {
        Some(Platform::Windows) => vec![program.to_path_buf()],
        Some(Platform::Linux) => vec![program.to_path_buf(), program.with_file_name("lost-scores")],
        Some(Platform::MacOs) => updates::bundle_of(program).into_iter().collect(),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use anyhow::{Result, bail};
    use lost_scores_app::updates::{Asset, Download, PUBLIC_KEY, Platform, Release};

    use super::{DAY_S, Outcome, due, leftovers, update};

    struct Nothing;

    impl Download for Nothing {
        fn bytes(&mut self, url: &str) -> Result<Vec<u8>> {
            bail!("{url} was not expected")
        }
    }

    fn release(tag: &str, url: &str, names: &[&str]) -> Release {
        Release {
            tag: tag.to_owned(),
            url: url.to_owned(),
            assets: names
                .iter()
                .map(|name| Asset {
                    name: (*name).to_owned(),
                    url: format!("https://files/{name}"),
                })
                .collect(),
        }
    }

    const PAGE: &str = "https://github.com/kz-lemon4ik/osu-lost-scores/releases/tag/v9.0.0";

    #[test]
    fn no_release_or_the_same_version_changes_nothing() {
        let program = Path::new("/nowhere/lost-scores-gui");
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
                Some(release("v1.0.0", PAGE, &[])),
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
    fn an_unsigned_release_is_left_to_the_player() {
        let outcome = update(
            Some(release(
                "v9.0.0",
                PAGE,
                &["lost-scores-9.0.0-windows-x64.exe", "SHA256SUMS"],
            )),
            "1.0.0",
            Some(Platform::Windows),
            Path::new("/nowhere/lost-scores-gui.exe"),
            PUBLIC_KEY,
            &mut Nothing,
        );
        let Outcome::Manual {
            version,
            reason,
            page,
        } = outcome
        else {
            panic!("{outcome:?}");
        };
        assert_eq!(version, "9.0.0");
        assert!(reason.contains("not signed"), "{reason}");
        assert_eq!(page.as_deref(), Some(PAGE));
    }

    #[test]
    fn a_copy_built_from_source_only_points_at_the_release() {
        let outcome = update(
            Some(release("v9.0.0", "https://example.com/v9.0.0", &[])),
            "1.0.0",
            None,
            Path::new("/nowhere/lost-scores-gui"),
            PUBLIC_KEY,
            &mut Nothing,
        );
        assert_eq!(
            outcome,
            Outcome::Manual {
                version: "9.0.0".to_owned(),
                reason: "this copy does not update itself".to_owned(),
                page: None,
            }
        );
        assert_eq!(
            outcome.text(),
            "Version 9.0.0 is out, but it was not installed: this copy does not update itself"
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

    #[test]
    fn leftovers_of_an_update_are_looked_for_beside_the_program() {
        let program = Path::new("/apps/Lost Scores.app/Contents/MacOS/lost-scores-gui");
        assert_eq!(
            leftovers(Some(Platform::MacOs), program),
            [Path::new("/apps/Lost Scores.app")]
        );
        assert_eq!(
            leftovers(Some(Platform::Linux), Path::new("/bin/lost-scores-gui")),
            [
                Path::new("/bin/lost-scores-gui"),
                Path::new("/bin/lost-scores")
            ]
        );
        assert!(leftovers(None, program).is_empty());
    }
}
