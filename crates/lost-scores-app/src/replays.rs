use std::fs;
use std::path::Path;

use osu_api::{ApiError, MAX_REPLAY, Server, Transport};
use osu_formats::ReplayHeader;

use crate::history::{SavedScan, Sent};
use crate::rows::printable;

#[derive(Debug)]
pub enum Outcome {
    Sent { row: usize },
    Unreadable { row: usize, reason: String },
    Refused { row: usize, error: ApiError },
    Waiting { error: ApiError },
}

fn waits(error: &ApiError) -> bool {
    matches!(
        error,
        ApiError::ReplaysPaused { .. }
            | ApiError::ReplayQuota
            | ApiError::Network(_)
            | ApiError::Server(_)
            | ApiError::Busy { .. }
            | ApiError::BusyToday
            | ApiError::LoginRequired
            | ApiError::ServerStatus { .. }
    )
}

fn readable(path: &Path) -> Result<Vec<u8>, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() > MAX_REPLAY {
        return Err(format!(
            "{} is larger than the 4 MB a replay may have",
            path.display()
        ));
    }
    ReplayHeader::check(&bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(bytes)
}

pub fn send_queued<T: Transport>(
    server: &mut Server<T>,
    sent: &mut Sent,
    scan: &SavedScan,
    replays_dir: &Path,
) -> Vec<Outcome> {
    let mut outcomes = Vec::new();
    for row in sent.queued.clone() {
        let Some(lost) = scan.lost.get(row) else {
            sent.queued.remove(&row);
            continue;
        };
        let bytes = match readable(&replays_dir.join(&lost.file)) {
            Ok(bytes) => bytes,
            Err(reason) => {
                sent.queued.remove(&row);
                outcomes.push(Outcome::Unreadable { row, reason });
                continue;
            }
        };
        match server.upload_replay(&sent.id, row, &bytes) {
            Ok(sha256) => {
                sent.queued.remove(&row);
                sent.replays.insert(row, sha256);
                outcomes.push(Outcome::Sent { row });
            }
            Err(error) if waits(&error) => {
                outcomes.push(Outcome::Waiting { error });
                break;
            }
            Err(ApiError::NotOnServer) => {
                sent.queued.clear();
                outcomes.push(Outcome::Refused {
                    row,
                    error: ApiError::NotOnServer,
                });
                break;
            }
            Err(error) => {
                sent.queued.remove(&row);
                outcomes.push(Outcome::Refused { row, error });
            }
        }
    }
    outcomes
}

