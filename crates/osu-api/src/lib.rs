use std::thread;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const TOKEN_URL: &str = "https://osu.ppy.sh/oauth/token";
const API_URL: &str = "https://osu.ppy.sh/api/v2";
const FILE_URL: &str = "https://osu.ppy.sh/osu";
const BATCH: usize = 50;
pub const SERVER_URL: &str = "https://api.lemon4ik.kz";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credentials {
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub retry_after: Option<u64>,
    pub body: Vec<u8>,
}

pub trait Transport {
    fn post_form(&mut self, url: &str, form: &[(&str, &str)]) -> Result<Response, ApiError>;
    fn get(&mut self, url: &str, token: &str) -> Result<Response, ApiError>;
    fn post_json(&mut self, url: &str, body: &[u8], token: &str) -> Result<Response, ApiError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserKey {
    Username,
    Id,
}

impl UserKey {
    fn as_str(self) -> &'static str {
        match self {
            UserKey::Username => "username",
            UserKey::Id => "id",
        }
    }
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("cannot reach the osu! API: {0}")]
    Network(String),
    #[error("the osu! API answered {status} for {url}")]
    Status { status: u16, url: String },
    #[error("unexpected answer from the osu! API for {url}")]
    Format {
        url: String,
        source: serde_json::Error,
    },
    #[error("cannot reach the Lost Scores server: {0}")]
    Server(String),
    #[error("the Lost Scores server is busy; try again in {retry_after} s")]
    Busy { retry_after: u64 },
    #[error("the Lost Scores server is busy today; add your own API keys to scan now")]
    BusyToday,
    #[error("log in with your osu! account first")]
    LoginRequired,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Beatmapset {
    pub status: String,
    pub ranked_date: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Beatmap {
    pub id: u32,
    pub checksum: Option<String>,
    pub status: String,
    pub beatmapset: Option<Beatmapset>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct User {
    pub id: u32,
    pub username: String,
    #[serde(default)]
    pub previous_usernames: Vec<String>,
    #[serde(default)]
    pub statistics: Option<UserStatistics>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct UserStatistics {
    pub pp: f64,
    pub hit_accuracy: f64,
    pub global_rank: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ScoreStatistics {
    pub count_300: u32,
    pub count_100: u32,
    pub count_50: u32,
    pub count_miss: u32,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ScoreBeatmap {
    pub id: u32,
    pub version: String,
    pub checksum: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ScoreBeatmapset {
    pub id: u32,
    pub artist: String,
    pub title: String,
    pub creator: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Score {
    pub id: u64,
    pub pp: Option<f64>,
    pub accuracy: f64,
    pub mods: Vec<String>,
    pub score: u64,
    pub rank: String,
    pub created_at: String,
    pub statistics: ScoreStatistics,
    pub beatmap: ScoreBeatmap,
    pub beatmapset: ScoreBeatmapset,
}

pub const BEST_PAGE: u32 = 100;

pub struct Client<T> {
    transport: T,
    credentials: Credentials,
    token: Option<String>,
    pause: Duration,
    last_request: Option<Instant>,
}

impl<T: Transport> Client<T> {
    pub fn new(transport: T, credentials: Credentials) -> Self {
        Self {
            transport,
            credentials,
            token: None,
            pause: Duration::from_secs(1),
            last_request: None,
        }
    }

    pub fn with_pause(mut self, pause: Duration) -> Self {
        self.pause = pause;
        self
    }

    pub fn check(&mut self) -> Result<(), ApiError> {
        self.renew_token()
    }

    pub fn beatmaps(&mut self, ids: &[u32]) -> Result<Vec<Beatmap>, ApiError> {
        let mut found = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(BATCH) {
            let query: Vec<String> = chunk.iter().map(|id| format!("ids[]={id}")).collect();
            let url = format!("{API_URL}/beatmaps?{}", query.join("&"));
            if let Some(page) = self.get_json::<Beatmaps>(&url)? {
                found.extend(page.beatmaps);
            }
        }
        Ok(found)
    }

    pub fn lookup(&mut self, checksum: &str) -> Result<Option<Beatmap>, ApiError> {
        self.get_json(&format!(
            "{API_URL}/beatmaps/lookup?checksum={}",
            encode(checksum)
        ))
    }

    pub fn user(&mut self, name: &str) -> Result<User, ApiError> {
        self.user_by(UserKey::Username, name)
    }

    pub fn user_by(&mut self, key: UserKey, value: &str) -> Result<User, ApiError> {
        let url = format!("{API_URL}/users/{}/osu?key={}", encode(value), key.as_str());
        self.get_json(&url)?
            .ok_or(ApiError::Status { status: 404, url })
    }

    pub fn best_scores(&mut self, user_id: u32, offset: u32) -> Result<Vec<Score>, ApiError> {
        let url = format!(
            "{API_URL}/users/{user_id}/scores/best?mode=osu&limit={BEST_PAGE}&offset={offset}"
        );
        Ok(self.get_json(&url)?.unwrap_or_default())
    }

    pub fn top_plays(&mut self, user_id: u32) -> Result<Vec<Score>, ApiError> {
        top_pages(|offset| self.best_scores(user_id, offset))
    }

    fn renew_token(&mut self) -> Result<(), ApiError> {
        self.wait();
        let form = [
            ("client_id", self.credentials.client_id.as_str()),
            ("client_secret", self.credentials.client_secret.as_str()),
            ("grant_type", "client_credentials"),
            ("scope", "public"),
        ];
        let response = self.transport.post_form(TOKEN_URL, &form)?;
        if response.status != 200 {
            return Err(ApiError::Status {
                status: response.status,
                url: TOKEN_URL.into(),
            });
        }
        let token: Token = parse(TOKEN_URL, &response.body)?;
        self.token = Some(token.access_token);
        Ok(())
    }

    pub fn osu_file(&mut self, beatmap_id: u32) -> Result<Vec<u8>, ApiError> {
        Ok(self
            .get_bytes(&format!("{FILE_URL}/{beatmap_id}"))?
            .unwrap_or_default())
    }

    fn get_json<D: DeserializeOwned>(&mut self, url: &str) -> Result<Option<D>, ApiError> {
        match self.get_bytes(url)? {
            Some(body) => parse(url, &body).map(Some),
            None => Ok(None),
        }
    }

    fn get_bytes(&mut self, url: &str) -> Result<Option<Vec<u8>>, ApiError> {
        let mut renewed = false;
        let mut throttled = 0;
        loop {
            let token = match &self.token {
                Some(token) => token.clone(),
                None => {
                    self.renew_token()?;
                    self.token.clone().unwrap_or_default()
                }
            };
            self.wait();
            let response = self.transport.get(url, &token)?;
            match response.status {
                200 => return Ok(Some(response.body)),
                404 => return Ok(None),
                401 if !renewed => {
                    renewed = true;
                    self.token = None;
                }
                429 if throttled < 3 => {
                    throttled += 1;
                    thread::sleep(Duration::from_secs(response.retry_after.unwrap_or(60)));
                }
                status => {
                    return Err(ApiError::Status {
                        status,
                        url: url.into(),
                    });
                }
            }
        }
    }

    fn wait(&mut self) {
        if let Some(last) = self.last_request {
            let elapsed = last.elapsed();
            if elapsed < self.pause {
                thread::sleep(self.pause - elapsed);
            }
        }
        self.last_request = Some(Instant::now());
    }
}

fn top_pages(
    mut page: impl FnMut(u32) -> Result<Vec<Score>, ApiError>,
) -> Result<Vec<Score>, ApiError> {
    let mut plays = Vec::new();
    for offset in [0, BEST_PAGE] {
        let scores = page(offset)?;
        let short = scores.len() < BEST_PAGE as usize;
        plays.extend(scores);
        if short {
            break;
        }
    }
    Ok(plays)
}

#[derive(Debug, Default, Deserialize)]
pub struct MapsAnswer {
    pub beatmaps: Vec<Beatmap>,
    #[serde(default)]
    pub missing_ids: Vec<u32>,
    #[serde(default)]
    pub missing_md5s: Vec<String>,
    #[serde(default)]
    pub pending_ids: Vec<u32>,
    #[serde(default)]
    pub pending_md5s: Vec<String>,
    #[serde(default)]
    pub retry_after: Option<u64>,
}

#[derive(Serialize)]
struct MapsQuestion<'a> {
    ids: &'a [u32],
    md5s: &'a [String],
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Login {
    pub token: String,
    pub osu_id: u32,
    pub username: String,
}

#[derive(Serialize)]
struct Redeem<'a> {
    code: &'a str,
    verifier: &'a str,
}

#[derive(Deserialize)]
struct ServerError {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    retry_after: Option<u64>,
}

pub struct Server<T> {
    transport: T,
    base: String,
    session: String,
}

impl<T: Transport> Server<T> {
    pub fn new(transport: T, base: &str) -> Self {
        Self {
            transport,
            base: base.trim_end_matches('/').to_owned(),
            session: String::new(),
        }
    }

    pub fn with_session(mut self, token: &str) -> Self {
        self.session = token.to_owned();
        self
    }

    pub fn login_url(&self, challenge: &str, port: u16) -> String {
        format!(
            "{}/v1/auth/start?challenge={}&port={port}",
            self.base,
            encode(challenge)
        )
    }

    pub fn redeem(&mut self, code: &str, verifier: &str) -> Result<Option<Login>, ApiError> {
        let url = format!("{}/v1/auth/token", self.base);
        let body = to_json(&url, &Redeem { code, verifier })?;
        let response = self
            .transport
            .post_json(&url, &body, "")
            .map_err(unreached)?;
        match response.status {
            200 => parse(&url, &response.body).map(Some),
            410 => Ok(None),
            status => Err(ApiError::Status { status, url }),
        }
    }

    pub fn logout(&mut self) -> Result<(), ApiError> {
        if self.session.is_empty() {
            return Ok(());
        }
        let url = format!("{}/api/auth/logout", self.base);
        let response = self
            .transport
            .post_json(&url, b"", &self.session)
            .map_err(unreached)?;
        match response.status {
            200..=299 => Ok(()),
            status => Err(ApiError::Status { status, url }),
        }
    }

    pub fn maps(&mut self, ids: &[u32], md5s: &[String]) -> Result<MapsAnswer, ApiError> {
        let url = format!("{}/v1/maps", self.base);
        let body = to_json(&url, &MapsQuestion { ids, md5s })?;
        let response = self
            .transport
            .post_json(&url, &body, &self.session)
            .map_err(unreached)?;
        match answer(&url, response)? {
            Some(body) => parse(&url, &body),
            None => Err(ApiError::Status { status: 404, url }),
        }
    }

    pub fn user_by(&mut self, key: UserKey, value: &str) -> Result<User, ApiError> {
        let url = format!(
            "{}/v1/osu/users/{}/osu?key={}",
            self.base,
            encode(value),
            key.as_str()
        );
        self.get_json(&url)?
            .ok_or(ApiError::Status { status: 404, url })
    }

    pub fn best_scores(&mut self, user_id: u32, offset: u32) -> Result<Vec<Score>, ApiError> {
        let url = format!(
            "{}/v1/osu/users/{user_id}/scores/best?mode=osu&limit={BEST_PAGE}&offset={offset}",
            self.base
        );
        Ok(self.get_json(&url)?.unwrap_or_default())
    }

    pub fn top_plays(&mut self, user_id: u32) -> Result<Vec<Score>, ApiError> {
        top_pages(|offset| self.best_scores(user_id, offset))
    }

    pub fn osu_file(&mut self, beatmap_id: u32) -> Result<Vec<u8>, ApiError> {
        let url = format!("{FILE_URL}/{beatmap_id}");
        let response = self.transport.get(&url, "")?;
        match response.status {
            200 => Ok(response.body),
            404 => Ok(Vec::new()),
            status => Err(ApiError::Status { status, url }),
        }
    }

    fn get_json<D: DeserializeOwned>(&mut self, url: &str) -> Result<Option<D>, ApiError> {
        let response = self.transport.get(url, &self.session).map_err(unreached)?;
        match answer(url, response)? {
            Some(body) => parse(url, &body).map(Some),
            None => Ok(None),
        }
    }
}

fn to_json<S: Serialize>(url: &str, value: &S) -> Result<Vec<u8>, ApiError> {
    serde_json::to_vec(value).map_err(|source| ApiError::Format {
        url: url.into(),
        source,
    })
}

fn unreached(error: ApiError) -> ApiError {
    match error {
        ApiError::Network(reason) => ApiError::Server(reason),
        other => other,
    }
}

fn answer(url: &str, response: Response) -> Result<Option<Vec<u8>>, ApiError> {
    let told = |body: &[u8]| serde_json::from_slice::<ServerError>(body).ok();
    match response.status {
        200 => Ok(Some(response.body)),
        404 => Ok(None),
        429 => Err(ApiError::Busy {
            retry_after: response
                .retry_after
                .filter(|seconds| *seconds > 0)
                .or_else(|| told(&response.body).and_then(|error| error.retry_after))
                .unwrap_or(60),
        }),
        503 if told(&response.body)
            .and_then(|error| error.error)
            .as_deref()
            == Some("busy_today") =>
        {
            Err(ApiError::BusyToday)
        }
        401 if told(&response.body)
            .and_then(|error| error.error)
            .as_deref()
            == Some("login_required") =>
        {
            Err(ApiError::LoginRequired)
        }
        status => Err(ApiError::Status {
            status,
            url: url.into(),
        }),
    }
}

#[derive(Deserialize)]
struct Token {
    access_token: String,
}

#[derive(Deserialize)]
struct Beatmaps {
    beatmaps: Vec<Beatmap>,
}

fn parse<D: DeserializeOwned>(url: &str, body: &[u8]) -> Result<D, ApiError> {
    serde_json::from_slice(body).map_err(|source| ApiError::Format {
        url: url.into(),
        source,
    })
}

pub fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

pub struct Http {
    agent: ureq::Agent,
}

impl Http {
    pub fn new(user_agent: &str) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .user_agent(user_agent)
            .timeout_global(Some(Duration::from_secs(30)))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

impl Transport for Http {
    fn post_form(&mut self, url: &str, form: &[(&str, &str)]) -> Result<Response, ApiError> {
        let response = self
            .agent
            .post(url)
            .header("Accept", "application/json")
            .send_form(form.iter().copied())
            .map_err(network)?;
        read(response)
    }

    fn get(&mut self, url: &str, token: &str) -> Result<Response, ApiError> {
        let request = self.agent.get(url).header("Accept", "application/json");
        let request = if token.is_empty() {
            request
        } else {
            request.header("Authorization", format!("Bearer {token}"))
        };
        read(request.call().map_err(network)?)
    }

    fn post_json(&mut self, url: &str, body: &[u8], token: &str) -> Result<Response, ApiError> {
        let request = self
            .agent
            .post(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json");
        let request = if token.is_empty() {
            request
        } else {
            request.header("Authorization", format!("Bearer {token}"))
        };
        read(request.send(body).map_err(network)?)
    }
}

fn read(mut response: ureq::http::Response<ureq::Body>) -> Result<Response, ApiError> {
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok());
    let body = response.body_mut().read_to_vec().map_err(network)?;
    Ok(Response {
        status,
        retry_after,
        body,
    })
}

fn network(error: ureq::Error) -> ApiError {
    ApiError::Network(error.to_string())
}

pub fn unix_time(iso: &str) -> Option<i64> {
    let (date, rest) = iso.split_once('T')?;
    let mut parts = date.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next()??);
    let (clock, zone) = rest.split_at_checked(8)?;
    let mut parts = clock.split(':').map(|part| part.parse::<i64>().ok());
    let (hour, minute, second) = (parts.next()??, parts.next()??, parts.next()??);
    let zone = zone.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match zone {
        "" | "Z" => 0,
        _ => {
            let sign = match zone.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let (hours, minutes) = zone[1..].split_once(':')?;
            sign * (hours.parse::<i64>().ok()? * 3600 + minutes.parse::<i64>().ok()? * 60)
        }
    };
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::time::Duration;

    use super::{
        ApiError, Client, Credentials, Login, Response, Score, ScoreBeatmap, ScoreBeatmapset,
        ScoreStatistics, Server, Transport, User, UserKey, UserStatistics, unix_time,
    };

    #[derive(Default)]
    struct Fake {
        answers: VecDeque<Response>,
        requests: Vec<String>,
    }

    impl Fake {
        fn answer(mut self, status: u16, body: &str) -> Self {
            self.answers.push_back(Response {
                status,
                retry_after: (status == 429).then_some(0),
                body: body.as_bytes().to_vec(),
            });
            self
        }
    }

    impl Transport for Fake {
        fn post_form(&mut self, url: &str, form: &[(&str, &str)]) -> Result<Response, ApiError> {
            let fields: Vec<String> = form.iter().map(|(k, v)| format!("{k}={v}")).collect();
            self.requests
                .push(format!("POST {url} {}", fields.join("&")));
            Ok(self.answers.pop_front().expect("no scripted answer"))
        }

        fn get(&mut self, url: &str, token: &str) -> Result<Response, ApiError> {
            self.requests.push(format!("GET {url} bearer={token}"));
            Ok(self.answers.pop_front().expect("no scripted answer"))
        }

        fn post_json(&mut self, url: &str, body: &[u8], token: &str) -> Result<Response, ApiError> {
            self.requests.push(format!(
                "POST {url} bearer={token} {}",
                String::from_utf8_lossy(body)
            ));
            Ok(self.answers.pop_front().expect("no scripted answer"))
        }
    }

    fn server(fake: Fake) -> Server<Fake> {
        Server::new(fake, "https://api.test/")
    }

    struct Down;

    impl Transport for Down {
        fn post_form(&mut self, _: &str, _: &[(&str, &str)]) -> Result<Response, ApiError> {
            Err(ApiError::Network("refused".into()))
        }

        fn get(&mut self, _: &str, _: &str) -> Result<Response, ApiError> {
            Err(ApiError::Network("refused".into()))
        }

        fn post_json(&mut self, _: &str, _: &[u8], _: &str) -> Result<Response, ApiError> {
            Err(ApiError::Network("refused".into()))
        }
    }

    #[test]
    fn a_server_out_of_reach_is_named_and_osu_files_still_blame_osu() {
        let mut api = Server::new(Down, "https://api.test");
        let maps = api.maps(&[1], &[]).unwrap_err();
        assert_eq!(
            maps.to_string(),
            "cannot reach the Lost Scores server: refused"
        );
        let user = api.user_by(UserKey::Id, "7").unwrap_err();
        assert!(matches!(user, ApiError::Server(_)));
        let file = api.osu_file(1).unwrap_err();
        assert_eq!(file.to_string(), "cannot reach the osu! API: refused");
    }

    #[test]
    fn the_server_answers_known_missing_and_pending_maps() {
        let fake = Fake::default().answer(
            200,
            &format!(
                r#"{{"beatmaps":[{}],"missing_ids":[2],"missing_md5s":[],"pending_ids":[3],"pending_md5s":["abc"],"retry_after":5}}"#,
                beatmap_json(1)
            ),
        );
        let mut api = server(fake);
        let answer = api.maps(&[1, 2, 3], &["abc".to_owned()]).unwrap();
        assert_eq!(
            answer.beatmaps.iter().map(|map| map.id).collect::<Vec<_>>(),
            [1]
        );
        assert_eq!(answer.missing_ids, [2]);
        assert_eq!(answer.pending_ids, [3]);
        assert_eq!(answer.pending_md5s, ["abc"]);
        assert_eq!(answer.retry_after, Some(5));
        assert_eq!(
            api.transport.requests[0],
            r#"POST https://api.test/v1/maps bearer= {"ids":[1,2,3],"md5s":["abc"]}"#
        );
    }

    #[test]
    fn a_busy_server_says_how_long_to_wait() {
        let mut fake = Fake::default();
        fake.answers.push_back(Response {
            status: 429,
            retry_after: Some(7),
            body: br#"{"retry_after":7}"#.to_vec(),
        });
        let mut api = server(fake.answer(503, r#"{"error":"busy_today"}"#));
        assert!(matches!(
            api.user_by(UserKey::Id, "7"),
            Err(ApiError::Busy { retry_after: 7 })
        ));
        let today = api.maps(&[1], &[]).unwrap_err();
        assert!(matches!(today, ApiError::BusyToday));
        assert_eq!(
            today.to_string(),
            "the Lost Scores server is busy today; add your own API keys to scan now"
        );
    }

    #[test]
    fn player_data_comes_from_the_server_without_a_token() {
        let fake = Fake::default()
            .answer(
                200,
                r#"{"id":7,"username":"Some Name","previous_usernames":[],"statistics":null}"#,
            )
            .answer(200, "[]");
        let mut api = server(fake);
        assert_eq!(api.user_by(UserKey::Username, "Some Name").unwrap().id, 7);
        assert!(api.best_scores(7, 100).unwrap().is_empty());
        assert_eq!(
            api.transport.requests,
            [
                "GET https://api.test/v1/osu/users/Some%20Name/osu?key=username bearer=",
                "GET https://api.test/v1/osu/users/7/scores/best?mode=osu&limit=100&offset=100 bearer=",
            ]
        );
    }

    #[test]
    fn osu_files_come_from_osu_without_a_token() {
        let fake = Fake::default().answer(200, "osu file format v14");
        let mut api = server(fake);
        assert_eq!(api.osu_file(5).unwrap(), b"osu file format v14");
        assert_eq!(
            api.transport.requests[0],
            "GET https://osu.ppy.sh/osu/5 bearer="
        );
    }

    #[test]
    fn the_session_goes_to_the_server_and_never_to_osu_files() {
        let fake = Fake::default()
            .answer(200, r#"{"beatmaps":[]}"#)
            .answer(
                200,
                r#"{"id":7,"username":"Some Name","previous_usernames":[],"statistics":null}"#,
            )
            .answer(200, "[]")
            .answer(200, "osu file format v14");
        let mut api = server(fake).with_session("s3cret");
        api.maps(&[1], &[]).unwrap();
        api.user_by(UserKey::Id, "7").unwrap();
        api.best_scores(7, 0).unwrap();
        api.osu_file(5).unwrap();
        assert_eq!(
            api.transport.requests,
            [
                r#"POST https://api.test/v1/maps bearer=s3cret {"ids":[1],"md5s":[]}"#,
                "GET https://api.test/v1/osu/users/7/osu?key=id bearer=s3cret",
                "GET https://api.test/v1/osu/users/7/scores/best?mode=osu&limit=100&offset=0 bearer=s3cret",
                "GET https://osu.ppy.sh/osu/5 bearer=",
            ]
        );
    }

    #[test]
    fn a_redeem_reads_the_session_or_expired() {
        let fake = Fake::default()
            .answer(200, r#"{"token":"tok","osu_id":7,"username":"Some Name"}"#)
            .answer(410, r#"{"error":"login_expired"}"#)
            .answer(400, r#"{"error":"bad_request"}"#);
        let mut api = server(fake);
        assert_eq!(
            api.redeem("code1", "verifier1").unwrap(),
            Some(Login {
                token: "tok".into(),
                osu_id: 7,
                username: "Some Name".into(),
            })
        );
        assert_eq!(api.redeem("code2", "verifier2").unwrap(), None);
        assert!(matches!(
            api.redeem("code3", "verifier3"),
            Err(ApiError::Status { status: 400, .. })
        ));
        assert_eq!(
            api.transport.requests[0],
            r#"POST https://api.test/v1/auth/token bearer= {"code":"code1","verifier":"verifier1"}"#
        );
    }

    #[test]
    fn a_401_from_the_server_asks_to_log_in() {
        let required = r#"{"error":"login_required"}"#;
        let mut api = server(Fake::default().answer(401, required).answer(401, required));
        let maps = api.maps(&[1], &[]).unwrap_err();
        assert!(matches!(maps, ApiError::LoginRequired));
        assert_eq!(maps.to_string(), "log in with your osu! account first");
        assert!(matches!(
            api.user_by(UserKey::Id, "7"),
            Err(ApiError::LoginRequired)
        ));
    }

    #[test]
    fn the_login_url_carries_the_challenge_and_the_port() {
        assert_eq!(
            server(Fake::default()).login_url("ab12", 53682),
            "https://api.test/v1/auth/start?challenge=ab12&port=53682"
        );
    }

    #[test]
    fn logging_out_revokes_the_session_on_the_server() {
        let mut api = server(Fake::default().answer(204, "")).with_session("tok");
        api.logout().unwrap();
        assert_eq!(
            api.transport.requests,
            ["POST https://api.test/api/auth/logout bearer=tok "]
        );
        server(Fake::default()).logout().unwrap();
    }

    const TOKEN: &str = r#"{"token_type":"Bearer","expires_in":86400,"access_token":"t1"}"#;
    const TOKEN_2: &str = r#"{"token_type":"Bearer","expires_in":86400,"access_token":"t2"}"#;

    fn beatmap_json(id: u32) -> String {
        format!(
            r#"{{"id":{id},"checksum":"md5-{id}","status":"ranked","beatmapset":{{"status":"ranked","ranked_date":"2014-08-05T13:02:24Z"}},"failtimes":{{"fail":[0]}}}}"#
        )
    }

    fn client(fake: Fake) -> Client<Fake> {
        Client::new(
            fake,
            Credentials {
                client_id: "123".into(),
                client_secret: "secret".into(),
            },
        )
        .with_pause(Duration::ZERO)
    }

    #[test]
    fn token_is_requested_once_and_sent_as_bearer() {
        let fake = Fake::default()
            .answer(200, TOKEN)
            .answer(200, &format!(r#"{{"beatmaps":[{}]}}"#, beatmap_json(1)))
            .answer(404, r#"{"error":null}"#);
        let mut api = client(fake);
        assert_eq!(api.beatmaps(&[1]).unwrap()[0].id, 1);
        assert_eq!(api.lookup("abc").unwrap(), None);
        assert_eq!(
            api.transport.requests,
            [
                "POST https://osu.ppy.sh/oauth/token client_id=123&client_secret=secret&grant_type=client_credentials&scope=public",
                "GET https://osu.ppy.sh/api/v2/beatmaps?ids[]=1 bearer=t1",
                "GET https://osu.ppy.sh/api/v2/beatmaps/lookup?checksum=abc bearer=t1",
            ]
        );
    }

    #[test]
    fn beatmaps_are_asked_for_in_batches_of_fifty() {
        let ids: Vec<u32> = (1..=120).collect();
        let mut fake = Fake::default().answer(200, TOKEN);
        for chunk in ids.chunks(50) {
            let maps: Vec<String> = chunk.iter().map(|&id| beatmap_json(id)).collect();
            fake = fake.answer(200, &format!(r#"{{"beatmaps":[{}]}}"#, maps.join(",")));
        }
        let mut api = client(fake);
        let maps = api.beatmaps(&ids).unwrap();
        assert_eq!(maps.len(), 120);
        let per_request: Vec<usize> = api.transport.requests[1..]
            .iter()
            .map(|r| r.matches("ids[]=").count())
            .collect();
        assert_eq!(per_request, [50, 50, 20]);
    }

    #[test]
    fn expired_token_is_renewed_once() {
        let fake = Fake::default()
            .answer(200, TOKEN)
            .answer(401, "{}")
            .answer(200, TOKEN_2)
            .answer(200, &format!(r#"{{"beatmaps":[{}]}}"#, beatmap_json(7)));
        let mut api = client(fake);
        assert_eq!(api.beatmaps(&[7]).unwrap()[0].id, 7);
        assert!(api.transport.requests[3].ends_with("bearer=t2"));
    }

    #[test]
    fn rate_limit_answer_is_waited_out() {
        let fake = Fake::default()
            .answer(200, TOKEN)
            .answer(429, "{}")
            .answer(200, &format!(r#"{{"beatmaps":[{}]}}"#, beatmap_json(3)));
        assert_eq!(client(fake).beatmaps(&[3]).unwrap()[0].id, 3);
    }

    #[test]
    fn server_errors_are_reported() {
        let fake = Fake::default().answer(200, TOKEN).answer(500, "oops");
        assert!(matches!(
            client(fake).beatmaps(&[1]),
            Err(ApiError::Status { status: 500, .. })
        ));
    }

    #[test]
    fn user_is_looked_up_by_name_with_previous_names() {
        let fake = Fake::default().answer(200, TOKEN).answer(
            200,
            r#"{"id":42,"username":"New Name","previous_usernames":["Old"],"country_code":"KZ"}"#,
        );
        let mut api = client(fake);
        assert_eq!(
            api.user("New Name").unwrap(),
            User {
                id: 42,
                username: "New Name".into(),
                previous_usernames: vec!["Old".into()],
                statistics: None,
            }
        );
        assert_eq!(
            api.transport.requests[1],
            "GET https://osu.ppy.sh/api/v2/users/New%20Name/osu?key=username bearer=t1"
        );
    }

    #[test]
    fn an_id_is_looked_up_by_id() {
        let fake = Fake::default()
            .answer(200, TOKEN)
            .answer(200, r#"{"id":12345,"username":"Real Name"}"#);
        let mut api = client(fake);
        let user = api.user_by(UserKey::Id, "12345").unwrap();
        assert_eq!(user.username, "Real Name");
        assert_eq!(
            api.transport.requests[1],
            "GET https://osu.ppy.sh/api/v2/users/12345/osu?key=id bearer=t1"
        );
    }

    #[test]
    fn iso_timestamps_become_unix_seconds() {
        for (iso, expected) in [
            ("2014-08-05T13:02:24Z", Some(1_407_243_744)),
            ("2024-10-09T10:47:02+00:00", Some(1_728_470_822)),
            ("2000-02-29T00:00:00Z", Some(951_782_400)),
            ("1969-12-31T23:59:59Z", Some(-1)),
            ("not a date", None),
        ] {
            assert_eq!(unix_time(iso), expected, "{iso}");
        }
    }

    fn score_json(id: u64) -> String {
        format!(
            r#"{{"id":{id},"pp":412.3,"accuracy":0.9865,"mods":["HD","DT"],"score":52083331,"rank":"A","created_at":"2024-09-03T23:39:00Z","statistics":{{"count_300":900,"count_100":20,"count_50":1,"count_miss":2,"count_geki":0,"count_katu":0}},"beatmap":{{"id":4633040,"version":"Extra","checksum":"abc","status":"ranked"}},"beatmapset":{{"id":2190501,"artist":"Drop","title":"Granat","creator":"Sotarks","status":"ranked"}},"weight":{{"percentage":100,"pp":412.3}}}}"#
        )
    }

    #[test]
    fn best_scores_asks_one_page_of_100() {
        let fake = Fake::default()
            .answer(200, TOKEN)
            .answer(200, &format!("[{}]", score_json(4680387838)));
        let mut api = client(fake);
        let scores = api.best_scores(7, 100).unwrap();
        assert_eq!(
            api.transport.requests[1],
            "GET https://osu.ppy.sh/api/v2/users/7/scores/best?mode=osu&limit=100&offset=100 bearer=t1"
        );
        assert_eq!(
            scores,
            vec![Score {
                id: 4680387838,
                pp: Some(412.3),
                accuracy: 0.9865,
                mods: vec!["HD".into(), "DT".into()],
                score: 52083331,
                rank: "A".into(),
                created_at: "2024-09-03T23:39:00Z".into(),
                statistics: ScoreStatistics {
                    count_300: 900,
                    count_100: 20,
                    count_50: 1,
                    count_miss: 2,
                },
                beatmap: ScoreBeatmap {
                    id: 4633040,
                    version: "Extra".into(),
                    checksum: Some("abc".into()),
                },
                beatmapset: ScoreBeatmapset {
                    id: 2190501,
                    artist: "Drop".into(),
                    title: "Granat".into(),
                    creator: "Sotarks".into(),
                },
            }]
        );
    }

    #[test]
    fn a_player_without_scores_gets_an_empty_page() {
        let fake = Fake::default().answer(200, TOKEN).answer(200, "[]");
        assert_eq!(client(fake).best_scores(7, 0).unwrap(), vec![]);
    }

    #[test]
    fn top_plays_stops_after_a_short_page() {
        let fake = Fake::default()
            .answer(200, TOKEN)
            .answer(200, &format!("[{}]", score_json(1)));
        let mut api = client(fake);
        assert_eq!(api.top_plays(7).unwrap().len(), 1);
        assert_eq!(api.transport.requests.len(), 2);
    }

    #[test]
    fn top_plays_asks_again_after_a_full_page_and_accepts_an_empty_one() {
        let page = vec![score_json(1); super::BEST_PAGE as usize].join(",");
        let fake = Fake::default()
            .answer(200, TOKEN)
            .answer(200, &format!("[{page}]"))
            .answer(200, "[]");
        let mut api = client(fake);
        assert_eq!(api.top_plays(7).unwrap().len(), super::BEST_PAGE as usize);
        assert_eq!(api.transport.requests.len(), 3);
        assert!(api.transport.requests[2].contains("offset=100"));
    }

    #[test]
    fn user_carries_pp_accuracy_and_rank() {
        let fake = Fake::default().answer(200, TOKEN).answer(
            200,
            r#"{"id":42,"username":"Name","statistics":{"pp":8557.1,"hit_accuracy":98.3758,"global_rank":14856,"play_count":1}}"#,
        );
        assert_eq!(
            client(fake).user("Name").unwrap().statistics,
            Some(UserStatistics {
                pp: 8557.1,
                hit_accuracy: 98.3758,
                global_rank: Some(14856),
            })
        );
    }

    #[test]
    fn osu_file_returns_the_raw_bytes() {
        let fake = Fake::default()
            .answer(200, TOKEN)
            .answer(200, "osu file format v14");
        let mut api = client(fake);
        assert_eq!(api.osu_file(5).unwrap(), b"osu file format v14");
        assert_eq!(
            api.transport.requests[1],
            "GET https://osu.ppy.sh/osu/5 bearer=t1"
        );
    }
}
