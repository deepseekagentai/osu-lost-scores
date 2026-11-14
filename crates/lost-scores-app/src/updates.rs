use anyhow::{Result, bail};
use serde::Deserialize;

const LATEST: &str = "https://api.github.com/repos/kz-lemon4ik/osu-lost-scores/releases/latest";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub url: String,
}

#[derive(Deserialize)]
struct Latest {
    tag_name: String,
    html_url: String,
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
            }))
        }
        other => bail!("the release lookup answered with status {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_release;

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
