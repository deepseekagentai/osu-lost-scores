use std::cmp::Ordering;
use std::collections::HashMap;
use std::ptr;

use crate::ScannedReplay;
use crate::maps::{MapCatalog, MapStatus};
use crate::pp::reference_mod_bits;

pub const SCORE_V1_END: i64 = 1_728_470_822;

#[derive(Debug)]
pub struct LostScore<'a> {
    pub play: &'a ScannedReplay,
    pub kept: &'a ScannedReplay,
    pub status: MapStatus,
}

pub fn find<'a>(replays: &'a [ScannedReplay], catalog: &MapCatalog) -> Vec<LostScore<'a>> {
    let mut groups: HashMap<(&str, u32), Vec<Play<'a>>> = HashMap::new();
    let mut best_on_map: HashMap<&str, Play<'a>> = HashMap::new();
    for replay in replays {
        let Some(pp) = replay.pp else {
            continue;
        };
        let md5 = replay.header.beatmap_md5.as_str();
        let ranked_at = catalog.get(md5).and_then(|info| info.ranked_at);
        if ranked_at.is_some_and(|at| replay.header.unix_time() < at) {
            continue;
        }
        let play = (replay, pp);
        groups
            .entry((md5, reference_mod_bits(replay.header.mods)))
            .or_default()
            .push(play);
        best_on_map
            .entry(md5)
            .and_modify(|best| {
                if by_pp(&play, best) == Ordering::Greater {
                    *best = play;
                }
            })
            .or_insert(play);
    }

    let mut lost = Vec::new();
    for ((md5, _), plays) in &groups {
        let Some(candidate) = first_max(plays.iter(), by_pp) else {
            continue;
        };
        let in_range = plays
            .iter()
            .filter(|play| play.0.header.unix_time() < SCORE_V1_END);
        let Some(kept) = first_max(in_range, by_score) else {
            continue;
        };
        let is_lost = !ptr::eq(candidate.0, kept.0)
            && candidate.1 > kept.1
            && candidate.0.header.score < kept.0.header.score
            && candidate.0.header.unix_time() < SCORE_V1_END
            && best_on_map
                .get(md5)
                .is_some_and(|best| ptr::eq(best.0, candidate.0));
        if is_lost {
            lost.push(LostScore {
                play: candidate.0,
                kept: kept.0,
                status: catalog
                    .get(md5)
                    .map_or(MapStatus::Unknown, |info| info.status),
            });
        }
    }
    lost.sort_by(|a, b| {
        pp_of(b.play)
            .total_cmp(&pp_of(a.play))
            .then(
                a.play
                    .header
                    .timestamp_ticks
                    .cmp(&b.play.header.timestamp_ticks),
            )
            .then(a.play.file_name.cmp(&b.play.file_name))
    });
    lost
}

type Play<'a> = (&'a ScannedReplay, f64);

fn pp_of(replay: &ScannedReplay) -> f64 {
    replay.pp.unwrap_or_default()
}

fn by_pp(a: &Play, b: &Play) -> Ordering {
    a.1.total_cmp(&b.1)
        .then(a.0.header.score.cmp(&b.0.header.score))
        .then(b.0.header.unix_time().cmp(&a.0.header.unix_time()))
}

fn by_score(a: &Play, b: &Play) -> Ordering {
    a.0.header
        .score
        .cmp(&b.0.header.score)
        .then(a.1.total_cmp(&b.1))
        .then(b.0.header.unix_time().cmp(&a.0.header.unix_time()))
}

