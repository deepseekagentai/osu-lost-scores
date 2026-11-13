use lost_scores_core::lost::LostScore;
use lost_scores_core::maps::{MapCatalog, MapStatus};
use serde::Serialize;

#[derive(Debug, PartialEq, Serialize)]
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
    pub artist: String,
    pub title: String,
    pub version: String,
    pub file: String,
}

pub fn rows(lost: &[LostScore], catalog: &MapCatalog) -> Vec<LostRow> {
    lost.iter()
        .map(|lost| {
            let header = &lost.play.header;
            let metadata = lost.play.beatmap.as_ref().map(|beatmap| &beatmap.metadata);
            LostRow {
                pp: lost.play.pp.unwrap_or_default(),
                kept_pp: lost.kept.pp.unwrap_or_default(),
                score: header.score,
                kept_score: lost.kept.header.score,
                mods: header.mods.acronyms(),
                unix_time: header.unix_time(),
                played: date(header.unix_time()),
                status: lost.status,
                beatmap_md5: header.beatmap_md5.clone(),
                beatmap_id: catalog
                    .get(&header.beatmap_md5)
                    .and_then(|info| info.beatmap_id)
                    .or(metadata.and_then(|m| m.beatmap_id)),
                artist: metadata.map(|m| m.artist.clone()).unwrap_or_default(),
                title: metadata.map(|m| m.title.clone()).unwrap_or_default(),
                version: metadata.map(|m| m.version.clone()).unwrap_or_default(),
                file: lost.play.file_name.clone(),
            }
        })
        .collect()
}

pub fn table(rows: &[LostRow], limit: usize, with_status: bool) -> String {
    let shown = if limit == 0 {
        rows.len()
    } else {
        limit.min(rows.len())
    };
    let status_column = |text: &str| {
        if with_status {
            format!("  {text:<9}")
        } else {
            String::new()
        }
    };
    let mut lines = vec![format!(
        "{:>4}  {:>7}  {:>7}  {:<6}  {:<10}{}  beatmap",
        "#",
        "pp",
        "kept",
        "mods",
        "played",
        status_column("status")
    )];
    for (index, row) in rows.iter().take(shown).enumerate() {
        let mods = if row.mods.is_empty() { "NM" } else { &row.mods };
        lines.push(format!(
            "{:>4}  {:>7.2}  {:>7.2}  {:<6}  {:<10}{}  {} - {} [{}]",
            index + 1,
            row.pp,
            row.kept_pp,
            mods,
            row.played,
            status_column(label(row.status)),
            printable(&row.artist),
            printable(&row.title),
            printable(&row.version)
        ));
    }
    lines.join("\n")
}

pub fn printable(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

fn label(status: MapStatus) -> &'static str {
    match status {
        MapStatus::Ranked => "ranked",
        MapStatus::Approved => "approved",
        MapStatus::Qualified => "qualified",
        MapStatus::Loved => "loved",
        MapStatus::Pending => "pending",
        MapStatus::Wip => "wip",
        MapStatus::Graveyard => "graveyard",
        MapStatus::Unknown => "unknown",
    }
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
    use lost_scores_core::maps::{MapCatalog, MapStatus};
    use lost_scores_core::{LocalBeatmap, ScannedReplay};
    use osu_formats::{BeatmapMetadata, Mods, ReplayHeader};

    use super::{LostRow, date, rows, table};

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
                artist: "A".into(),
                title: "B".into(),
                version: "x".into(),
                file: "p.osr".into(),
            }]
        );
    }

    #[test]
    fn control_characters_never_reach_the_terminal() {
        let mut evil = replay("e.osr", 1, 300.0, 1_700_000_000);
        if let Some(beatmap) = evil.beatmap.as_mut() {
            beatmap.metadata.title = "Evil\u{1b}]52;c;ZXZpbA==\u{7}Title\u{9b}2J".into();
        }
        let kept = replay("k.osr", 2, 200.0, 1_700_000_000);
        let lost = [LostScore {
            play: &evil,
            kept: &kept,
            status: MapStatus::Ranked,
        }];
        let text = table(&rows(&lost, &MapCatalog::default()), 0, false);
        assert!(
            !text.chars().any(|c| c.is_control() && c != '\n'),
            "{text:?}"
        );
        assert!(
            text.contains("A - Evil]52;c;ZXZpbA==Title2J [x]"),
            "{text:?}"
        );
    }

    #[test]
    fn table_shows_at_most_the_limit() {
        let a = replay("a.osr", 1, 300.0, 1_700_000_000);
        let b = replay("b.osr", 2, 200.0, 1_700_000_000);
        let lost = [
            LostScore {
                play: &a,
                kept: &b,
                status: MapStatus::Ranked,
            },
            LostScore {
                play: &b,
                kept: &a,
                status: MapStatus::Loved,
            },
        ];
        let all = rows(&lost, &MapCatalog::default());
        let data_lines = |text: String| {
            text.lines()
                .filter(|line| line.contains("A - B [x]"))
                .count()
        };
        assert_eq!(data_lines(table(&all, 1, false)), 1);
        assert_eq!(data_lines(table(&all, 0, true)), 2);
    }
}
