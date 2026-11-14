use std::thread;
use std::time::{Duration, Instant};

use lost_scores_core::maps::{FetchedMap, MapSource};
use lost_scores_core::missing::OsuFiles;
use osu_api::{ApiError, Beatmap, Http, SERVER_URL, Score, Server, Transport, UserKey};

use crate::online::{Player, USER_AGENT, fetched, player_from};
use crate::pipeline::Api;

const MAX_IDS: usize = 500;
const MAX_MD5S: usize = 200;
const MIN_WAIT_S: u64 = 2;
const WAIT_CAP: Duration = Duration::from_secs(30 * 60);
const FILE_PAUSE: Duration = Duration::from_secs(1);

pub fn server_url() -> String {
    std::env::var("LOST_SCORES_SERVER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| SERVER_URL.to_owned())
}

pub struct Shared<T = Http> {
    server: Server<T>,
    last_file: Option<Instant>,
    sleep: Box<dyn FnMut(Duration) + Send>,
    notice: Box<dyn FnMut(u64) + Send>,
}

impl Shared<Http> {
    pub fn new() -> Self {
        Self::at(&server_url())
    }

    pub fn at(base: &str) -> Self {
        Self::with_transport(Http::new(USER_AGENT), base)
    }
}

impl Default for Shared<Http> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Transport> Shared<T> {
    pub fn with_transport(transport: T, base: &str) -> Self {
        Self {
            server: Server::new(transport, base),
            last_file: None,
            sleep: Box::new(thread::sleep),
            notice: Box::new(|_| {}),
        }
    }

    pub fn on_wait(mut self, notice: impl FnMut(u64) + Send + 'static) -> Self {
        self.notice = Box::new(notice);
        self
    }

    pub fn account(&mut self, key: UserKey, value: &str) -> Result<Player, ApiError> {
        let user = self.patiently(|server| server.user_by(key, value))?;
        let name = user.username.clone();
        Ok(player_from(&name, user))
    }

    fn patiently<R>(
        &mut self,
        mut call: impl FnMut(&mut Server<T>) -> Result<R, ApiError>,
    ) -> Result<R, ApiError> {
        let mut idle = Duration::ZERO;
        loop {
            match call(&mut self.server) {
                Err(ApiError::Busy { retry_after }) => self.wait(&mut idle, retry_after)?,
                result => return result,
            }
        }
    }

    fn wait(&mut self, idle: &mut Duration, seconds: u64) -> Result<(), ApiError> {
        let seconds = seconds.max(MIN_WAIT_S);
        let pause = Duration::from_secs(seconds);
        if *idle + pause > WAIT_CAP {
            return Err(ApiError::Busy {
                retry_after: seconds,
            });
        }
        (self.notice)(seconds);
        (self.sleep)(pause);
        *idle += pause;
        Ok(())
    }

    fn resolve(&mut self, ids: &[u32], md5s: &[String]) -> Result<Vec<Beatmap>, ApiError> {
        let mut found = Vec::new();
        let mut ids = ids.to_vec();
        let mut md5s = md5s.to_vec();
        let mut idle = Duration::ZERO;
        loop {
            let asked = ids.len() + md5s.len();
            let mut pending_ids = Vec::new();
            let mut pending_md5s = Vec::new();
            let mut wait = 0;
            for chunk in ids.chunks(MAX_IDS) {
                let answer = self.patiently(|server| server.maps(chunk, &[]))?;
                found.extend(answer.beatmaps);
                pending_ids.extend(answer.pending_ids);
                wait = wait.max(answer.retry_after.unwrap_or(0));
            }
            for chunk in md5s.chunks(MAX_MD5S) {
                let answer = self.patiently(|server| server.maps(&[], chunk))?;
                found.extend(answer.beatmaps);
                pending_md5s.extend(answer.pending_md5s);
                wait = wait.max(answer.retry_after.unwrap_or(0));
            }
            let pending = pending_ids.len() + pending_md5s.len();
            if pending == 0 {
                return Ok(found);
            }
            if pending < asked {
                idle = Duration::ZERO;
            }
            self.wait(&mut idle, wait)?;
            ids = pending_ids;
            md5s = pending_md5s;
        }
    }

    fn pace_files(&mut self) {
        if let Some(last) = self.last_file {
            let elapsed = last.elapsed();
            if elapsed < FILE_PAUSE {
                (self.sleep)(FILE_PAUSE - elapsed);
            }
        }
        self.last_file = Some(Instant::now());
    }
}

impl<T: Transport> Api for Shared<T> {
    fn player(&mut self, name: &str) -> Result<Player, ApiError> {
        let user = self.patiently(|server| server.user_by(UserKey::Username, name))?;
        Ok(player_from(name, user))
    }

    fn top(&mut self, user_id: u32) -> Result<Vec<Score>, ApiError> {
        self.patiently(|server| server.top_plays(user_id))
    }

    fn shared(&self) -> bool {
        true
    }
}

impl<T: Transport> MapSource for Shared<T> {
    type Error = ApiError;

    fn batch(&mut self, ids: &[u32]) -> Result<Vec<FetchedMap>, ApiError> {
        Ok(self.resolve(ids, &[])?.into_iter().map(fetched).collect())
    }

    fn lookup(&mut self, md5: &str) -> Result<Option<FetchedMap>, ApiError> {
        Ok(self
            .resolve(&[], &[md5.to_owned()])?
            .into_iter()
            .find(|map| map.checksum.as_deref() == Some(md5))
            .map(fetched))
    }

    fn batch_size(&self) -> usize {
        MAX_IDS
    }
}

impl<T: Transport> OsuFiles for Shared<T> {
    type Error = ApiError;

    fn download(&mut self, beatmap_id: u32) -> Result<Vec<u8>, ApiError> {
        self.pace_files();
        self.server.osu_file(beatmap_id)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use lost_scores_core::maps::{MapSource, MapStatus};
    use lost_scores_core::missing::OsuFiles;
    use osu_api::{ApiError, Response, Transport};

    use super::{Shared, WAIT_CAP};
    use crate::pipeline::Api;

    type Log<T> = Arc<Mutex<Vec<T>>>;

    const USER: &str =
        r#"{"id":7,"username":"Synthetic","previous_usernames":[],"statistics":null}"#;

    #[derive(Clone, Default)]
    struct Fake {
        answers: Arc<Mutex<VecDeque<(u16, String)>>>,
        requests: Log<String>,
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

        fn next(&self) -> Result<Response, ApiError> {
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

    impl Transport for Fake {
        fn post_form(&mut self, url: &str, _: &[(&str, &str)]) -> Result<Response, ApiError> {
            self.requests.lock().unwrap().push(format!("FORM {url}"));
            self.next()
        }

        fn get(&mut self, url: &str, token: &str) -> Result<Response, ApiError> {
            self.requests
                .lock()
                .unwrap()
                .push(format!("GET {url} bearer={token}"));
            self.next()
        }

        fn post_json(&mut self, url: &str, body: &[u8]) -> Result<Response, ApiError> {
            self.requests
                .lock()
                .unwrap()
                .push(format!("POST {url} {}", String::from_utf8_lossy(body)));
            self.next()
        }
    }

    fn map(id: u32) -> String {
        format!(
            r#"{{"id":{id},"checksum":"{id:032x}","status":"ranked","beatmapset":{{"status":"ranked","ranked_date":"2015-01-01T00:00:00Z"}}}}"#
        )
    }

    fn answer(found: &[u32], pending: &[u32], retry_after: Option<u64>) -> String {
        let maps: Vec<String> = found.iter().map(|id| map(*id)).collect();
        format!(
            r#"{{"beatmaps":[{}],"missing_ids":[],"missing_md5s":[],"pending_ids":{:?},"pending_md5s":[],"retry_after":{}}}"#,
            maps.join(","),
            pending,
            retry_after.map_or("null".to_owned(), |seconds| seconds.to_string())
        )
    }

    fn shared(fake: &Fake) -> (Shared<Fake>, Log<u64>, Log<Duration>) {
        let notices = Arc::new(Mutex::new(Vec::new()));
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let mut api = Shared::with_transport(fake.clone(), "https://api.test");
        let seen = notices.clone();
        api = api.on_wait(move |seconds| seen.lock().unwrap().push(seconds));
        let slept = sleeps.clone();
        api.sleep = Box::new(move |duration| slept.lock().unwrap().push(duration));
        (api, notices, sleeps)
    }

    #[test]
    fn pending_maps_are_asked_again_after_the_wait_and_only_they() {
        let fake = Fake::default()
            .answer(200, &answer(&[1], &[2], Some(9)))
            .answer(200, &answer(&[2], &[], None));
        let (mut api, notices, sleeps) = shared(&fake);
        let maps = api.batch(&[1, 2]).unwrap();
        let ids: Vec<u32> = maps.iter().filter_map(|map| map.info.beatmap_id).collect();
        assert_eq!(ids, [1, 2]);
        assert_eq!(maps[0].info.status, MapStatus::Ranked);
        assert_eq!(*notices.lock().unwrap(), [9]);
        assert_eq!(*sleeps.lock().unwrap(), [Duration::from_secs(9)]);
        assert_eq!(
            fake.requests(),
            [
                r#"POST https://api.test/v1/maps {"ids":[1,2],"md5s":[]}"#,
                r#"POST https://api.test/v1/maps {"ids":[2],"md5s":[]}"#,
            ]
        );
    }

    #[test]
    fn a_wait_past_half_an_hour_ends_with_a_busy_error() {
        let mut fake = Fake::default();
        for _ in 0..40 {
            fake = fake.answer(200, &answer(&[], &[5], Some(60)));
        }
        let (mut api, _, sleeps) = shared(&fake);
        let error = api.batch(&[5]).unwrap_err();
        assert!(matches!(error, ApiError::Busy { .. }));
        let waited: Duration = sleeps.lock().unwrap().iter().sum();
        assert!(waited <= Duration::from_secs(30 * 60));
    }

    #[test]
    fn waits_that_keep_bringing_maps_have_no_limit() {
        let fake = Fake::default()
            .answer(200, &answer(&[], &[1, 2], Some(1000)))
            .answer(200, &answer(&[1], &[2], Some(1000)))
            .answer(200, &answer(&[2], &[], None));
        let (mut api, _, sleeps) = shared(&fake);
        assert_eq!(api.batch(&[1, 2]).unwrap().len(), 2);
        assert_eq!(sleeps.lock().unwrap().len(), 2);
    }

    #[test]
    fn every_request_gets_its_own_wait_limit() {
        let fake = Fake::default()
            .answer(200, &answer(&[], &[1], Some(1000)))
            .answer(200, &answer(&[1], &[], None))
            .answer(200, &answer(&[], &[2], Some(1000)))
            .answer(200, &answer(&[2], &[], None));
        let (mut api, _, _) = shared(&fake);
        assert_eq!(api.batch(&[1]).unwrap().len(), 1);
        assert_eq!(api.batch(&[2]).unwrap().len(), 1);
    }

    #[test]
    fn a_busy_player_call_waits_and_asks_again() {
        let fake = Fake::default()
            .answer(429, r#"{"retry_after":5}"#)
            .answer(200, USER);
        let (mut api, notices, sleeps) = shared(&fake);
        assert_eq!(api.player("Synthetic").unwrap().id, 7);
        assert_eq!(*notices.lock().unwrap(), [5]);
        assert_eq!(*sleeps.lock().unwrap(), [Duration::from_secs(5)]);
    }

    #[test]
    fn a_player_call_busy_for_half_an_hour_gives_up() {
        let mut fake = Fake::default();
        for _ in 0..40 {
            fake = fake.answer(429, r#"{"retry_after":60}"#);
        }
        let (mut api, _, sleeps) = shared(&fake);
        let error = api.player("Synthetic").unwrap_err();
        assert!(matches!(error, ApiError::Busy { .. }));
        let waited: Duration = sleeps.lock().unwrap().iter().sum();
        assert!(waited > WAIT_CAP - Duration::from_secs(60) && waited <= WAIT_CAP);
    }

    #[test]
    fn osu_files_come_at_most_one_a_second() {
        let fake = Fake::default().answer(200, "a").answer(200, "b");
        let (mut api, notices, sleeps) = shared(&fake);
        api.download(1).unwrap();
        api.download(2).unwrap();
        let sleeps = sleeps.lock().unwrap();
        assert_eq!(sleeps.len(), 1);
        assert!(sleeps[0] > Duration::from_millis(900));
        assert!(notices.lock().unwrap().is_empty());
    }

    #[test]
    fn the_server_takes_500_maps_a_request() {
        let (api, _, _) = shared(&Fake::default());
        assert_eq!(api.batch_size(), 500);
    }

    #[test]
    fn a_busy_day_is_reported_with_its_text() {
        let fake = Fake::default().answer(503, r#"{"error":"busy_today"}"#);
        let (mut api, _, _) = shared(&fake);
        let error = api.player("Synthetic").unwrap_err();
        assert_eq!(
            error.to_string(),
            "the Lost Scores server is busy today; add your own API keys to scan now"
        );
    }

    #[test]
    fn a_lookup_by_md5_waits_like_a_batch() {
        let fake = Fake::default()
            .answer(
                200,
                r#"{"beatmaps":[],"missing_ids":[],"missing_md5s":[],"pending_ids":[],"pending_md5s":["0000000000000000000000000000000b"],"retry_after":3}"#,
            )
            .answer(200, &answer(&[11], &[], None));
        let (mut api, _, _) = shared(&fake);
        let found = api.lookup("0000000000000000000000000000000b").unwrap();
        assert_eq!(found.and_then(|map| map.info.beatmap_id), Some(11));
    }

    #[test]
    fn player_data_and_files_need_no_keys() {
        let fake = Fake::default()
            .answer(200, USER)
            .answer(200, "[]")
            .answer(200, "osu file format v14");
        let (mut api, _, _) = shared(&fake);
        assert_eq!(api.player("Synthetic").unwrap().id, 7);
        assert!(api.top(7).unwrap().is_empty());
        assert_eq!(api.download(5).unwrap(), b"osu file format v14");
        let requests = fake.requests();
        assert!(requests.iter().all(|request| request.ends_with("bearer=")));
        assert_eq!(requests[2], "GET https://osu.ppy.sh/osu/5 bearer=");
    }
}
