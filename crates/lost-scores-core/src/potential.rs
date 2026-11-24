use std::collections::HashMap;

use crate::lost::LostScore;
use crate::maps::MapCatalog;
use crate::pp::accuracy;

pub const TOP_SIZE: usize = 200;
pub const WEIGHT: f64 = 0.95;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Play {
    pub beatmap_id: u32,
    pub pp: f64,
    pub accuracy: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Online(usize),
    Lost(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ranked {
    pub origin: Origin,
    pub play: Play,
    pub weight: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Potential {
    pub top: Vec<Ranked>,
    pub pp: f64,
    pub accuracy: f64,
    pub lost_in_top: usize,
}

pub fn weighted_sum(plays: &[Play]) -> f64 {
    let mut pp: Vec<f64> = plays.iter().map(|play| play.pp).collect();
    pp.sort_by(|a, b| b.total_cmp(a));
    pp.iter()
        .take(TOP_SIZE)
        .zip(weights())
        .map(|(pp, weight)| pp * weight)
        .sum()
}

pub fn potential(online: &[Play], lost: &[Play], current_pp: f64) -> Potential {
    let mut slots: Vec<(Origin, Play)> = Vec::new();
    let mut by_map: HashMap<u32, usize> = HashMap::new();
    let entries = online
        .iter()
        .enumerate()
        .map(|(i, play)| (Origin::Online(i), *play))
        .chain(
            lost.iter()
                .enumerate()
                .map(|(i, play)| (Origin::Lost(i), *play)),
        );
    for (origin, play) in entries {
        match by_map.get(&play.beatmap_id) {
            Some(&slot) => {
                if play.pp > slots[slot].1.pp {
                    slots[slot] = (origin, play);
                }
            }
            None => {
                by_map.insert(play.beatmap_id, slots.len());
                slots.push((origin, play));
            }
        }
    }
    slots.sort_by(|a, b| b.1.pp.total_cmp(&a.1.pp));
    let top: Vec<Ranked> = slots
        .into_iter()
        .take(TOP_SIZE)
        .zip(weights())
        .map(|((origin, play), weight)| Ranked {
            origin,
            play,
            weight,
        })
        .collect();
    let total_weight: f64 = top.iter().map(|ranked| ranked.weight).sum();
    let accuracy = if top.is_empty() {
        0.0
    } else {
        top.iter()
            .map(|ranked| ranked.play.accuracy * ranked.weight)
            .sum::<f64>()
            / total_weight
    };
    let weighted_top: f64 = top
        .iter()
        .map(|ranked| ranked.play.pp * ranked.weight)
        .sum();
    Potential {
        pp: current_pp - weighted_sum(online) + weighted_top,
        accuracy,
        lost_in_top: top
            .iter()
            .filter(|ranked| matches!(ranked.origin, Origin::Lost(_)))
            .count(),
        top,
    }
}

pub fn lost_plays(found: &[LostScore], catalog: &MapCatalog) -> Vec<(usize, Play)> {
    found
        .iter()
        .enumerate()
        .filter(|(_, lost)| lost.status.gives_pp())
        .filter_map(|(index, lost)| {
            let beatmap_id = catalog
                .get(&lost.play.header.beatmap_md5)
                .and_then(|info| info.beatmap_id)
                .or_else(|| {
                    lost.play
                        .beatmap
                        .as_ref()
                        .and_then(|beatmap| beatmap.metadata.beatmap_id)
                })?;
            Some((
                index,
                Play {
                    beatmap_id,
                    pp: lost.play.pp?,
                    accuracy: accuracy(&lost.play.header),
                },
            ))
        })
        .collect()
}

fn weights() -> impl Iterator<Item = f64> {
    (0..).map(|place| WEIGHT.powi(place))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use osu_formats::{BeatmapMetadata, Mods, ReplayHeader};

    use super::*;
    use crate::lost::LostScore;
    use crate::maps::{MapCatalog, MapInfo, MapStatus};
    use crate::{LocalBeatmap, ScannedReplay};

    fn replay(md5: &str, local_id: Option<u32>) -> ScannedReplay {
        ScannedReplay {
            file_name: format!("{md5}.osr"),
            header: ReplayHeader {
                mode: 0,
                version: 20190906,
                beatmap_md5: md5.into(),
                player_name: "Lemon".into(),
                replay_md5: String::new(),
                count_300: 90,
                count_100: 10,
                count_50: 0,
                count_geki: 0,
                count_katu: 0,
                count_miss: 0,
                score: 1_000_000,
                max_combo: 100,
                perfect: false,
                mods: Mods::from_bits(0),
                timestamp_ticks: 0,
            },
            beatmap: Some(LocalBeatmap {
                path: PathBuf::from("Songs").join(md5),
                metadata: BeatmapMetadata {
                    beatmap_id: local_id,
                    ..BeatmapMetadata::default()
                },
            }),
            pp: Some(300.0),
        }
    }

    fn catalog(entries: &[(&str, Option<u32>, MapStatus)]) -> MapCatalog {
        let mut catalog = MapCatalog::default();
        for (md5, beatmap_id, status) in entries {
            catalog.insert(
                (*md5).into(),
                MapInfo {
                    beatmap_id: *beatmap_id,
                    beatmapset_id: None,
                    status: *status,
                    ranked_at: None,
                    outdated: false,
                    checked_at: None,
                },
            );
        }
        catalog
    }

    fn lost<'a>(replay: &'a ScannedReplay, status: MapStatus) -> LostScore<'a> {
        LostScore {
            play: replay,
            kept: replay,
            counted: replay.pp.unwrap_or_default(),
            status,
        }
    }

    #[test]
    fn only_maps_that_give_pp_enter() {
        let statuses = [
            MapStatus::Ranked,
            MapStatus::Approved,
            MapStatus::Loved,
            MapStatus::Graveyard,
            MapStatus::Unknown,
        ];
        let replays: Vec<ScannedReplay> = (0..statuses.len())
            .map(|i| replay(&format!("m{i}"), Some(100 + i as u32)))
            .collect();
        let found: Vec<LostScore> = replays
            .iter()
            .zip(statuses)
            .map(|(replay, status)| lost(replay, status))
            .collect();
        let indices: Vec<usize> = lost_plays(&found, &MapCatalog::default())
            .into_iter()
            .map(|(index, _)| index)
            .collect();
        assert_eq!(indices, [0, 1]);
    }

    #[test]
    fn beatmap_id_comes_from_the_catalog_then_the_local_file() {
        let replays = [replay("a", Some(9)), replay("b", Some(9))];
        let found = [
            lost(&replays[0], MapStatus::Ranked),
            lost(&replays[1], MapStatus::Ranked),
        ];
        let catalog = catalog(&[
            ("a", Some(7), MapStatus::Ranked),
            ("b", None, MapStatus::Ranked),
        ]);
        let ids: Vec<u32> = lost_plays(&found, &catalog)
            .into_iter()
            .map(|(_, play)| play.beatmap_id)
            .collect();
        assert_eq!(ids, [7, 9]);
    }

    #[test]
    fn plays_without_a_beatmap_id_are_skipped() {
        let replays = [replay("a", None)];
        let found = [lost(&replays[0], MapStatus::Ranked)];
        assert!(lost_plays(&found, &MapCatalog::default()).is_empty());
    }

    fn p(beatmap_id: u32, pp: f64, accuracy: f64) -> Play {
        Play {
            beatmap_id,
            pp,
            accuracy,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn ids(potential: &Potential) -> Vec<u32> {
        potential.top.iter().map(|r| r.play.beatmap_id).collect()
    }

    #[test]
    fn lost_play_replaces_a_weaker_online_play_on_its_map() {
        let result = potential(&[p(1, 300.0, 98.0)], &[p(1, 320.0, 97.0)], 0.0);
        assert_eq!(result.top.len(), 1);
        assert_eq!(result.top[0].origin, Origin::Lost(0));
    }

    #[test]
    fn equal_pp_keeps_the_online_play() {
        let result = potential(&[p(1, 300.0, 98.0)], &[p(1, 300.0, 99.0)], 0.0);
        assert_eq!(result.top[0].origin, Origin::Online(0));
    }

    #[test]
    fn a_stronger_repeat_online_play_wins_over_the_first_one_and_a_lost_play() {
        let online = [p(1, 300.0, 98.0), p(1, 350.0, 99.0)];
        let result = potential(&online, &[p(1, 320.0, 97.0)], 0.0);
        assert_eq!(result.top.len(), 1);
        assert_eq!(result.top[0].origin, Origin::Online(1));
        assert!(close(result.top[0].play.pp, 350.0));
        assert_eq!(result.lost_in_top, 0);
    }

    #[test]
    fn ties_keep_the_order_of_first_appearance() {
        let online = [p(10, 200.0, 0.0), p(11, 300.0, 0.0), p(12, 200.0, 0.0)];
        let result = potential(&online, &[p(13, 200.0, 0.0)], 0.0);
        assert_eq!(ids(&result), [11, 10, 12, 13]);
    }

    #[test]
    fn top_keeps_200_plays_weighted_by_095() {
        let online: Vec<Play> = (0..210).map(|i| p(i, 1000.0 - f64::from(i), 0.0)).collect();
        let result = potential(&online, &[], 0.0);
        assert_eq!(result.top.len(), 200);
        assert!(close(result.top[1].weight, 0.95));
        assert!(close(result.top[199].weight, 0.95f64.powi(199)));
    }

    #[test]
    fn potential_pp_swaps_the_weighted_online_top_for_the_new_one() {
        let online = [p(1, 100.0, 0.0), p(2, 50.0, 0.0)];
        let result = potential(&online, &[p(3, 80.0, 0.0)], 1000.0);
        let expected = 1000.0 - (100.0 + 50.0 * 0.95) + (100.0 + 80.0 * 0.95 + 50.0 * 0.95 * 0.95);
        assert!(close(result.pp, expected));
    }

    #[test]
    fn accuracy_is_weighted_like_pp() {
        let result = potential(&[p(1, 100.0, 100.0), p(2, 50.0, 90.0)], &[], 0.0);
        assert!(close(result.accuracy, (100.0 + 90.0 * 0.95) / (1.0 + 0.95)));
    }

    #[test]
    fn lost_in_top_counts_lost_plays() {
        let result = potential(
            &[p(1, 100.0, 0.0)],
            &[p(2, 90.0, 0.0), p(3, 80.0, 0.0)],
            0.0,
        );
        assert_eq!(result.lost_in_top, 2);
    }
}
