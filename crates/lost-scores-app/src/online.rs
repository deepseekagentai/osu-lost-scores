use crate::pipeline::Api;
use lost_scores_core::maps::{FetchedMap, MapInfo, MapSource, MapStatus};
use lost_scores_core::missing::OsuFiles;
use osu_api::{ApiError, Beatmap, Client, Credentials, Http, Score};

pub const USER_AGENT: &str = concat!("lost-scores/", env!("CARGO_PKG_VERSION"));

pub struct Online {
    client: Client<Http>,
}

pub struct Player {
    pub id: u32,
    pub names: Vec<String>,
    pub pp: f64,
    pub accuracy: f64,
    pub global_rank: Option<u32>,
}

impl Online {
    pub fn new(credentials: Credentials) -> Self {
        Self {
            client: Client::new(Http::new(USER_AGENT), credentials),
        }
    }

    pub fn check(&mut self) -> Result<(), ApiError> {
        self.client.check()
    }
}

impl Api for Online {
    fn player(&mut self, name: &str) -> Result<Player, ApiError> {
        let user = self.client.user(name)?;
        let statistics = user.statistics;
        Ok(Player {
            id: user.id,
            names: all_names(name, user.username, user.previous_usernames),
            pp: statistics.as_ref().map_or(0.0, |s| s.pp),
            accuracy: statistics.as_ref().map_or(0.0, |s| s.hit_accuracy),
            global_rank: statistics.and_then(|s| s.global_rank),
        })
    }

    fn top(&mut self, user_id: u32) -> Result<Vec<Score>, ApiError> {
        self.client.top_plays(user_id)
    }
}

impl MapSource for Online {
    type Error = ApiError;

    fn batch(&mut self, ids: &[u32]) -> Result<Vec<FetchedMap>, ApiError> {
        Ok(self
            .client
            .beatmaps(ids)?
            .into_iter()
            .map(fetched)
            .collect())
    }

    fn lookup(&mut self, md5: &str) -> Result<Option<FetchedMap>, ApiError> {
        Ok(self.client.lookup(md5)?.map(fetched))
    }
}

impl OsuFiles for Online {
    type Error = ApiError;

    fn download(&mut self, beatmap_id: u32) -> Result<Vec<u8>, ApiError> {
        self.client.osu_file(beatmap_id)
    }
}

fn all_names(player: &str, current: String, previous: Vec<String>) -> Vec<String> {
    let mut names = vec![player.to_owned()];
    for name in std::iter::once(current).chain(previous) {
        if !names
            .iter()
            .any(|known| known.to_lowercase() == name.to_lowercase())
        {
            names.push(name);
        }
    }
    names
}

fn fetched(beatmap: Beatmap) -> FetchedMap {
    let ranked_at = beatmap
        .beatmapset
        .as_ref()
        .and_then(|set| set.ranked_date.as_deref())
        .and_then(osu_api::unix_time);
    FetchedMap {
        checksum: beatmap.checksum,
        info: MapInfo {
            beatmap_id: Some(beatmap.id),
            status: MapStatus::from_name(&beatmap.status),
            ranked_at,
            outdated: false,
            checked_at: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use lost_scores_core::maps::{FetchedMap, MapInfo, MapStatus};
    use osu_api::{Beatmap, Beatmapset};

    use super::{all_names, fetched};

    #[test]
    fn api_beatmap_becomes_a_catalog_entry() {
        let beatmap = Beatmap {
            id: 402752,
            checksum: Some("dd00049a".into()),
            status: "loved".into(),
            beatmapset: Some(Beatmapset {
                status: "loved".into(),
                ranked_date: Some("2014-08-05T13:02:24Z".into()),
            }),
        };
        assert_eq!(
            fetched(beatmap),
            FetchedMap {
                checksum: Some("dd00049a".into()),
                info: MapInfo {
                    beatmap_id: Some(402752),
                    status: MapStatus::Loved,
                    ranked_at: Some(1_407_243_744),
                    outdated: false,
                    checked_at: None,
                },
            }
        );
    }

    #[test]
    fn player_names_include_the_current_and_previous_ones_once() {
        assert_eq!(
            all_names("lemon", "Lemon".into(), vec!["Old".into(), "old".into()]),
            ["lemon", "Old"]
        );
    }
}
