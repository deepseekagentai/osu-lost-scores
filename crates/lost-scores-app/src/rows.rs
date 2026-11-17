use lost_scores_core::grade::{Grade, grade};
use lost_scores_core::lost::LostScore;
use lost_scores_core::maps::{MapCatalog, MapStatus};
use lost_scores_core::potential::{Origin, Play, Potential, WEIGHT};
use lost_scores_core::pp::accuracy;
use osu_api::Score;
use osu_formats::ReplayHeader;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hits {
    pub n300: u32,
    pub n100: u32,
    pub n50: u32,
    pub misses: u32,
}

impl Hits {
    fn of(header: &ReplayHeader) -> Self {
        Self {
            n300: header.count_300.into(),
            n100: header.count_100.into(),
            n50: header.count_50.into(),
            misses: header.count_miss.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LostRow {
    pub pp: f64,
    pub kept_pp: f64,
    pub score: u32,
    pub kept_score: u32,
    pub mods: String,
    pub unix_time: i64,
    pub played: String,
    pub status: MapStatus,
    pub beatmap_md5: String,
    pub beatmap_id: Option<u32>,
    #[serde(default)]
    pub beatmapset_id: Option<u32>,
    pub artist: String,
    pub title: String,
    pub version: String,
    pub file: String,
    pub creator: String,
    pub hits: Hits,
    pub accuracy: f64,
    pub grade: Grade,
    pub max_combo: u32,
}

pub fn rows(lost: &[LostScore], catalog: &MapCatalog) -> Vec<LostRow> {
    lost.iter()
        .map(|score| {
            let header = &score.play.header;
            let metadata = score.play.beatmap.as_ref().map(|beatmap| &beatmap.metadata);
            LostRow {
                pp: score.play.pp.unwrap_or_default(),
                kept_pp: score.kept.pp.unwrap_or_default(),
                score: header.score,
                kept_score: score.kept.header.score,
                mods: header.mods.acronyms(),
                unix_time: header.unix_time(),
                played: date(header.unix_time()),
                status: score.status,
                beatmap_md5: header.beatmap_md5.clone(),
                beatmap_id: catalog
                    .get(&header.beatmap_md5)
                    .and_then(|info| info.beatmap_id)
                    .or(metadata.and_then(|m| m.beatmap_id)),
                beatmapset_id: metadata.and_then(|m| m.beatmapset_id).or_else(|| {
                    catalog
                        .get(&header.beatmap_md5)
                        .and_then(|info| info.beatmapset_id)
                }),
                artist: metadata.map(|m| m.artist.clone()).unwrap_or_default(),
                title: metadata.map(|m| m.title.clone()).unwrap_or_default(),
                version: metadata.map(|m| m.version.clone()).unwrap_or_default(),
                file: score.play.file_name.clone(),
                creator: metadata.map(|m| m.creator.clone()).unwrap_or_default(),
                hits: Hits::of(header),
                accuracy: accuracy(header),
                grade: grade(header),
                max_combo: header.max_combo.into(),
            }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PotentialRow {
    pub place: usize,
    pub pp: f64,
    pub weight: f64,
    pub weighted: f64,
    pub lost: bool,
    pub beatmap_id: u32,
    #[serde(default)]
    pub beatmapset_id: Option<u32>,
    pub artist: String,
    pub title: String,
    pub version: String,
    pub creator: String,
    pub mods: String,
    pub accuracy: f64,
    pub unix_time: i64,
    pub played: String,
    pub hits: Hits,
    pub score: u64,
    pub grade: Grade,
    pub status: MapStatus,
    pub kept_pp: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OnlineRow {
    pub place: usize,
    pub pp: f64,
    pub weight: f64,
    pub weighted: f64,
    pub beatmap_id: u32,
    #[serde(default)]
    pub beatmapset_id: Option<u32>,
    pub artist: String,
    pub title: String,
    pub version: String,
    pub creator: String,
    pub mods: String,
    pub accuracy: f64,
    pub grade: Grade,
    pub hits: Hits,
    pub score: u64,
    pub unix_time: i64,
    pub played: String,
}

fn api_hits(score: &Score) -> Hits {
    Hits {
        n300: score.statistics.count_300,
        n100: score.statistics.count_100,
        n50: score.statistics.count_50,
        misses: score.statistics.count_miss,
    }
}

fn api_grade(score: &Score) -> Grade {
    Grade::from_rank(&score.rank).unwrap_or(Grade::D)
}

fn api_unix_time(score: &Score) -> i64 {
    score
        .created_at
        .parse::<jiff::Timestamp>()
        .map_or(0, |time| time.as_second())
}

fn api_date(score: &Score) -> String {
    score.created_at.get(..10).unwrap_or_default().to_owned()
}

pub fn online_rows(online: &[Score]) -> Vec<OnlineRow> {
    online
        .iter()
        .enumerate()
        .map(|(index, score)| {
            let pp = score.pp.unwrap_or_default();
            let weight = WEIGHT.powi(i32::try_from(index).unwrap_or(i32::MAX));
            OnlineRow {
                place: index + 1,
                pp,
                weight,
                weighted: pp * weight,
                beatmap_id: score.beatmap.id,
                beatmapset_id: Some(score.beatmapset.id),
                artist: score.beatmapset.artist.clone(),
                title: score.beatmapset.title.clone(),
                version: score.beatmap.version.clone(),
                creator: score.beatmapset.creator.clone(),
                mods: score.mods.concat(),
                accuracy: score.accuracy * 100.0,
                grade: api_grade(score),
                hits: api_hits(score),
                score: score.score,
                unix_time: api_unix_time(score),
                played: api_date(score),
            }
        })
        .collect()
}

struct Details {
    lost: bool,
    beatmapset_id: Option<u32>,
    artist: String,
    title: String,
    version: String,
    creator: String,
    mods: String,
    unix_time: i64,
    played: String,
    hits: Hits,
    score: u64,
    grade: Grade,
    status: MapStatus,
    kept_pp: Option<f64>,
}

pub fn potential_rows(
    potential: &Potential,
    online: &[Score],
    found: &[LostScore],
    lost: &[(usize, Play)],
    catalog: &MapCatalog,
) -> Vec<PotentialRow> {
    potential
        .top
        .iter()
        .enumerate()
        .map(|(index, ranked)| {
            let details = match ranked.origin {
                Origin::Online(i) => {
                    let score = &online[i];
                    Details {
                        lost: false,
                        beatmapset_id: Some(score.beatmapset.id),
                        artist: score.beatmapset.artist.clone(),
                        title: score.beatmapset.title.clone(),
                        version: score.beatmap.version.clone(),
                        creator: score.beatmapset.creator.clone(),
                        mods: score.mods.concat(),
                        unix_time: api_unix_time(score),
                        played: api_date(score),
                        hits: api_hits(score),
                        score: score.score,
                        grade: api_grade(score),
                        status: MapStatus::Ranked,
                        kept_pp: None,
                    }
                }
                Origin::Lost(i) => {
                    let lost_score = &found[lost[i].0];
                    let play = lost_score.play;
                    let metadata = play.beatmap.as_ref().map(|beatmap| &beatmap.metadata);
                    Details {
                        lost: true,
                        beatmapset_id: metadata.and_then(|m| m.beatmapset_id).or_else(|| {
                            catalog
                                .get(&play.header.beatmap_md5)
                                .and_then(|info| info.beatmapset_id)
                        }),
                        artist: metadata.map(|m| m.artist.clone()).unwrap_or_default(),
                        title: metadata.map(|m| m.title.clone()).unwrap_or_default(),
                        version: metadata.map(|m| m.version.clone()).unwrap_or_default(),
                        creator: metadata.map(|m| m.creator.clone()).unwrap_or_default(),
                        mods: play.header.mods.acronyms(),
                        unix_time: play.header.unix_time(),
                        played: date(play.header.unix_time()),
                        hits: Hits::of(&play.header),
                        score: play.header.score.into(),
                        grade: grade(&play.header),
                        status: lost_score.status,
                        kept_pp: Some(lost_score.kept.pp.unwrap_or_default()),
                    }
                }
            };
            PotentialRow {
                place: index + 1,
                pp: ranked.play.pp,
                weight: ranked.weight,
                weighted: ranked.play.pp * ranked.weight,
                lost: details.lost,
                beatmap_id: ranked.play.beatmap_id,
                beatmapset_id: details.beatmapset_id,
                artist: details.artist,
                title: details.title,
                version: details.version,
                creator: details.creator,
                mods: details.mods,
                accuracy: ranked.play.accuracy,
                unix_time: details.unix_time,
                played: details.played,
                hits: details.hits,
                score: details.score,
                grade: details.grade,
                status: details.status,
                kept_pp: details.kept_pp,
            }
        })
        .collect()
}

pub fn weight_percent(weight: f64) -> String {
    let percent = weight * 100.0;
    if percent >= 1.0 {
        format!("{percent:.0}%")
    } else if percent >= 0.01 {
        format!("{percent:.2}%")
    } else {
        "<0.01%".to_owned()
    }
}

pub fn mod_acronyms(mods: &str) -> Vec<String> {
    let letters: Vec<char> = mods.chars().collect();
    letters
        .chunks(2)
        .map(|pair| pair.iter().collect::<String>())
        .filter(|acronym| acronym != "NM")
        .collect()
}

pub fn thousands(value: f64) -> String {
    let digits = format!("{:.0}", value.round());
    let (sign, digits) = digits
        .strip_prefix('-')
        .map_or(("", digits.as_str()), |d| ("-", d));
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("{sign}{grouped}")
}

pub fn printable(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

pub fn date(unix: i64) -> String {
    let z = unix.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use lost_scores_core::lost::LostScore;
    use lost_scores_core::maps::{MapCatalog, MapInfo, MapStatus};
    use lost_scores_core::potential::{Origin, Play, Potential, Ranked};
    use lost_scores_core::{LocalBeatmap, ScannedReplay};
    use osu_api::{Score, ScoreBeatmap, ScoreBeatmapset, ScoreStatistics};
    use osu_formats::{BeatmapMetadata, Mods, ReplayHeader};

    use lost_scores_core::grade::Grade;

    use super::{
        Hits, LostRow, date, mod_acronyms, online_rows, potential_rows, rows, thousands,
        weight_percent,
    };

    fn replay(name: &str, score: u32, pp: f64, unix_time: i64) -> ScannedReplay {
        ScannedReplay {
            file_name: name.into(),
            header: ReplayHeader {
                mode: 0,
                version: 20190906,
                beatmap_md5: "aaaa".into(),
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
                mods: Mods::from_bits(8 | 64),
                timestamp_ticks: unix_time * 10_000_000 + 621_355_968_000_000_000,
            },
            beatmap: Some(LocalBeatmap {
                path: PathBuf::from("Songs/1 A - B/x.osu"),
                metadata: BeatmapMetadata {
                    title: "B".into(),
                    artist: "A".into(),
                    creator: "m".into(),
                    version: "x".into(),
                    beatmap_id: Some(11),
                    beatmapset_id: Some(1),
                },
            }),
            pp: Some(pp),
        }
    }

    fn score() -> Score {
        Score {
            id: 1,
            pp: Some(400.0),
            accuracy: 0.98,
            mods: vec!["HD".into(), "DT".into()],
            score: 1_000_000,
            rank: "S".into(),
            created_at: "2024-09-07T21:44:00Z".into(),
            statistics: ScoreStatistics {
                count_300: 100,
                count_100: 2,
                count_50: 0,
                count_miss: 0,
            },
            beatmap: ScoreBeatmap {
                id: 1,
                version: "Expert".into(),
                checksum: None,
            },
            beatmapset: ScoreBeatmapset {
                id: 2,
                artist: "SmokyThrill77".into(),
                title: "Congrathalations!".into(),
                creator: "Smoke".into(),
            },
        }
    }

    fn sample() -> (Potential, Vec<Score>, ScannedReplay) {
        let lost_play = Play {
            beatmap_id: 11,
            pp: 300.0,
            accuracy: 97.0,
        };
        let potential = Potential {
            top: vec![
                Ranked {
                    origin: Origin::Online(0),
                    play: Play {
                        beatmap_id: 1,
                        pp: 400.0,
                        accuracy: 98.0,
                    },
                    weight: 1.0,
                },
                Ranked {
                    origin: Origin::Lost(0),
                    play: lost_play,
                    weight: 0.95,
                },
            ],
            pp: 0.0,
            accuracy: 0.0,
            lost_in_top: 1,
        };
        (
            potential,
            vec![score()],
            replay("p", 1, 300.0, 1_700_000_000),
        )
    }

    #[test]
    fn lost_rows_carry_hits_accuracy_and_grade() {
        let mut play = replay("p.osr", 10_000_000, 300.0, 1_728_470_722);
        play.header.count_300 = 90;
        play.header.count_100 = 8;
        play.header.count_50 = 1;
        play.header.count_miss = 1;
        play.header.max_combo = 321;
        play.header.mods = Mods::from_bits(0);
        let kept = replay("k.osr", 11_000_000, 250.0, 1_728_000_000);
        let lost = [LostScore {
            play: &play,
            kept: &kept,
            status: MapStatus::Loved,
        }];
        let row = &rows(&lost, &MapCatalog::default())[0];
        assert_eq!(
            row.hits,
            Hits {
                n300: 90,
                n100: 8,
                n50: 1,
                misses: 1
            }
        );
        assert!((row.accuracy - 27_850.0 / 300.0).abs() < 1e-9);
        assert_eq!(row.grade, Grade::B);
        assert_eq!(row.max_combo, 321);
        assert_eq!(row.creator, "m");
    }

    #[test]
    fn online_rows_take_the_grade_and_hits_from_the_api_score() {
        let mut second = score();
        second.rank = "XH".into();
        second.pp = Some(200.0);
        second.beatmap.id = 2;
        let rows = online_rows(&[score(), second]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].place, 1);
        assert_eq!(rows[0].grade, Grade::S);
        assert_eq!(
            rows[0].hits,
            Hits {
                n300: 100,
                n100: 2,
                n50: 0,
                misses: 0
            }
        );
        assert_eq!(rows[0].score, 1_000_000);
        assert_eq!(rows[0].mods, "HDDT");
        assert!((rows[0].accuracy - 98.0).abs() < 1e-9);
        assert_eq!(rows[0].creator, "Smoke");
        assert_eq!(rows[0].played, "2024-09-07");
        assert_eq!(rows[0].unix_time, 1_725_745_440);
        let mut broken = score();
        broken.created_at = "garbage".into();
        assert_eq!(online_rows(&[broken])[0].unix_time, 0);
        assert_eq!(rows[0].weight, 1.0);
        assert_eq!(rows[1].place, 2);
        assert_eq!(rows[1].grade, Grade::XH);
        assert!((rows[1].weight - 0.95).abs() < 1e-12);
        assert!((rows[1].weighted - 190.0).abs() < 1e-9);
        assert_eq!(rows[1].beatmap_id, 2);
    }

    #[test]
    fn dates_are_utc_days() {
        for (unix, expected) in [
            (0, "1970-01-01"),
            (-1, "1969-12-31"),
            (951_782_400, "2000-02-29"),
            (1_709_164_800, "2024-02-29"),
            (1_728_470_822, "2024-10-09"),
            (4_102_444_800, "2100-01-01"),
        ] {
            assert_eq!(date(unix), expected, "{unix}");
        }
    }

    #[test]
    fn row_describes_the_lost_play_and_the_kept_one() {
        let play = replay("p.osr", 10_000_000, 300.0, 1_728_470_722);
        let kept = replay("k.osr", 11_000_000, 250.0, 1_728_000_000);
        let lost = [LostScore {
            play: &play,
            kept: &kept,
            status: MapStatus::Ranked,
        }];
        assert_eq!(
            rows(&lost, &MapCatalog::default()),
            [LostRow {
                pp: 300.0,
                kept_pp: 250.0,
                score: 10_000_000,
                kept_score: 11_000_000,
                mods: "HDDT".into(),
                unix_time: 1_728_470_722,
                played: "2024-10-09".into(),
                status: MapStatus::Ranked,
                beatmap_md5: "aaaa".into(),
                beatmap_id: Some(11),
                beatmapset_id: Some(1),
                artist: "A".into(),
                title: "B".into(),
                version: "x".into(),
                file: "p.osr".into(),
                creator: "m".into(),
                hits: Hits {
                    n300: 100,
                    n100: 0,
                    n50: 0,
                    misses: 0
                },
                accuracy: 100.0,
                grade: Grade::XH,
                max_combo: 100,
            }]
        );
    }

    #[test]
    fn potential_rows_carry_weight_and_lost_marks() {
        let (potential, scores, played) = sample();
        let found = [LostScore {
            play: &played,
            kept: &played,
            status: MapStatus::Ranked,
        }];
        let lost = [(0, potential.top[1].play)];
        let rows = potential_rows(&potential, &scores, &found, &lost, &MapCatalog::default());
        assert_eq!(rows[0].weighted, rows[0].pp);
        assert!((rows[1].weighted - rows[1].pp * 0.95).abs() < 1e-9);
        assert!(!rows[0].lost);
        assert!(rows[1].lost);
        assert_eq!(rows[0].title, "Congrathalations!");
        assert_eq!(rows[1].title, "B");
        assert_eq!(rows[0].grade, Grade::S);
        assert_eq!(rows[0].score, 1_000_000);
        assert_eq!(rows[0].status, MapStatus::Ranked);
        assert_eq!(rows[0].kept_pp, None);
        assert_eq!(rows[1].hits.n300, 100);
        assert_eq!(rows[1].grade, Grade::XH);
        assert_eq!(rows[1].score, 1);
        assert_eq!(rows[1].kept_pp, Some(300.0));
        assert_eq!(rows[0].unix_time, 1_725_745_440);
        assert_eq!(rows[1].unix_time, played.header.unix_time());
        assert_ne!(rows[1].unix_time, 0);
    }

    fn catalog_with_set(md5: &str, set: u32) -> MapCatalog {
        let mut catalog = MapCatalog::default();
        catalog.insert(
            md5.into(),
            MapInfo {
                beatmap_id: Some(1),
                beatmapset_id: Some(set),
                status: MapStatus::Ranked,
                ranked_at: None,
                outdated: false,
                checked_at: None,
            },
        );
        catalog
    }

    #[test]
    fn rows_carry_the_beatmapset_id() {
        let play = replay("p.osr", 10_000_000, 300.0, 1_728_470_722);
        let lost = [LostScore {
            play: &play,
            kept: &play,
            status: MapStatus::Ranked,
        }];
        assert_eq!(
            rows(&lost, &MapCatalog::default())[0].beatmapset_id,
            Some(1)
        );
        let mut bare = replay("b.osr", 1, 300.0, 1_728_470_722);
        bare.beatmap = None;
        let missing = [LostScore {
            play: &bare,
            kept: &bare,
            status: MapStatus::Ranked,
        }];
        assert_eq!(
            rows(&missing, &MapCatalog::default())[0].beatmapset_id,
            None
        );
        let mut old_file = replay("o.osr", 10_000_000, 300.0, 1_728_470_722);
        if let Some(beatmap) = old_file.beatmap.as_mut() {
            beatmap.metadata.beatmapset_id = None;
        }
        let mut catalog = MapCatalog::default();
        catalog.insert(
            old_file.header.beatmap_md5.clone(),
            MapInfo {
                beatmap_id: Some(10),
                beatmapset_id: Some(44),
                status: MapStatus::Ranked,
                ranked_at: None,
                outdated: false,
                checked_at: None,
            },
        );
        let old = [LostScore {
            play: &old_file,
            kept: &old_file,
            status: MapStatus::Ranked,
        }];
        assert_eq!(rows(&old, &catalog)[0].beatmapset_id, Some(44));
        assert_eq!(online_rows(&[score()])[0].beatmapset_id, Some(2));
        let (potential, scores, played) = sample();
        let found = [LostScore {
            play: &played,
            kept: &played,
            status: MapStatus::Ranked,
        }];
        let lost_plays = [(0, potential.top[1].play)];
        let potential_rows = potential_rows(
            &potential,
            &scores,
            &found,
            &lost_plays,
            &MapCatalog::default(),
        );
        assert_eq!(potential_rows[0].beatmapset_id, Some(2));
        assert_eq!(potential_rows[1].beatmapset_id, Some(1));
        let (_, _, mut bare_played) = sample();
        if let Some(beatmap) = bare_played.beatmap.as_mut() {
            beatmap.metadata.beatmapset_id = None;
        }
        let bare_found = [LostScore {
            play: &bare_played,
            kept: &bare_played,
            status: MapStatus::Ranked,
        }];
        let from_catalog = super::potential_rows(
            &potential,
            &scores,
            &bare_found,
            &lost_plays,
            &catalog_with_set(&bare_played.header.beatmap_md5, 45),
        );
        assert_eq!(from_catalog[1].beatmapset_id, Some(45));
    }

    #[test]
    fn mod_acronyms_split_pairs_and_drop_nm() {
        assert_eq!(mod_acronyms("HDDT"), ["HD", "DT"]);
        assert_eq!(mod_acronyms("HR"), ["HR"]);
        assert!(mod_acronyms("NM").is_empty());
        assert!(mod_acronyms("").is_empty());
    }

    #[test]
    fn weights_below_a_percent_keep_their_decimals() {
        for (weight, expected) in [
            (1.0, "100%"),
            (0.95, "95%"),
            (0.95_f64.powi(89), "1%"),
            (0.95_f64.powi(104), "0.48%"),
            (0.95_f64.powi(199), "<0.01%"),
        ] {
            assert_eq!(weight_percent(weight), expected, "{weight}");
        }
    }

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(8557.4), "8,557");
        assert_eq!(thousands(1_234_567.6), "1,234,568");
        assert_eq!(thousands(12.0), "12");
    }
}
