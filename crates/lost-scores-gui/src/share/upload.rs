use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};

use crate::share::render::ShareImage;

pub const CATBOX_API: &str = "https://catbox.moe/user/api.php";
const LINK_PREFIX: &str = "https://files.catbox.moe/";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const MIN_TOTAL: Duration = Duration::from_secs(60);
const SLOWEST_BYTES_PER_SECOND: u64 = 50_000;
const MAX_ANSWER: u64 = 64 * 1024;
const ERROR_TEXT_CHARS: usize = 120;

pub fn multipart_body(boundary: &str, file_name: &str, png: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(png.len() + 512);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"reqtype\"\r\n\r\nfileupload\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"fileToUpload\"; filename=\"{file_name}\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(png);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

pub fn parse_link(status: u16, body: &str) -> Result<String> {
    let text = body.trim();
    if status == 200 && text.starts_with(LINK_PREFIX) && !text.contains(char::is_whitespace) {
        return Ok(text.to_owned());
    }
    let shown: String = text
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(ERROR_TEXT_CHARS)
        .collect();
    Err(anyhow!("catbox answered {status}: {shown}"))
}

pub fn total_timeout(bytes: u64) -> Duration {
    MIN_TOTAL.max(Duration::from_secs(bytes / SLOWEST_BYTES_PER_SECOND))
}

pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .user_agent(lost_scores_app::online::USER_AGENT)
        .http_status_as_error(false)
        .build()
        .into()
}

fn boundary() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("----LostScores{nanos:x}")
}

pub fn upload(agent: &ureq::Agent, file: &Path) -> Result<String> {
    let png = std::fs::read(file).with_context(|| format!("cannot read {}", file.display()))?;
    let name = file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("image.png");
    let boundary = boundary();
    let body = multipart_body(&boundary, name, &png);
    let mut response = agent
        .post(CATBOX_API)
        .config()
        .timeout_global(Some(total_timeout(png.len() as u64)))
        .build()
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .send(&body[..])
        .context("cannot reach catbox")?;
    let status = response.status().as_u16();
    let answer = response
        .body_mut()
        .with_config()
        .limit(MAX_ANSWER)
        .read_to_string()
        .unwrap_or_default();
    parse_link(status, &answer)
}

#[derive(Clone, Copy, Debug)]
pub struct Consent {
    remembered: bool,
}

impl Consent {
    pub fn new(remembered: bool) -> Self {
        Self { remembered }
    }

    pub fn must_ask(&self) -> bool {
        !self.remembered
    }

    pub fn accept(&mut self, dont_ask_again: bool) {
        self.remembered |= dont_ask_again;
    }

    pub fn remembered(&self) -> bool {
        self.remembered
    }
}

#[derive(Default)]
pub struct Choices {
    shown: Vec<ShareImage>,
    pending: Option<ShareImage>,
}

impl Choices {
    pub fn replace(&mut self, shown: Vec<ShareImage>) {
        self.shown = shown;
    }

    pub fn all(&self) -> &[ShareImage] {
        &self.shown
    }

    pub fn get(&self, index: usize) -> Option<ShareImage> {
        self.shown.get(index).cloned()
    }

    pub fn hold(&mut self, index: usize) -> bool {
        self.pending = self.get(index);
        self.pending.is_some()
    }

    pub fn release(&mut self) -> Option<ShareImage> {
        self.pending.take()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use std::time::Duration;

    use super::{Choices, Consent, multipart_body, parse_link, total_timeout};
    use crate::share::render::ShareImage;

    const LINK: &str = "https://files.catbox.moe/abc123.png";

    #[test]
    fn the_body_has_reqtype_and_the_file() {
        let png = [0x89, b'P', b'N', b'G', 0, 13, 10, 255, 0];
        let body = multipart_body("XBOUNDARY", "lost_scores.png", &png);
        let text = String::from_utf8_lossy(&body);
        assert!(text.starts_with("--XBOUNDARY\r\n"));
        assert!(text.contains(
            "Content-Disposition: form-data; name=\"reqtype\"\r\n\r\nfileupload\r\n--XBOUNDARY\r\n"
        ));
        assert!(text.contains(
            "Content-Disposition: form-data; name=\"fileToUpload\"; filename=\"lost_scores.png\"\r\nContent-Type: image/png\r\n\r\n"
        ));
        assert!(text.ends_with("\r\n--XBOUNDARY--\r\n"));
        assert!(body.windows(png.len()).any(|window| window == png));
        assert!(!text.contains("userhash"));
    }

    #[test]
    fn only_a_catbox_link_is_accepted() {
        assert_eq!(parse_link(200, &format!("{LINK}\n")).unwrap(), LINK);
        let page = "<html><body>Something went wrong</body></html>";
        assert!(parse_link(200, page).is_err());
        let error = parse_link(412, "Not allowed").unwrap_err().to_string();
        assert!(error.contains("412"), "{error}");
        let error = parse_link(500, LINK).unwrap_err().to_string();
        assert!(error.contains("500"), "{error}");
    }

    #[test]
    fn the_total_timeout_grows_with_the_file() {
        assert_eq!(total_timeout(0), Duration::from_secs(60));
        assert_eq!(total_timeout(1_300_000), Duration::from_secs(60));
        assert_eq!(total_timeout(3_000_000), Duration::from_secs(60));
        assert_eq!(total_timeout(10_000_000), Duration::from_secs(200));
        assert_eq!(total_timeout(20_000_000), Duration::from_secs(400));
    }

    #[test]
    fn the_first_upload_asks_first() {
        let mut consent = Consent::new(false);
        assert!(consent.must_ask());
        consent.accept(false);
        assert!(consent.must_ask());
        assert!(!consent.remembered());
        consent.accept(true);
        assert!(!consent.must_ask());
        assert!(consent.remembered());
        assert!(!Consent::new(true).must_ask());
    }

    fn image(label: &'static str, path: &str) -> ShareImage {
        ShareImage {
            label,
            path: PathBuf::from(path),
        }
    }

    #[test]
    fn the_sheet_keeps_the_chosen_image_when_the_list_changes() {
        let mut choices = Choices::default();
        choices.replace(vec![
            image("Lost Scores", "/a/lost.png"),
            image("Badge", "/a/badge.png"),
        ]);
        assert!(choices.hold(1));
        choices.replace(vec![image("Lost Scores", "/b/lost.png")]);
        assert_eq!(choices.get(1), None);
        assert_eq!(choices.release(), Some(image("Badge", "/a/badge.png")));
        assert_eq!(choices.release(), None);
        assert!(!choices.hold(5));
    }
}