fn first_max<'p, 'a: 'p>(
    plays: impl Iterator<Item = &'p Play<'a>>,
    cmp: fn(&Play, &Play) -> Ordering,
) -> Option<&'p Play<'a>> {
    plays.fold(None, |best, play| match best {
        Some(best) if cmp(play, best) != Ordering::Greater => Some(best),
        _ => Some(play),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use osu_formats::{BeatmapMetadata, Mods, ReplayHeader};

    use super::{SCORE_V1_END, find};
    use crate::maps::MapCatalog;
    use crate::{LocalBeatmap, ScannedReplay};

    const BEFORE: i64 = 1_700_000_000;
    const AFTER: i64 = SCORE_V1_END + 1;
    const HD: u32 = 8;
    const TD: u32 = 4;

    fn play(
        name: &str,
        md5: &str,
        mods: u32,
        score: u32,
        pp: Option<f64>,
        unix_time: i64,
    ) -> ScannedReplay {
        ScannedReplay {
            file_name: name.into(),
            header: ReplayHeader {
                mode: 0,
                version: 20190906,
                beatmap_md5: md5.into(),
                player_name: "Lemon".into(),
                replay_md5: String::new(),
                count_300: 100,
                count_100: 0,
                count_50: 0,
                count_geki: 0,
                count_katu: 0,
                count_miss: 0,
                score,
                max_combo: 100,
                perfect: false,
                mods: Mods::from_bits(mods),
                timestamp_ticks: unix_time * 10_000_000 + 621_355_968_000_000_000,
            },
            beatmap: Some(LocalBeatmap {
                path: PathBuf::from("Songs").join(md5),
                metadata: BeatmapMetadata::default(),
            }),
            pp,
        }
    }

    fn lost_names(replays: &[ScannedReplay], catalog: &MapCatalog) -> Vec<(String, String)> {
        find(replays, catalog)
            .into_iter()
            .map(|lost| (lost.play.file_name.clone(), lost.kept.file_name.clone()))
            .collect()
    }

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn play_overwritten_by_a_later_higher_score_is_lost() {
        let replays = [
            play("p", "m", 0, 10_000_000, Some(300.0), BEFORE),
            play("k", "m", 0, 11_000_000, Some(250.0), BEFORE + 100),
        ];
        assert_eq!(
            lost_names(&replays, &MapCatalog::default()),
            pairs(&[("p", "k")])
        );
    }

    #[test]
    fn play_below_an_earlier_higher_score_is_lost() {
        let replays = [
            play("k", "m", 0, 11_000_000, Some(250.0), BEFORE),
            play("p", "m", 0, 10_000_000, Some(300.0), BEFORE + 100),
        ];
        assert_eq!(
            lost_names(&replays, &MapCatalog::default()),
            pairs(&[("p", "k")])
        );
    }

    #[test]
    fn play_after_the_switch_is_kept() {
        let replays = [
            play("k", "m", 0, 11_000_000, Some(250.0), BEFORE),
            play("p", "m", 0, 10_000_000, Some(300.0), AFTER),
        ];
        assert!(lost_names(&replays, &MapCatalog::default()).is_empty());
    }

    #[test]
    fn record_beaten_only_after_the_switch_survives() {
        let replays = [
            play("p", "m", 0, 10_000_000, Some(300.0), BEFORE),
            play("k", "m", 0, 11_000_000, Some(250.0), AFTER),
        ];
        assert!(lost_names(&replays, &MapCatalog::default()).is_empty());
    }

    #[test]
    fn only_the_best_play_on_the_map_counts() {
        let replays = [
            play("p", "m", 0, 10_000_000, Some(300.0), BEFORE),
            play("k", "m", 0, 11_000_000, Some(250.0), BEFORE + 100),
            play("hd", "m", HD, 9_000_000, Some(320.0), BEFORE + 200),
        ];
        assert!(lost_names(&replays, &MapCatalog::default()).is_empty());
    }

    #[test]
    fn plays_before_ranking_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("maps.json");
        let ranked_at = BEFORE + 50;
        std::fs::write(
            &path,
            format!(r#"{{"version": 1, "maps": {{"m": {{"beatmap_id": 1, "status": "ranked", "ranked_at": {ranked_at}}}}}}}"#),
        )
        .unwrap();
        let catalog = MapCatalog::load(&path).unwrap();
        for replays in [
            [
                play("k", "m", 0, 11_000_000, Some(250.0), BEFORE),
                play("p", "m", 0, 10_000_000, Some(300.0), BEFORE + 100),
            ],
            [
                play("p", "m", 0, 10_000_000, Some(300.0), BEFORE),
                play("k", "m", 0, 11_000_000, Some(250.0), BEFORE + 100),
            ],
        ] {
            assert!(lost_names(&replays, &catalog).is_empty());
        }
    }

    #[test]
    fn equal_pp_goes_to_the_higher_score() {
        let replays = [
            play("p1", "m", 0, 10_000_000, Some(300.0), BEFORE),
            play("p2", "m", 0, 10_500_000, Some(300.0), BEFORE + 100),
            play("k", "m", 0, 11_000_000, Some(250.0), BEFORE + 200),
        ];
        assert_eq!(
            lost_names(&replays, &MapCatalog::default()),
            pairs(&[("p2", "k")])
        );
    }

    #[test]
    fn touch_device_plays_share_a_group_like_the_reference() {
        let replays = [
            play("p", "m", TD, 10_000_000, Some(300.0), BEFORE),
            play("k", "m", 0, 11_000_000, Some(250.0), BEFORE + 100),
        ];
        assert_eq!(
            lost_names(&replays, &MapCatalog::default()),
            pairs(&[("p", "k")])
        );
    }

    #[test]
    fn replays_without_pp_are_skipped() {
        let replays = [
            play("p", "m", 0, 10_000_000, None, BEFORE),
            play("k", "m", 0, 11_000_000, Some(250.0), BEFORE + 100),
        ];
        assert!(lost_names(&replays, &MapCatalog::default()).is_empty());
    }

    #[test]
    fn lost_scores_come_sorted_by_pp() {
        let replays = [
            play("p1", "a", 0, 10_000_000, Some(200.0), BEFORE),
            play("k1", "a", 0, 11_000_000, Some(150.0), BEFORE + 100),
            play("p2", "b", 0, 10_000_000, Some(300.0), BEFORE),
            play("k2", "b", 0, 11_000_000, Some(250.0), BEFORE + 100),
        ];
        assert_eq!(
            lost_names(&replays, &MapCatalog::default()),
            pairs(&[("p2", "k2"), ("p1", "k1")])
        );
    }
}
