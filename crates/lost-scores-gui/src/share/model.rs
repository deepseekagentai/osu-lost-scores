use jiff::tz::TimeZone;
use lost_scores_app::ago::ago;
use lost_scores_app::history::SavedScan;
use lost_scores_app::pipeline::PotentialResult;
use lost_scores_app::rows::{mod_acronyms, thousands};
use lost_scores_core::grade::Grade;
use lost_scores_core::maps::MapStatus;

use crate::share::rasters::CoverKey;
use crate::table::when_short;
use crate::view::{lost_text, weighted_text};

pub const MAX_IMAGE_ROWS: usize = 250;
pub const MOD_ICON_WIDTH: f32 = 30.0;
pub const MOD_ICON_GAP: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShareOptions {
    pub count: usize,
    pub include_all: bool,
    pub show_lost_in_top: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShareRow {
    pub place: String,
    pub cover: Option<CoverKey>,
    pub grade: Grade,
    pub title: String,
    pub artist: String,
    pub version: String,
    pub creator: String,
    pub ago: String,
    pub mods: Vec<String>,
    pub accuracy: String,
    pub pp: String,
    pub sub: String,
    pub sub_red: String,
    pub lost: bool,
    pub status: Option<MapStatus>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stats {
    pub pp_from: String,
    pub pp_to: String,
    pub pp_delta: String,
    pub accuracy_from: String,
    pub accuracy_to: String,
    pub accuracy_delta: String,
    pub accuracy_rises: bool,
    pub lost_in_top: String,
}

fn since(scan: &SavedScan, unix_time: i64) -> String {
    ago((scan.made_at - unix_time) as f64)
}

fn ranked_or_all(scan: &SavedScan, options: &ShareOptions) -> Vec<usize> {
    let mut picked: Vec<usize> = (0..scan.lost.len())
        .filter(|&index| options.include_all || scan.lost[index].status.gives_pp())
        .collect();
    picked.sort_by(|&a, &b| scan.lost[b].pp.total_cmp(&scan.lost[a].pp));
    picked
}

pub fn cover_keys(scan: &SavedScan, options: &ShareOptions) -> Vec<CoverKey> {
    let mut keys: Vec<CoverKey> = lost_rows(scan, options)
        .into_iter()
        .chain(potential_rows(scan, options))
        .filter_map(|row| row.cover)
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

pub fn lost_count(scan: &SavedScan, options: &ShareOptions) -> usize {
    ranked_or_all(scan, options).len()
}

pub fn lost_rows(scan: &SavedScan, options: &ShareOptions) -> Vec<ShareRow> {
    ranked_or_all(scan, options)
        .into_iter()
        .take(options.count.min(MAX_IMAGE_ROWS))
        .enumerate()
        .map(|(position, index)| {
            let row = &scan.lost[index];
            ShareRow {
                place: format!("#{}", position + 1),
                cover: CoverKey::of(row.beatmapset_id, &row.beatmap_md5),
                grade: row.grade,
                title: row.title.clone(),
                artist: row.artist.clone(),
                version: row.version.clone(),
                creator: row.creator.clone(),
                ago: since(scan, row.unix_time),
                mods: mod_acronyms(&row.mods),
                accuracy: format!("{:.2}%", row.accuracy),
                pp: format!("{:.0}", row.pp),
                sub: String::new(),
                sub_red: lost_text(row.pp, row.counted()),
                lost: false,
                status: Some(row.status),
            }
        })
        .collect()
}

pub fn potential_rows(scan: &SavedScan, options: &ShareOptions) -> Vec<ShareRow> {
    let Some(potential) = &scan.potential else {
        return Vec::new();
    };
    let first_lost = potential.rows.iter().position(|row| row.lost);
    let shown = match first_lost {
        Some(index) if options.show_lost_in_top => options.count.max(index + 1),
        _ => options.count,
    }
    .min(MAX_IMAGE_ROWS);
    potential
        .rows
        .iter()
        .take(shown)
        .map(|row| ShareRow {
            place: format!("#{}", row.place),
            cover: row.beatmapset_id.map(CoverKey::Set),
            grade: row.grade,
            title: row.title.clone(),
            artist: row.artist.clone(),
            version: row.version.clone(),
            creator: row.creator.clone(),
            ago: since(scan, row.unix_time),
            mods: mod_acronyms(&row.mods),
            accuracy: format!("{:.2}%", row.accuracy),
            pp: format!("{:.0}", row.pp),
            sub: weighted_text(row.weight, row.weighted),
            sub_red: String::new(),
            lost: row.lost,
            status: None,
        })
        .collect()
}

pub fn mods_width(rows: &[ShareRow]) -> f32 {
    let longest = rows.iter().map(|row| row.mods.len()).max().unwrap_or(0);
    if longest == 0 {
        return 0.0;
    }
    longest as f32 * MOD_ICON_WIDTH + (longest - 1) as f32 * MOD_ICON_GAP
}

pub fn stats(potential: &PotentialResult) -> Stats {
    Stats {
        pp_from: thousands(potential.current_pp),
        pp_to: format!("{}pp", thousands(potential.potential_pp)),
        pp_delta: format!("({:+.1})", potential.potential_pp - potential.current_pp),
        accuracy_from: format!("{:.2}%", potential.current_accuracy),
        accuracy_to: format!("{:.2}%", potential.potential_accuracy),
        accuracy_delta: format!(
            "({:+.2}%)",
            potential.potential_accuracy - potential.current_accuracy
        ),
        accuracy_rises: potential.potential_accuracy >= potential.current_accuracy,
        lost_in_top: potential.lost_in_top.to_string(),
    }
}

pub fn subtitle(scan: &SavedScan, options: &ShareOptions) -> String {
    format!(
        "peppy scammed me for {} of them!",
        lost_count(scan, options)
    )
}

const NO_LOST: &str = "No lost plays found";

pub fn lost_empty_text(scan: &SavedScan) -> String {
    scan.zero_reason().unwrap_or_else(|| NO_LOST.to_owned())
}

pub fn badge_lost_line(scan: &SavedScan, options: &ShareOptions) -> String {
    let ranked = scan.lost.iter().filter(|row| row.status.gives_pp()).count();
    let plural = |count: usize| if count == 1 { "play" } else { "plays" };
    if scan.lost.is_empty() && scan.before_cutoff == Some(0) {
        return "No replays before 9 Oct 2024".to_owned();
    }
    let outside_top = scan
        .potential
        .as_ref()
        .is_some_and(|top| top.lost_in_top == 0);
    if !options.include_all && ranked > 0 && outside_top {
        return format!("{ranked} lost {}, none in your top 200", plural(ranked));
    }
    if options.include_all {
        format!(
            "{} lost {}, {} ranked",
            scan.lost.len(),
            plural(scan.lost.len()),
            ranked
        )
    } else {
        format!("{} lost {} on ranked maps", ranked, plural(ranked))
    }
}

pub fn badge_when_line(scan: &SavedScan, zone: &TimeZone) -> String {
    let day = when_short(scan.made_at, zone);
    match scan.potential.as_ref().and_then(|p| p.global_rank) {
        Some(rank) => format!("#{} · {day}", thousands(f64::from(rank))),
        None => day,
    }
}

#[cfg(test)]
mod tests {
    use jiff::tz::TimeZone;
    use lost_scores_app::history::{SavedScan, VERSION};
    use lost_scores_app::pipeline::PotentialResult;
    use lost_scores_app::rows::{Hits, LostRow, PotentialRow};
    use lost_scores_core::grade::Grade;
    use lost_scores_core::maps::MapStatus;

    use super::{
        MAX_IMAGE_ROWS, ShareOptions, badge_lost_line, badge_when_line, lost_count,
        lost_empty_text, lost_rows, mods_width, potential_rows, stats, subtitle,
    };

    const MADE_AT: i64 = 1_794_649_471;

    fn hits() -> Hits {
        Hits {
            n300: 500,
            n100: 5,
            n50: 0,
            misses: 1,
        }
    }

    fn lost(pp: f64, status: MapStatus, mods: &str) -> LostRow {
        LostRow {
            pp,
            kept_pp: pp - 10.0,
            counted_pp: None,
            score: 1_000_000,
            kept_score: 900_000,
            mods: mods.into(),
            unix_time: MADE_AT - 3 * 86_400,
            played: "2026-11-11".into(),
            status,
            beatmap_md5: "ab".into(),
            beatmap_id: Some(1),
            beatmapset_id: Some(7),
            artist: "Artist".into(),
            title: format!("Title {pp}"),
            version: "Insane".into(),
            file: "a.osr".into(),
            creator: "Mapper".into(),
            hits: hits(),
            accuracy: 98.5,
            grade: Grade::S,
            max_combo: 300,
        }
    }

    fn weighted(place: usize, lost: bool, mods: &str) -> PotentialRow {
        PotentialRow {
            place,
            pp: 500.0 - place as f64,
            weight: 0.95_f64.powi(place as i32 - 1),
            weighted: 400.0,
            lost,
            beatmap_id: place as u32,
            beatmapset_id: Some(place as u32),
            artist: "Artist".into(),
            title: format!("Top {place}"),
            version: "Expert".into(),
            creator: "Mapper".into(),
            mods: mods.into(),
            accuracy: 99.0,
            unix_time: MADE_AT - 86_400,
            played: "2026-11-13".into(),
            hits: hits(),
            score: 1_000_000,
            grade: Grade::A,
            status: MapStatus::Ranked,
            kept_pp: None,
        }
    }

    fn potential(rows: Vec<PotentialRow>, rank: Option<u32>) -> PotentialResult {
        PotentialResult {
            current_pp: 5_000.4,
            potential_pp: 5_012.9,
            current_accuracy: 98.1,
            potential_accuracy: 98.07,
            global_rank: rank,
            lost_in_top: 3,
            rows,
            online: Vec::new(),
        }
    }

    fn scan(lost: Vec<LostRow>, potential: Option<PotentialResult>) -> SavedScan {
        SavedScan {
            version: VERSION,
            made_at: MADE_AT,
            player_id: 1,
            player_name: "Tester".into(),
            include_all: false,
            check_missing: false,
            lost_total: lost.len(),
            lost_ranked: lost.iter().filter(|row| row.status.gives_pp()).count(),
            before_cutoff: None,
            lost,
            potential,
        }
    }

    fn options(count: usize, include_all: bool, show_lost_in_top: bool) -> ShareOptions {
        ShareOptions {
            count,
            include_all,
            show_lost_in_top,
        }
    }

    #[test]
    fn the_lost_image_measures_the_loss_against_what_osu_counts_now() {
        let mut row = lost(356.0, MapStatus::Ranked, "HD");
        row.kept_pp = 240.0;
        row.counted_pp = Some(320.0);
        let rows = lost_rows(&scan(vec![row], None), &options(10, false, false));
        assert_eq!(rows[0].sub_red, "-36.0pp");
    }

    #[test]
    fn lost_image_takes_ranked_plays_unless_include() {
        let scan = scan(
            vec![
                lost(100.0, MapStatus::Ranked, ""),
                lost(300.0, MapStatus::Loved, "HD"),
                lost(200.0, MapStatus::Ranked, "DT"),
                lost(250.0, MapStatus::Graveyard, ""),
            ],
            None,
        );
        let ranked = lost_rows(&scan, &options(10, false, false));
        let titles: Vec<&str> = ranked.iter().map(|row| row.title.as_str()).collect();
        assert_eq!(titles, ["Title 200", "Title 100"]);
        let places: Vec<&str> = ranked.iter().map(|row| row.place.as_str()).collect();
        assert_eq!(places, ["#1", "#2"]);
        assert!(ranked.iter().all(|row| !row.lost));
        assert_eq!(
            subtitle(&scan, &options(10, false, false)),
            "peppy scammed me for 2 of them!"
        );

        let all = lost_rows(&scan, &options(10, true, false));
        let titles: Vec<&str> = all.iter().map(|row| row.title.as_str()).collect();
        assert_eq!(titles, ["Title 300", "Title 250", "Title 200", "Title 100"]);
        assert_eq!(all[0].status, Some(MapStatus::Loved));
        assert_eq!(
            subtitle(&scan, &options(10, true, false)),
            "peppy scammed me for 4 of them!"
        );
    }

    #[test]
    fn the_image_never_draws_more_than_the_row_cap() {
        let rows = (0..400)
            .map(|n| lost(1000.0 - f64::from(n), MapStatus::Ranked, ""))
            .collect();
        let many = scan(rows, None);
        let shown = lost_rows(&many, &options(999, false, false));
        assert_eq!(shown.len(), MAX_IMAGE_ROWS);
        assert_eq!(shown.len(), 250);
        assert_eq!(shown[249].place, "#250");

        let tall = (1..=300)
            .map(|place| weighted(place, place == 280, ""))
            .collect();
        let top = scan(Vec::new(), Some(potential(tall, Some(1))));
        assert_eq!(
            potential_rows(&top, &options(999, false, true)).len(),
            MAX_IMAGE_ROWS
        );
    }

    #[test]
    fn the_lost_image_keeps_the_top_n_and_counts_them_all() {
        let rows = (0..8)
            .map(|n| lost(100.0 + f64::from(n), MapStatus::Ranked, ""))
            .collect();
        let scan = scan(rows, None);
        let shown = lost_rows(&scan, &options(3, false, false));
        assert_eq!(shown.len(), 3);
        assert_eq!(shown[0].pp, "107");
        assert_eq!(lost_count(&scan, &options(3, false, false)), 8);
        assert_eq!(shown[0].sub_red, "-10.0pp");
        assert_eq!(shown[0].ago, "3 days ago");
    }

    #[test]
    fn potential_image_extends_to_the_first_lost_play() {
        let rows = |first_lost: Option<usize>| {
            (1..=30)
                .map(|place| weighted(place, Some(place) == first_lost, ""))
                .collect::<Vec<_>>()
        };
        let far = scan(Vec::new(), Some(potential(rows(Some(14)), Some(1))));
        assert_eq!(potential_rows(&far, &options(10, false, true)).len(), 14);
        assert_eq!(potential_rows(&far, &options(10, false, false)).len(), 10);
        let shown = potential_rows(&far, &options(10, false, true));
        assert!(shown[13].lost && !shown[12].lost);
        assert_eq!(shown[13].place, "#14");

        let near = scan(Vec::new(), Some(potential(rows(Some(4)), Some(1))));
        assert_eq!(potential_rows(&near, &options(10, false, true)).len(), 10);

        let none = scan(Vec::new(), Some(potential(rows(None), Some(1))));
        assert_eq!(potential_rows(&none, &options(10, false, true)).len(), 10);

        let short = scan(Vec::new(), Some(potential(rows(None)[..4].to_vec(), None)));
        assert_eq!(potential_rows(&short, &options(10, false, true)).len(), 4);

        let missing = scan(Vec::new(), None);
        assert!(potential_rows(&missing, &options(10, false, true)).is_empty());
    }

    #[test]
    fn the_mod_column_is_as_wide_as_the_longest_list() {
        let scan = scan(
            vec![
                lost(300.0, MapStatus::Ranked, "HD"),
                lost(200.0, MapStatus::Ranked, "HDDTHR"),
                lost(100.0, MapStatus::Ranked, ""),
            ],
            None,
        );
        let rows = lost_rows(&scan, &options(10, false, false));
        assert_eq!(mods_width(&rows), 3.0 * 30.0 + 2.0 * 2.0);
        assert_eq!(mods_width(&rows[..1]), 30.0);
        assert_eq!(mods_width(&[]), 0.0);
    }

    #[test]
    fn an_empty_picture_and_the_badge_say_why_nothing_was_lost() {
        let mut scan = scan(Vec::new(), None);
        assert_eq!(lost_empty_text(&scan), "No lost plays found");
        scan.before_cutoff = Some(0);
        assert_eq!(
            lost_empty_text(&scan),
            "No replays from before 9 October 2024, when osu! stopped deleting beaten scores"
        );
        assert_eq!(
            badge_lost_line(&scan, &options(10, false, false)),
            "No replays before 9 Oct 2024"
        );
        scan.before_cutoff = Some(812);
        assert_eq!(
            lost_empty_text(&scan),
            "No lost plays among your 812 replays from before 9 October 2024"
        );
        assert_eq!(
            badge_lost_line(&scan, &options(10, false, false)),
            "0 lost plays on ranked maps"
        );
    }

    #[test]
    fn lost_plays_outside_the_top_show_on_the_badge() {
        let mut top = potential(Vec::new(), None);
        top.lost_in_top = 0;
        let scan = scan(
            vec![
                lost(300.0, MapStatus::Ranked, ""),
                lost(280.0, MapStatus::Ranked, "HD"),
            ],
            Some(top),
        );
        assert_eq!(
            badge_lost_line(&scan, &options(10, false, false)),
            "2 lost plays, none in your top 200"
        );
    }

    #[test]
    fn no_lost_plays_still_renders() {
        let scan = scan(Vec::new(), None);
        assert!(lost_rows(&scan, &options(10, true, false)).is_empty());
        assert_eq!(lost_count(&scan, &options(10, false, false)), 0);
        assert_eq!(
            subtitle(&scan, &options(10, false, false)),
            "peppy scammed me for 0 of them!"
        );
        assert_eq!(mods_width(&[]), 0.0);
        assert_eq!(
            badge_lost_line(&scan, &options(10, false, false)),
            "0 lost plays on ranked maps"
        );
    }

    #[test]
    fn nm_has_no_icon() {
        let scan = scan(
            vec![
                lost(300.0, MapStatus::Ranked, "NM"),
                lost(200.0, MapStatus::Ranked, ""),
            ],
            None,
        );
        let rows = lost_rows(&scan, &options(10, false, false));
        assert!(rows.iter().all(|row| row.mods.is_empty()));
        assert_eq!(mods_width(&rows), 0.0);
    }

    #[test]
    fn the_accuracy_colour_follows_the_direction() {
        let mut result = potential(Vec::new(), None);
        result.current_accuracy = 98.1;
        result.potential_accuracy = 98.07;
        assert!(!stats(&result).accuracy_rises);
        result.potential_accuracy = 98.3;
        assert!(stats(&result).accuracy_rises);
        assert_eq!(stats(&result).accuracy_delta, "(+0.20%)");
    }

    #[test]
    fn the_stats_card_reads_from_to_with_the_change() {
        let result = potential(Vec::new(), None);
        let card = stats(&result);
        assert_eq!(card.pp_from, "5,000");
        assert_eq!(card.pp_to, "5,013pp");
        assert_eq!(card.pp_delta, "(+12.5)");
        assert_eq!(card.accuracy_from, "98.10%");
        assert_eq!(card.accuracy_to, "98.07%");
        assert_eq!(card.accuracy_delta, "(-0.03%)");
        assert_eq!(card.lost_in_top, "3");
    }

    #[test]
    fn the_badge_lines_follow_include_and_the_rank() {
        let plain = scan(
            vec![
                lost(300.0, MapStatus::Ranked, ""),
                lost(200.0, MapStatus::Loved, ""),
                lost(100.0, MapStatus::Ranked, ""),
            ],
            Some(potential(Vec::new(), Some(1234))),
        );
        assert_eq!(
            badge_lost_line(&plain, &options(10, false, false)),
            "2 lost plays on ranked maps"
        );
        assert_eq!(
            badge_lost_line(&plain, &options(10, true, false)),
            "3 lost plays, 2 ranked"
        );
        assert_eq!(
            badge_when_line(&plain, &TimeZone::UTC),
            "#1,234 · 14 Nov 2026"
        );
        let unranked = scan(Vec::new(), Some(potential(Vec::new(), None)));
        assert_eq!(badge_when_line(&unranked, &TimeZone::UTC), "14 Nov 2026");
    }
}
