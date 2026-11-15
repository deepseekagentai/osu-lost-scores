use std::fmt;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use osu_api::{ApiError, Server, Transport, encode};
use sha2::{Digest, Sha256};

use crate::session::Session;

pub const LIMIT: Duration = Duration::from_secs(10 * 60);
const SITE_URL: &str = "https://lost.lemon4ik.kz";
const HEAD_LIMIT: usize = 8 * 1024;
const POLL: Duration = Duration::from_millis(50);
const BROWSER_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Pending {
    pub url: String,
    verifier: String,
    listener: TcpListener,
}

#[derive(Debug)]
pub enum LoginError {
    Cancelled,
    Expired,
    Refused,
    Busy,
    TimedOut,
    Api(ApiError),
    Io(io::Error),
}

impl fmt::Display for LoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoginError::Cancelled => f.write_str("the login was cancelled on osu!"),
            LoginError::Expired => f.write_str("the login expired; try again"),
            LoginError::Refused => f.write_str("osu! did not confirm the login; try again"),
            LoginError::Busy => {
                f.write_str("the Lost Scores server is busy; try again in a minute")
            }
            LoginError::TimedOut => {
                f.write_str("the browser did not finish the login in time; try again")
            }
            LoginError::Api(error) => error.fmt(f),
            LoginError::Io(error) => write!(f, "cannot wait for the browser: {error}"),
        }
    }
}

impl std::error::Error for LoginError {}

pub fn begin<T: Transport>(server: &Server<T>) -> io::Result<Pending> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    let verifier = hex(&bytes);
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let url = server.login_url(&challenge_of(&verifier), listener.local_addr()?.port());
    Ok(Pending {
        url,
        verifier,
        listener,
    })
}

pub fn wait<T: Transport>(
    server: &mut Server<T>,
    pending: Pending,
    limit: Duration,
) -> Result<Session, LoginError> {
    let deadline = Instant::now() + limit;
    let challenge = challenge_of(&pending.verifier);
    pending
        .listener
        .set_nonblocking(true)
        .map_err(LoginError::Io)?;
    loop {
        let stream = match pending.listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(LoginError::TimedOut);
                }
                thread::sleep(POLL);
                continue;
            }
            Err(error) => return Err(LoginError::Io(error)),
        };
        let until = (Instant::now() + BROWSER_TIMEOUT).min(deadline);
        let Some(answer) = answer_of(&stream, until, &challenge) else {
            reply(stream, "404 Not Found", None);
            continue;
        };
        let outcome = match answer {
            Answer::Error(error) => Err(match error.as_str() {
                "access_denied" => LoginError::Cancelled,
                "login_expired" => LoginError::Expired,
                "busy" => LoginError::Busy,
                _ => LoginError::Refused,
            }),
            Answer::Code(code) => match server.redeem(&code, &pending.verifier) {
                Ok(Some(login)) => Ok(Session {
                    token: login.token,
                    osu_id: login.osu_id,
                    username: login.username,
                    server: server.base().to_owned(),
                }),
                Ok(None) => Err(LoginError::Expired),
                Err(error) => Err(LoginError::Api(error)),
            },
        };
        let site = match &outcome {
            Ok(session) => format!(
                "{SITE_URL}/oauth/success?source=desktop&username={}&user_id={}",
                encode(&session.username),
                session.osu_id
            ),
            Err(LoginError::Cancelled) => format!("{SITE_URL}/oauth/success?error=access_denied"),
            Err(LoginError::Expired) => format!("{SITE_URL}/oauth/success?error=login_expired"),
            Err(LoginError::Refused) => format!("{SITE_URL}/oauth/success?error=osu_refused"),
            Err(LoginError::Busy) => format!("{SITE_URL}/oauth/success?error=busy"),
            Err(_) => format!("{SITE_URL}/oauth/success?error=login_failed"),
        };
        reply(stream, "302 Found", Some(&site));
        return outcome;
    }
}

enum Answer {
    Code(String),
    Error(String),
}

fn answer_of(stream: &TcpStream, until: Instant, challenge: &str) -> Option<Answer> {
    stream.set_nonblocking(false).ok()?;
    stream.set_write_timeout(Some(BROWSER_TIMEOUT)).ok()?;
    let head = head_of(stream, until)?;
    let target = head.split_whitespace().nth(1)?;
    let query = target.strip_prefix("/?")?;
    let value = |name: &str| {
        query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_owned())
    };
    if value("challenge").as_deref() != Some(challenge) {
        return None;
    }
    value("error")
        .map(Answer::Error)
        .or_else(|| value("code").map(Answer::Code))
}

