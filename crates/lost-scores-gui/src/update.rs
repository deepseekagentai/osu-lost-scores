use lost_scores_app::updates::Release;

const RELEASES_URL: &str = "https://github.com/kz-lemon4ik/osu-lost-scores/releases/";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateMessage {
    pub text: String,
    pub open: Option<String>,
}

fn numbers(version: &str) -> Option<Vec<u64>> {
    version
        .trim_start_matches(['v', 'V'])
        .split('.')
        .map(|part| part.parse().ok())
        .collect()
}

fn is_newer(tag: &str, current: &str) -> bool {
    match (numbers(tag), numbers(current)) {
        (Some(tag), Some(current)) => tag > current,
        _ => tag.trim_start_matches(['v', 'V']) != current.trim_start_matches(['v', 'V']),
    }
}

pub fn update_message(current: &str, release: Option<&Release>) -> UpdateMessage {
    match release {
        None => UpdateMessage {
            text: "No releases yet".to_owned(),
            open: None,
        },
        Some(release) if is_newer(&release.tag, current) => UpdateMessage {
            text: format!("Version {} is available", release.tag),
            open: release
                .url
                .starts_with(RELEASES_URL)
                .then(|| release.url.clone()),
        },
        Some(_) => UpdateMessage {
            text: "You have the latest version".to_owned(),
            open: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use lost_scores_app::updates::Release;

    use super::{UpdateMessage, update_message};

    fn release(tag: &str) -> Release {
        Release {
            tag: tag.to_owned(),
            url: format!("https://github.com/kz-lemon4ik/osu-lost-scores/releases/tag/{tag}"),
        }
    }

    #[test]
    fn only_release_pages_of_this_project_are_opened() {
        for url in [
            "https://example.com/phish",
            "http://github.com/kz-lemon4ik/osu-lost-scores/releases/tag/v9",
            "https://github.com/kz-lemon4ik/osu-lost-scores-evil/releases/tag/v9",
            "file:///etc/passwd",
        ] {
            let message = update_message(
                "0.1.0",
                Some(&Release {
                    tag: "v9.0.0".to_owned(),
                    url: url.to_owned(),
                }),
            );
            assert_eq!(message.text, "Version v9.0.0 is available");
            assert_eq!(message.open, None, "{url}");
        }
    }

    #[test]
    fn update_messages_cover_newer_same_and_none() {
        assert_eq!(
            update_message("0.1.0", Some(&release("v0.2.0"))),
            UpdateMessage {
                text: "Version v0.2.0 is available".to_owned(),
                open: Some(
                    "https://github.com/kz-lemon4ik/osu-lost-scores/releases/tag/v0.2.0".to_owned()
                ),
            }
        );
        assert_eq!(
            update_message("0.1.0", Some(&release("v0.1.0"))),
            UpdateMessage {
                text: "You have the latest version".to_owned(),
                open: None,
            }
        );
        assert_eq!(
            update_message("0.2.0", Some(&release("v0.1.9"))).text,
            "You have the latest version"
        );
        assert_eq!(
            update_message("0.9.0", Some(&release("v0.10.0"))).text,
            "Version v0.10.0 is available"
        );
        assert_eq!(
            update_message("0.1.0", None),
            UpdateMessage {
                text: "No releases yet".to_owned(),
                open: None,
            }
        );
    }
}
