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
    pending: Vec<ShareImage>,
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

    pub fn hold_all(&mut self) -> bool {
        self.pending = self.shown.clone();
        !self.pending.is_empty()
    }

    pub fn release(&mut self) -> Vec<ShareImage> {
        std::mem::take(&mut self.pending)
    }
}

pub const LINKS_IN_THE_LOG: &str = "Links in the log";

pub struct Summary {
    pub links: String,
    pub message: String,
    pub label: String,
    pub tone: i32,
    pub first_error: String,
}

pub fn links_summary(results: &[(&str, Result<String, String>)]) -> Summary {
    let links: Vec<&str> = results
        .iter()
        .filter_map(|(_, result)| result.as_deref().ok())
        .collect();
    let message = match (links.len(), results.len()) {
        (0, _) => "Could not upload: see the log".to_owned(),
        (1, 1) => "Link copied".to_owned(),
        (done, total) if done == total => format!("{done} links copied"),
        (done, total) => format!("Uploaded {done} of {total}: see the log"),
    };
    let (label, tone) = match (links.len(), results.len()) {
        (0, _) => ("Upload failed".to_owned(), -1),
        (done, total) if done == total => ("Links copied".to_owned(), 1),
        (done, total) => (format!("Uploaded {done} of {total}"), -1),
    };
    let first_error = results
        .iter()
        .find_map(|(name, result)| result.as_ref().err().map(|why| format!("{name}: {why}")))
        .unwrap_or_default();
    Summary {
        links: links.join("\n"),
        message,
        label,
        tone,
        first_error,
    }
}

pub fn progress_label(done: usize, total: usize) -> String {
    if total == 1 {
        "Uploading...".to_owned()
    } else {
        format!("Uploading {done} of {total}")
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use std::time::Duration;

    use super::{
        Choices, Consent, links_summary, multipart_body, parse_link, progress_label, total_timeout,
    };
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
    fn the_sheet_keeps_every_picture_it_was_shown_with() {
        let mut choices = Choices::default();
        choices.replace(vec![
            image("Badge", "/a/badge.png"),
            image("Lost Scores", "/a/lost.png"),
        ]);
        assert!(choices.hold_all());
        choices.replace(vec![image("Lost Scores", "/b/lost.png")]);
        assert_eq!(
            choices.release(),
            [
                image("Badge", "/a/badge.png"),
                image("Lost Scores", "/a/lost.png")
            ]
        );
        assert!(choices.release().is_empty());
        choices.replace(Vec::new());
        assert!(!choices.hold_all());
    }

    #[test]
    fn the_links_and_the_message_follow_what_was_uploaded() {
        let ok = |link: &str| Ok(link.to_owned());
        let failed = || Err("timeout".to_owned());
        let all = links_summary(&[
            ("Badge", ok("https://files.catbox.moe/a.png")),
            ("Lost Scores", ok("https://files.catbox.moe/b.png")),
            ("Potential Top", ok("https://files.catbox.moe/c.png")),
        ]);
        assert_eq!(
            all.links,
            "https://files.catbox.moe/a.png\nhttps://files.catbox.moe/b.png\nhttps://files.catbox.moe/c.png"
        );
        assert_eq!(all.message, "3 links copied");
        let some = links_summary(&[
            ("Badge", ok("https://files.catbox.moe/a.png")),
            ("Lost Scores", failed()),
        ]);
        assert_eq!(some.links, "https://files.catbox.moe/a.png");
        assert_eq!(some.message, "Uploaded 1 of 2: see the log");
        let none = links_summary(&[("Badge", failed())]);
        assert_eq!(none.links, "");
        assert_eq!(none.message, "Could not upload: see the log");
        let one = links_summary(&[("Badge", ok("https://files.catbox.moe/a.png"))]);
        assert_eq!(one.message, "Link copied");
    }

    #[test]
    fn the_button_shows_how_the_upload_ended() {
        let ok = |link: &str| Ok(link.to_owned());
        let failed = |why: &str| Err(why.to_owned());
        let all = links_summary(&[
            ("Badge", ok("https://files.catbox.moe/a.png")),
            ("Lost Scores", ok("https://files.catbox.moe/b.png")),
        ]);
        assert_eq!((all.label.as_str(), all.tone), ("Links copied", 1));
        assert_eq!(all.first_error, "");
        let some = links_summary(&[
            ("Badge", ok("https://files.catbox.moe/a.png")),
            ("Lost Scores", failed("catbox answered 412: no file")),
            ("Potential Top", failed("timeout")),
        ]);
        assert_eq!((some.label.as_str(), some.tone), ("Uploaded 1 of 3", -1));
        assert_eq!(
            some.first_error,
            "Lost Scores: catbox answered 412: no file"
        );
        let none = links_summary(&[("Badge", failed("cannot reach catbox"))]);
        assert_eq!((none.label.as_str(), none.tone), ("Upload failed", -1));
        assert_eq!(none.first_error, "Badge: cannot reach catbox");
    }

    #[test]
    fn the_button_counts_the_pictures_while_uploading() {
        assert_eq!(progress_label(1, 3), "Uploading 1 of 3");
        assert_eq!(progress_label(1, 1), "Uploading...");
    }
}