fn head_of(mut stream: &TcpStream, until: Instant) -> Option<String> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        let left = until
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())?;
        stream.set_read_timeout(Some(left)).ok()?;
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 || head.len() + read > HEAD_LIMIT {
            return None;
        }
        head.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8(head).ok()
}

fn reply(mut stream: TcpStream, status: &str, location: Option<&str>) {
    let location = location.map_or(String::new(), |url| format!("Location: {url}\r\n"));
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\n{location}Content-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let _ = stream.flush();
}

fn challenge_of(verifier: &str) -> String {
    hex(&Sha256::digest(verifier.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use osu_api::{ApiError, Response, Server, Transport};

    use super::{LIMIT, LoginError, begin, challenge_of, wait};
    use crate::session::Session;

    #[derive(Clone, Default)]
    struct Fake {
        answers: Arc<Mutex<VecDeque<(u16, String)>>>,
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl Fake {
        fn answer(self, status: u16, body: &str) -> Self {
            self.answers
                .lock()
                .unwrap()
                .push_back((status, body.to_owned()));
            self
        }

        fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Transport for Fake {
        fn post_form(&mut self, url: &str, _: &[(&str, &str)]) -> Result<Response, ApiError> {
            panic!("unexpected form to {url}")
        }

        fn get(&mut self, url: &str, _: &str) -> Result<Response, ApiError> {
            panic!("unexpected GET {url}")
        }

        fn post_json(&mut self, url: &str, body: &[u8], _: &str) -> Result<Response, ApiError> {
            self.requests
                .lock()
                .unwrap()
                .push(format!("{url} {}", String::from_utf8_lossy(body)));
            let (status, body) = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("no scripted answer");
            Ok(Response {
                status,
                retry_after: None,
                body: body.into_bytes(),
            })
        }
    }

    fn port_of(url: &str) -> u16 {
        url.rsplit("port=").next().unwrap().parse().unwrap()
    }

    fn challenge_in(url: &str) -> String {
        url.split("challenge=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap()
            .to_owned()
    }

    fn visit(port: u16, target: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            stream,
            "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: text/html\r\n\r\n"
        )
        .unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        reply
    }

    fn browse(port: u16, targets: &[&str]) -> thread::JoinHandle<Vec<String>> {
        let targets: Vec<String> = targets.iter().map(|target| (*target).to_owned()).collect();
        thread::spawn(move || targets.iter().map(|target| visit(port, target)).collect())
    }

    const SITE: &str = "https://lost.lemon4ik.kz/oauth/success";

    #[test]
    fn the_challenge_is_the_sha256_of_the_verifier() {
        assert_eq!(
            challenge_of("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let server = Server::new(Fake::default(), "https://api.test");
        let first = begin(&server).unwrap();
        let second = begin(&server).unwrap();
        assert_eq!(first.verifier.len(), 64);
        assert!(
            first
                .verifier
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert_ne!(first.verifier, second.verifier);
        assert_eq!(
            first.url,
            format!(
                "https://api.test/api/v1/auth/start?challenge={}&port={}",
                challenge_of(&first.verifier),
                first.listener.local_addr().unwrap().port()
            )
        );
    }

    #[test]
    fn waiting_redeems_the_code_from_the_browser_and_sends_it_to_the_site() {
        let fake =
            Fake::default().answer(200, r#"{"token":"tok","osu_id":7,"username":"Some Name"}"#);
        let mut server = Server::new(fake.clone(), "https://api.test");
        let pending = begin(&server).unwrap();
        let challenge = challenge_in(&pending.url);
        let answer = format!("/?code=abc&challenge={challenge}");
        let browser = browse(port_of(&pending.url), &[&answer]);
        let session = wait(&mut server, pending, Duration::from_secs(10)).unwrap();
        assert_eq!(
            session,
            Session {
                token: "tok".into(),
                osu_id: 7,
                username: "Some Name".into(),
                server: "https://api.test".into(),
            }
        );
        let replies = browser.join().unwrap();
        assert!(replies[0].starts_with("HTTP/1.1 302 Found\r\n"));
        assert!(replies[0].contains(&format!(
            "\r\nLocation: {SITE}?source=desktop&username=Some%20Name&user_id=7\r\n"
        )));
        let requests = fake.requests();
        assert_eq!(requests.len(), 1);
        let (url, body) = requests[0].split_once(' ').unwrap();
        assert_eq!(url, "https://api.test/api/v1/auth/token");
        let body: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["code"], "abc");
        assert_eq!(challenge_of(body["verifier"].as_str().unwrap()), challenge);
    }

    #[test]
    fn waiting_reports_a_cancel_on_osu() {
        let fake = Fake::default();
        let mut server = Server::new(fake.clone(), "https://api.test");
        let pending = begin(&server).unwrap();
        let answer = format!(
            "/?error=access_denied&challenge={}",
            challenge_in(&pending.url)
        );
        let browser = browse(port_of(&pending.url), &[&answer]);
        let error = wait(&mut server, pending, Duration::from_secs(10)).unwrap_err();
        assert!(matches!(error, LoginError::Cancelled));
        assert_eq!(error.to_string(), "the login was cancelled on osu!");
        assert!(
            browser.join().unwrap()[0]
                .contains(&format!("\r\nLocation: {SITE}?error=access_denied\r\n"))
        );
        assert!(fake.requests().is_empty());
    }

    #[test]
    fn waiting_reports_a_busy_server() {
        let fake = Fake::default();
        let mut server = Server::new(fake.clone(), "https://api.test");
        let pending = begin(&server).unwrap();
        let answer = format!("/?error=busy&challenge={}", challenge_in(&pending.url));
        let browser = browse(port_of(&pending.url), &[&answer]);
        let error = wait(&mut server, pending, Duration::from_secs(10)).unwrap_err();
        assert!(matches!(error, LoginError::Busy));
        assert_eq!(
            error.to_string(),
            "the Lost Scores server is busy; try again in a minute"
        );
        assert!(
            browser.join().unwrap()[0].contains(&format!("\r\nLocation: {SITE}?error=busy\r\n"))
        );
        assert!(fake.requests().is_empty());
    }

    #[test]
    fn waiting_ignores_other_requests_and_stops_at_the_limit() {
        let fake = Fake::default();
        let mut server = Server::new(fake.clone(), "https://api.test");
        let pending = begin(&server).unwrap();
        let browser = browse(port_of(&pending.url), &["/favicon.ico", "/?state=x"]);
        let error = wait(&mut server, pending, Duration::from_millis(1500)).unwrap_err();
        assert!(matches!(error, LoginError::TimedOut));
        for reply in browser.join().unwrap() {
            assert!(reply.starts_with("HTTP/1.1 404 Not Found\r\n"), "{reply}");
        }
        assert!(fake.requests().is_empty());
    }

    #[test]
    fn the_app_waits_as_long_as_the_server_keeps_a_login() {
        assert_eq!(LIMIT, Duration::from_secs(10 * 60));
    }

    #[test]
    fn answers_without_the_login_challenge_are_ignored() {
        let fake =
            Fake::default().answer(200, r#"{"token":"tok","osu_id":7,"username":"Some Name"}"#);
        let mut server = Server::new(fake.clone(), "https://api.test");
        let pending = begin(&server).unwrap();
        let right = format!("/?code=abc&challenge={}", challenge_in(&pending.url));
        let browser = browse(
            port_of(&pending.url),
            &[
                "/?error=access_denied",
                "/?code=planted&challenge=0000",
                &right,
            ],
        );
        let session = wait(&mut server, pending, Duration::from_secs(10)).unwrap();
        assert_eq!(session.osu_id, 7);
        let replies = browser.join().unwrap();
        assert!(
            replies[0].starts_with("HTTP/1.1 404 Not Found\r\n"),
            "{}",
            replies[0]
        );
        assert!(
            replies[1].starts_with("HTTP/1.1 404 Not Found\r\n"),
            "{}",
            replies[1]
        );
        assert!(
            replies[2].starts_with("HTTP/1.1 302 Found\r\n"),
            "{}",
            replies[2]
        );
        assert_eq!(fake.requests().len(), 1);
    }

    #[test]
    fn a_silent_connection_does_not_keep_the_login_past_its_limit() {
        let mut server = Server::new(Fake::default(), "https://api.test");
        let pending = begin(&server).unwrap();
        let silent = TcpStream::connect(("127.0.0.1", port_of(&pending.url))).unwrap();
        let started = Instant::now();
        let error = wait(&mut server, pending, Duration::from_millis(1200)).unwrap_err();
        assert!(matches!(error, LoginError::TimedOut));
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        drop(silent);
    }

    #[test]
    fn a_code_the_server_refuses_ends_the_login() {
        let fake = Fake::default().answer(410, r#"{"error":"login_expired"}"#);
        let mut server = Server::new(fake, "https://api.test");
        let pending = begin(&server).unwrap();
        let answer = format!("/?code=old&challenge={}", challenge_in(&pending.url));
        let browser = browse(port_of(&pending.url), &[&answer]);
        let error = wait(&mut server, pending, Duration::from_secs(10)).unwrap_err();
        assert!(matches!(error, LoginError::Expired));
        assert!(
            browser.join().unwrap()[0]
                .contains(&format!("\r\nLocation: {SITE}?error=login_expired\r\n"))
        );
    }
}
