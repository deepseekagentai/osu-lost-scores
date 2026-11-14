use lost_scores_app::rows::{LostRow, PotentialRow, printable};
use lost_scores_core::maps::MapStatus;

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

pub fn potential_table(rows: &[PotentialRow], limit: usize) -> String {
    let shown = if limit == 0 {
        rows.len()
    } else {
        limit.min(rows.len())
    };
    let mut lines = vec![format!(
        "{:>4}  {:>7}  {:>6}  {:>8}  {:<6}  {:>7}  {:<4}  beatmap",
        "#", "pp", "weight", "weighted", "mods", "acc", ""
    )];
    for row in rows.iter().take(shown) {
        let mods = if row.mods.is_empty() { "NM" } else { &row.mods };
        lines.push(format!(
            "{:>4}  {:>7.2}  {:>5.0}%  {:>8.2}  {:<6}  {:>6.2}%  {:<4}  {} - {} [{}]",
            row.place,
            row.pp,
            row.weight * 100.0,
            row.weighted,
            mods,
            row.accuracy,
            if row.lost { "LOST" } else { "" },
            printable(&row.artist),
            printable(&row.title),
            printable(&row.version)
        ));
    }
    lines.join("\n")
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

#[cfg(test)]
mod tests {
    use lost_scores_app::rows::{Hits, LostRow, PotentialRow};
    use lost_scores_core::grade::Grade;
    use lost_scores_core::maps::MapStatus;

    use super::{potential_table, table};

    fn lost_row(title: &str) -> LostRow {
        LostRow {
            pp: 300.0,
            kept_pp: 250.0,
            score: 1,
            kept_score: 2,
            mods: "HDDT".into(),
            unix_time: 1_700_000_000,
            played: "2023-11-14".into(),
            status: MapStatus::Ranked,
            beatmap_md5: "aaaa".into(),
            beatmap_id: Some(11),
            artist: "A".into(),
            title: title.into(),
            version: "x".into(),
            file: "p.osr".into(),
            creator: "m".into(),
            hits: Hits {
                n300: 100,
                n100: 0,
                n50: 0,
                misses: 0,
            },
            accuracy: 100.0,
            grade: Grade::X,
            max_combo: 100,
        }
    }

    fn potential_row(place: usize, lost: bool) -> PotentialRow {
        PotentialRow {
            place,
            pp: 400.0,
            weight: 1.0,
            weighted: 400.0,
            lost,
            beatmap_id: 1,
            artist: "A".into(),
            title: "B".into(),
            version: "x".into(),
            creator: "m".into(),
            mods: String::new(),
            accuracy: 98.0,
            played: "2023-11-14".into(),
            hits: Hits {
                n300: 100,
                n100: 0,
                n50: 0,
                misses: 0,
            },
            score: 1,
            grade: Grade::X,
            status: MapStatus::Ranked,
            kept_pp: None,
        }
    }

    #[test]
    fn control_characters_never_reach_the_terminal() {
        let rows = [lost_row("Evil\u{1b}]52;c;ZXZpbA==\u{7}Title\u{9b}2J")];
        let text = table(&rows, 0, false);
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
        let mut loved = lost_row("B");
        loved.status = MapStatus::Loved;
        let all = [lost_row("B"), loved];
        let data_lines = |text: String| {
            text.lines()
                .filter(|line| line.contains("A - B [x]"))
                .count()
        };
        assert_eq!(data_lines(table(&all, 1, false)), 1);
        assert_eq!(data_lines(table(&all, 0, true)), 2);
    }

    #[test]
    fn potential_table_marks_lost_rows() {
        let rows = [potential_row(1, false), potential_row(2, true)];
        let table = potential_table(&rows, 0);
        let lines: Vec<&str> = table.lines().collect();
        assert!(!lines[1].contains("LOST"));
        assert!(lines[2].contains("LOST"));
    }
}
