use lost_scores_app::updates::Release;

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
            open: Some(release.url.clone()),
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
            url: format!("https://github.com/example/releases/tag/{tag}"),
        }
    }

    #[test]
    fn update_messages_cover_newer_same_and_none() {
        assert_eq!(
            update_message("0.1.0", Some(&release("v0.2.0"))),
            UpdateMessage {
                text: "Version v0.2.0 is available".to_owned(),
                open: Some("https://github.com/example/releases/tag/v0.2.0".to_owned()),
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