pub fn describe(outcome: &Outcome, scan: &SavedScan) -> String {
    let play = |row: usize| {
        scan.lost.get(row).map_or_else(
            || format!("play {row}"),
            |lost| {
                format!(
                    "{} - {} [{}]",
                    printable(&lost.artist),
                    printable(&lost.title),
                    printable(&lost.version)
                )
            },
        )
    };
    match outcome {
        Outcome::Sent { row } => format!("sent the replay of {}", play(*row)),
        Outcome::Unreadable { row, reason } => {
            format!("kept back the replay of {}: {reason}", play(*row))
        }
        Outcome::Refused { row, error } => {
            format!("the server refused the replay of {}: {error}", play(*row))
        }
        Outcome::Waiting { error } => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::fs;
    use std::path::Path;
    use std::rc::Rc;

    use lost_scores_core::grade::Grade;
    use lost_scores_core::maps::MapStatus;
    use osu_api::{ApiError, Response, Server, Submission, Transport};

    use super::{Outcome, describe, send_queued};
    use crate::history::{SavedScan, Sent, VERSION};
    use crate::rows::{Hits, LostRow};

    struct Fake {
        answers: VecDeque<(u16, &'static str)>,
        requests: Rc<RefCell<Vec<String>>>,
    }

    impl Transport for Fake {
        fn post_form(&mut self, url: &str, _: &[(&str, &str)]) -> Result<Response, ApiError> {
            panic!("unexpected POST {url}")
        }

        fn get(&mut self, url: &str, _: &str) -> Result<Response, ApiError> {
            panic!("unexpected GET {url}")
        }

        fn post_json(&mut self, url: &str, _: &[u8], _: &str) -> Result<Response, ApiError> {
            panic!("unexpected POST {url}")
        }

        fn delete(&mut self, url: &str, _: &str) -> Result<Response, ApiError> {
            panic!("unexpected DELETE {url}")
        }

        fn put(&mut self, url: &str, body: &[u8], _: &str) -> Result<Response, ApiError> {
            self.requests
                .borrow_mut()
                .push(format!("PUT {url} {}", body.len()));
            let (status, body) = self.answers.pop_front().expect("no scripted answer");
            Ok(Response {
                status,
                retry_after: None,
                body: body.as_bytes().to_vec(),
            })
        }
    }

    fn server(answers: &[(u16, &'static str)]) -> (Server<Fake>, Rc<RefCell<Vec<String>>>) {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let fake = Fake {
            answers: answers.iter().copied().collect(),
            requests: Rc::clone(&requests),
        };
        (
            Server::new(fake, "https://api.test").with_session("tok"),
            requests,
        )
    }

    fn row(file: &str) -> LostRow {
        LostRow {
            pp: 321.5,
            kept_pp: 300.25,
            score: 1_234_567,
            kept_score: 1_300_000,
            mods: "HDDT".into(),
            unix_time: 1_700_000_000,
            played: "2023-11-14 22:13".into(),
            status: MapStatus::Ranked,
            beatmap_md5: "0b577cbe9bc774600d20cfa17c0aa94c".into(),
            beatmap_id: Some(11),
            beatmapset_id: None,
            artist: "A".into(),
            title: "B".into(),
            version: "x".into(),
            file: file.into(),
            creator: "m".into(),
            hits: Hits {
                n300: 300,
                n100: 20,
                n50: 1,
                misses: 2,
            },
            accuracy: 96.5,
            grade: Grade::S,
            max_combo: 500,
        }
    }

    fn scan(files: &[&str]) -> SavedScan {
        SavedScan {
            version: VERSION,
            made_at: 1_790_000_000,
            player_id: 7,
            player_name: "Lemon".into(),
            include_all: false,
            check_missing: false,
            lost: files.iter().map(|file| row(file)).collect(),
            lost_total: files.len(),
            lost_ranked: files.len(),
            potential: None,
        }
    }

    fn replay() -> Vec<u8> {
        let mut out = vec![0];
        out.extend(20210520i32.to_le_bytes());
        for text in ["0b577cbe9bc774600d20cfa17c0aa94c", "Lemon", ""] {
            if text.is_empty() {
                out.push(0x00);
            } else {
                out.push(0x0b);
                out.push(u8::try_from(text.len()).unwrap());
                out.extend(text.as_bytes());
            }
        }
        for count in [300u16, 20, 1, 0, 0, 2] {
            out.extend(count.to_le_bytes());
        }
        out.extend(1_234_567u32.to_le_bytes());
        out.extend(500u16.to_le_bytes());
        out.push(0);
        out.extend(72u32.to_le_bytes());
        out.push(0x00);
        out.extend(638_357_968_000_000_000i64.to_le_bytes());
        out.extend(0i32.to_le_bytes());
        out.extend(1i64.to_le_bytes());
        out
    }

    fn sent(queued: &[usize]) -> Sent {
        let answer = Submission {
            id: "abc".into(),
            visibility: "private".into(),
            status: "unverified".into(),
        };
        let mut sent = Sent::new(&answer, "https://api.test");
        sent.queued.extend(queued.iter().copied());
        sent
    }

    fn files(dir: &Path, names: &[(&str, Vec<u8>)]) {
        for (name, bytes) in names {
            fs::write(dir.join(name), bytes).unwrap();
        }
    }

    #[test]
    fn each_outcome_is_told_with_its_play() {
        let saved = scan(&["a.osr"]);
        let told = [
            Outcome::Sent { row: 0 },
            Outcome::Unreadable {
                row: 0,
                reason: "a.osr: replay ends too early".into(),
            },
            Outcome::Refused {
                row: 0,
                error: ApiError::ReplayMismatch,
            },
            Outcome::Waiting {
                error: ApiError::ReplaysPaused { retry_after: 60 },
            },
        ]
        .iter()
        .map(|outcome| describe(outcome, &saved))
        .collect::<Vec<_>>();
        assert_eq!(
            told,
            [
                "sent the replay of A - B [x]",
                "kept back the replay of A - B [x]: a.osr: replay ends too early",
                "the server refused the replay of A - B [x]: this replay is not the lost play it was chosen for",
                "the Lost Scores server takes no new replays right now; they will go out later",
            ]
        );
    }

    #[test]
    fn the_chosen_replays_are_sent_and_remembered() {
        let dir = tempfile::tempdir().unwrap();
        files(dir.path(), &[("a.osr", replay()), ("b.osr", replay())]);
        let (mut api, requests) =
            server(&[(200, r#"{"sha256":"aa"}"#), (200, r#"{"sha256":"bb"}"#)]);
        let mut record = sent(&[0, 1]);
        let outcomes = send_queued(
            &mut api,
            &mut record,
            &scan(&["a.osr", "b.osr"]),
            dir.path(),
        );
        assert!(matches!(
            outcomes[..],
            [Outcome::Sent { row: 0 }, Outcome::Sent { row: 1 }]
        ));
        assert_eq!(
            record.replays.into_iter().collect::<Vec<_>>(),
            [(0, "aa".into()), (1, "bb".into())]
        );
        assert!(record.queued.is_empty());
        let size = replay().len();
        assert_eq!(
            *requests.borrow(),
            [
                format!("PUT https://api.test/api/v1/submissions/abc/replays/0 {size}"),
                format!("PUT https://api.test/api/v1/submissions/abc/replays/1 {size}"),
            ]
        );
    }

    #[test]
    fn a_paused_upload_stays_queued_and_goes_out_later() {
        let dir = tempfile::tempdir().unwrap();
        files(
            dir.path(),
            &[
                ("a.osr", replay()),
                ("b.osr", replay()),
                ("c.osr", replay()),
            ],
        );
        let saved = scan(&["a.osr", "b.osr", "c.osr"]);
        let mut record = sent(&[0, 1, 2]);
        let (mut api, _) = server(&[
            (200, r#"{"sha256":"aa"}"#),
            (503, r#"{"error":"replays_paused","retry_after":3600}"#),
        ]);
        let outcomes = send_queued(&mut api, &mut record, &saved, dir.path());
        assert!(matches!(
            outcomes[..],
            [
                Outcome::Sent { row: 0 },
                Outcome::Waiting {
                    error: ApiError::ReplaysPaused { retry_after: 3600 }
                }
            ]
        ));
        assert_eq!(record.queued.iter().copied().collect::<Vec<_>>(), [1, 2]);
        let (mut later, _) = server(&[(200, r#"{"sha256":"bb"}"#), (200, r#"{"sha256":"cc"}"#)]);
        send_queued(&mut later, &mut record, &saved, dir.path());
        assert!(record.queued.is_empty());
        assert_eq!(record.replays.len(), 3);
    }

    #[test]
    fn a_file_the_app_cannot_read_is_never_sent() {
        let dir = tempfile::tempdir().unwrap();
        let mut broken = replay();
        broken.push(0);
        files(
            dir.path(),
            &[("b.osr", broken), ("c.osr", vec![0; 4 * 1024 * 1024 + 1])],
        );
        let (mut api, requests) = server(&[]);
        let mut record = sent(&[0, 1, 2]);
        let outcomes = send_queued(
            &mut api,
            &mut record,
            &scan(&["a.osr", "b.osr", "c.osr"]),
            dir.path(),
        );
        assert!(matches!(
            outcomes[..],
            [
                Outcome::Unreadable { row: 0, .. },
                Outcome::Unreadable { row: 1, .. },
                Outcome::Unreadable { row: 2, .. }
            ]
        ));
        assert!(record.queued.is_empty() && record.replays.is_empty());
        assert!(requests.borrow().is_empty());
    }

    #[test]
    fn a_refused_replay_is_reported_with_the_servers_reason() {
        let dir = tempfile::tempdir().unwrap();
        files(dir.path(), &[("a.osr", replay()), ("b.osr", replay())]);
        let (mut api, requests) = server(&[
            (400, r#"{"error":"replay_mismatch"}"#),
            (404, r#"{"error":"not_found"}"#),
        ]);
        let mut record = sent(&[0, 1, 2]);
        let outcomes = send_queued(
            &mut api,
            &mut record,
            &scan(&["a.osr", "b.osr", "c.osr"]),
            dir.path(),
        );
        assert!(matches!(
            outcomes[..],
            [
                Outcome::Refused {
                    row: 0,
                    error: ApiError::ReplayMismatch
                },
                Outcome::Refused {
                    row: 1,
                    error: ApiError::NotOnServer
                }
            ]
        ));
        assert!(record.queued.is_empty() && record.replays.is_empty());
        assert_eq!(requests.borrow().len(), 2);
    }
}
