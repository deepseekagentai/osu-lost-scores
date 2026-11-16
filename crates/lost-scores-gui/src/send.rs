use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use lost_scores_app::history::{self, Sent};
use lost_scores_app::online::USER_AGENT;
use lost_scores_app::pipeline::PotentialResult;
use lost_scores_app::replays::{self, Change, Outcome};
use lost_scores_app::rows::thousands;
use lost_scores_app::session::Session as Login;
use osu_api::{ApiError, Http, Server, Submission, SubmissionInfo};

pub enum Call {
    Submit(Vec<u8>),
    Publish(String),
    Unpublish(String),
    Delete(String),
    List,
    Replay {
        id: String,
        row: usize,
        bytes: Vec<u8>,
    },
}

pub enum Reply {
    Sent(Submission),
    Deleted,
    Listed(Vec<SubmissionInfo>),
    Replayed(String),
}

pub type Courier = Arc<dyn Fn(&Login, Call) -> Result<Reply, ApiError> + Send + Sync>;

pub fn over_http() -> Courier {
    Arc::new(|login: &Login, call: Call| {
        let mut server =
            Server::new(Http::new(USER_AGENT), &login.server).with_session(&login.token);
        match call {
            Call::Submit(body) => server.submit(&body).map(Reply::Sent),
            Call::Publish(id) => server.publish(&id).map(Reply::Sent),
            Call::Unpublish(id) => server.unpublish(&id).map(Reply::Sent),
            Call::Delete(id) => server.delete_submission(&id).map(|()| Reply::Deleted),
            Call::List => server.submissions().map(Reply::Listed),
            Call::Replay { id, row, bytes } => {
                server.upload_replay(&id, row, &bytes).map(Reply::Replayed)
            }
        }
    })
}

#[derive(Debug, Default)]
pub struct Shared {
    pub lines: Vec<String>,
    pub sent: usize,
    pub waiting: Option<String>,
}

impl Shared {
    pub fn summary(&self) -> Option<String> {
        match (self.sent, &self.waiting) {
            (0, Some(waiting)) => Some(crate::account::sentence(waiting)),
            (0, None) => self
                .lines
                .first()
                .map(|line| crate::account::sentence(line)),
            (1, _) => Some("Shared 1 replay".into()),
            (count, _) => Some(format!("Shared {count} replays")),
        }
    }
}

pub fn share_replays(
    courier: &Courier,
    login: &Login,
    folder: &Path,
    replays_dir: &Path,
    now: i64,
    report: &mut dyn FnMut(&Change) -> bool,
) -> Result<Shared> {
    let scan = history::load(folder)?;
    let Some(mut sent) = history::submission(folder, &login.server) else {
        return Ok(Shared::default());
    };
    let mut upload = |id: &str, row: usize, bytes: &[u8]| {
        let call = Call::Replay {
            id: id.to_owned(),
            row,
            bytes: bytes.to_vec(),
        };
        match courier(login, call)? {
            Reply::Replayed(sha256) => Ok(sha256),
            _ => Err(ApiError::Server(
                "the server gave an unexpected answer".into(),
            )),
        }
    };
    let outcomes = replays::send_queued(
        &mut upload,
        &mut sent,
        &scan,
        replays_dir,
        now,
        &mut |_, change| change.is_none_or(&mut *report),
    );
    Ok(Shared {
        lines: outcomes
            .iter()
            .map(|outcome| replays::describe(outcome, &scan))
            .collect(),
        sent: outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Outcome::Sent { .. }))
            .count(),
        waiting: outcomes.iter().find_map(|outcome| match outcome {
            Outcome::Waiting { error } => Some(error.to_string()),
            _ => None,
        }),
    })
}

pub fn label(sent: &Sent) -> String {
    if published(sent) {
        format!("Published, {}", sent.status)
    } else {
        "Private".to_owned()
    }
}

pub fn published(sent: &Sent) -> bool {
    sent.visibility == "public"
}

pub fn done(before: Option<&Sent>, after: &Sent) -> &'static str {
    match (before.is_some_and(published), published(after)) {
        (_, true) => "Published to the Hall of Fame",
        (true, false) => "Taken out of the Hall of Fame",
        (false, false) => "Sent to your profile",
    }
}

pub fn reconcile(sent: &Sent, listed: &[SubmissionInfo]) -> Option<Sent> {
    listed
        .iter()
        .find(|info| info.id == sent.id)
        .map(|info| Sent {
            visibility: info.visibility.clone(),
            status: info.status.clone(),
            ..sent.clone()
        })
}

pub fn offer(top: &PotentialResult) -> Option<String> {
    let lost = top.potential_pp - top.current_pp;
    (lost >= 0.5).then(|| {
        format!(
            "ScoreV1 cost you {} pp: {} \u{2192} {} pp. Show it next to other players?",
            thousands(lost),
            thousands(top.current_pp),
            thousands(top.potential_pp)
        )
    })
}

pub const SERVER_COPY_STAYS: &str =
    " Its copy on the Lost Scores server stays; delete it there first if you want it gone.";

pub fn server_copies(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => " 1 of them stays on the Lost Scores server.".to_owned(),
        many => format!(" {many} of them stay on the Lost Scores server."),
    }
}
