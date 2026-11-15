use std::sync::Arc;

use lost_scores_app::history::Sent;
use lost_scores_app::online::USER_AGENT;
use lost_scores_app::pipeline::PotentialResult;
use lost_scores_app::rows::thousands;
use lost_scores_app::session::Session as Login;
use osu_api::{ApiError, Http, Server, Submission, SubmissionInfo};

pub enum Call {
    Submit(Vec<u8>),
    Publish(String),
    Unpublish(String),
    Delete(String),
    List,
}

pub enum Reply {
    Sent(Submission),
    Deleted,
    Listed(Vec<SubmissionInfo>),
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
        }
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
