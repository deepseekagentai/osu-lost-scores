use std::cmp::Ordering;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use lost_scores_app::history::SavedScan;
use lost_scores_app::pipeline::PotentialResult;
use lost_scores_app::rows::{Hits, thousands};
use lost_scores_core::grade::Grade;
use lost_scores_core::maps::MapStatus;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Lost,
    Online,
    Potential,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Lost, Tab::Online, Tab::Potential];

    pub fn index(self) -> usize {
        match self {
            Tab::Lost => 0,
            Tab::Online => 1,
            Tab::Potential => 2,
        }
    }

    pub fn from_index(index: i32) -> Tab {
        match index {
            1 => Tab::Online,
            2 => Tab::Potential,
            _ => Tab::Lost,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Beatmap,
    Mods,
    Hits,
    Score,
    Accuracy,
    Pp,
}

impl Column {
    pub fn from_index(index: i32) -> Column {
        match index {
            0 => Column::Beatmap,
            1 => Column::Mods,
            2 => Column::Hits,
            3 => Column::Score,
            4 => Column::Accuracy,
            _ => Column::Pp,
        }
    }

    pub fn index(self) -> i32 {
        match self {
            Column::Beatmap => 0,
            Column::Mods => 1,
            Column::Hits => 2,
            Column::Score => 3,
            Column::Accuracy => 4,
            Column::Pp => 5,
        }
    }

    fn first_click_descending(self) -> bool {
        matches!(self, Column::Score | Column::Accuracy | Column::Pp)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Detail {
    Lost {
        kept_pp: f64,
    },
    Weighted {
        place: usize,
        weight: f64,
        weighted: f64,
        lost: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub grade: Grade,
    pub title: String,
    pub artist: String,
    pub version: String,
    pub creator: String,
    pub mods: String,
    pub hits: Hits,
    pub score: u64,
    pub accuracy: f64,
    pub pp: f64,
    pub unix_time: i64,
    pub date: String,
    pub status: Option<MapStatus>,
    pub beatmap_id: Option<u32>,
    pub detail: Detail,
    artist_key: String,
    title_key: String,
    haystack: String,
}

impl Item {
    #[allow(clippy::too_many_arguments)]
    fn new(
        grade: Grade,
        title: &str,
        artist: &str,
        version: &str,
        creator: &str,
        mods: &str,
        hits: Hits,
        score: u64,
        accuracy: f64,
        pp: f64,
        unix_time: i64,
        status: Option<MapStatus>,
        beatmap_id: Option<u32>,
        detail: Detail,
        zone: &TimeZone,
    ) -> Self {
        let date = when(unix_time, zone);
        let haystack = [title, artist, version, creator, mods, &date]
            .join("\n")
            .to_lowercase();
        Self {
            grade,
            title: title.to_owned(),
            artist: artist.to_owned(),
            version: version.to_owned(),
            creator: creator.to_owned(),
            mods: mods.to_owned(),
            hits,
            score,
            accuracy,
            pp,
            unix_time,
            date,
            status,
            beatmap_id,
            detail,
            artist_key: artist.to_lowercase(),
            title_key: title.to_lowercase(),
            haystack,
        }
    }

    pub fn mod_count(&self) -> usize {
        self.mods.len() / 2
    }

    fn compare(&self, other: &Item, column: Column) -> Ordering {
        match column {
            Column::Beatmap => self
                .artist_key
                .cmp(&other.artist_key)
                .then_with(|| self.title_key.cmp(&other.title_key)),
            Column::Mods => self
                .mod_count()
                .cmp(&other.mod_count())
                .then_with(|| self.mods.cmp(&other.mods)),
            Column::Hits => self
                .hits
                .misses
                .cmp(&other.hits.misses)
                .then_with(|| self.hits.n50.cmp(&other.hits.n50))
                .then_with(|| self.hits.n100.cmp(&other.hits.n100)),
            Column::Score => self.score.cmp(&other.score),
            Column::Accuracy => self.accuracy.total_cmp(&other.accuracy),
            Column::Pp => self.pp.total_cmp(&other.pp),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct TabRows {
    items: Vec<Item>,
    footer: Vec<(String, String)>,
    sort: Option<(Column, bool)>,
}

#[derive(Clone, Debug, Default)]
pub struct Table {
    tabs: [TabRows; 3],
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

pub fn when(unix: i64, zone: &TimeZone) -> String {
    let Ok(time) = Timestamp::from_second(unix) else {
        return String::new();
    };
    let local = time.to_zoned(zone.clone());
    format!(
        "{} {} {} · {:02}:{:02}",
        local.day(),
        MONTHS[local.month() as usize - 1],
        local.year(),
        local.hour(),
        local.minute()
    )
}

pub fn wrap(current: usize, total: usize, step: i32) -> usize {
    if total == 0 {
        return 0;
    }
    (current as i64 + i64::from(step)).rem_euclid(total as i64) as usize
}

impl Table {
    pub fn from_scan(scan: &SavedScan) -> Self {
        Self::in_zone(scan, &TimeZone::system())
    }

    pub fn in_zone(scan: &SavedScan, zone: &TimeZone) -> Self {
        let mut table = Table::default();
        table.tabs[Tab::Lost.index()] = lost_tab(scan, zone);
        if let Some(potential) = &scan.potential {
            table.tabs[Tab::Online.index()] = online_tab(scan, zone);
            table.tabs[Tab::Potential.index()] = potential_tab(zone, potential);
        }
        for tab in Tab::ALL {
            table.apply(tab, Column::Pp, true);
        }
        table
    }

    pub fn rows(&self, tab: Tab) -> &[Item] {
        &self.tabs[tab.index()].items
    }

    pub fn sorting(&self, tab: Tab) -> (Column, bool) {
        self.tabs[tab.index()].sort.unwrap_or((Column::Pp, true))
    }

    pub fn sort(&mut self, tab: Tab, column: Column) {
        let descending = match self.tabs[tab.index()].sort {
            Some((current, descending)) if current == column => !descending,
            _ => column.first_click_descending(),
        };
        self.apply(tab, column, descending);
    }

    fn apply(&mut self, tab: Tab, column: Column, descending: bool) {
        let rows = &mut self.tabs[tab.index()];
        rows.items.sort_by(|a, b| {
            let order = a.compare(b, column);
            if descending { order.reverse() } else { order }
        });
        rows.sort = Some((column, descending));
    }

    pub fn search(&self, tab: Tab, text: &str) -> Vec<usize> {
        let needle = text.trim().to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        self.rows(tab)
            .iter()
            .enumerate()
            .filter(|(_, item)| item.haystack.contains(&needle))
            .map(|(index, _)| index)
            .collect()
    }

    pub fn copy_text(&self, tab: Tab, index: usize) -> String {
        let Some(item) = self.rows(tab).get(index) else {
            return String::new();
        };
        let mods = if item.mods.is_empty() {
            "NM"
        } else {
            &item.mods
        };
        format!(
            "{} - {} [{}] +{} | {:.2}% | {:.2}pp | {}",
            item.artist,
            item.title,
            item.version,
            mods,
            item.accuracy,
            item.pp,
            item.date.replace(" · ", " ")
        )
    }

    pub fn footer(&self, tab: Tab) -> Vec<(String, String)> {
        self.tabs[tab.index()].footer.clone()
    }
}

fn pair(label: &str, value: String) -> (String, String) {
    (label.to_owned(), value)
}

fn lost_tab(scan: &SavedScan, zone: &TimeZone) -> TabRows {
    let items: Vec<Item> = scan
        .lost
        .iter()
        .map(|row| {
            Item::new(
                row.grade,
                &row.title,
                &row.artist,
                &row.version,
                &row.creator,
                &row.mods,
                row.hits,
                u64::from(row.score),
                row.accuracy,
                row.pp,
                row.unix_time,
                Some(row.status),
                row.beatmap_id,
                Detail::Lost {
                    kept_pp: row.kept_pp,
                },
                zone,
            )
        })
        .collect();
    let average = if scan.lost.is_empty() {
        0.0
    } else {
        scan.lost
            .iter()
            .map(|row| row.pp - row.kept_pp)
            .sum::<f64>()
            / scan.lost.len() as f64
    };
    let footer = vec![
        pair("Lost plays", thousands(scan.lost_total as f64)),
        pair("On ranked maps", thousands(scan.lost_ranked as f64)),
        pair("Average pp lost", format!("{average:.2}")),
    ];
    TabRows {
        items,
        footer,
        sort: None,
    }
}

fn online_tab(scan: &SavedScan, zone: &TimeZone) -> TabRows {
    let Some(potential) = &scan.potential else {
        return TabRows::default();
    };
    let items = potential
        .online
        .iter()
        .map(|row| {
            Item::new(
                row.grade,
                &row.title,
                &row.artist,
                &row.version,
                &row.creator,
                &row.mods,
                row.hits,
                row.score,
                row.accuracy,
                row.pp,
                row.unix_time,
                None,
                Some(row.beatmap_id),
                Detail::Weighted {
                    place: row.place,
                    weight: row.weight,
                    weighted: row.weighted,
                    lost: false,
                },
                zone,
            )
        })
        .collect();
    let rank = potential.global_rank.map_or_else(
        || "unranked".to_owned(),
        |rank| format!("#{}", thousands(f64::from(rank))),
    );
    let footer = vec![
        pair(
            "Performance",
            format!("{} pp", thousands(potential.current_pp)),
        ),
        pair("Accuracy", format!("{:.2}%", potential.current_accuracy)),
        pair("Global rank", rank),
    ];
    TabRows {
        items,
        footer,
        sort: None,
    }
}

fn potential_tab(zone: &TimeZone, potential: &PotentialResult) -> TabRows {
    let items = potential
        .rows
        .iter()
        .map(|row| {
            Item::new(
                row.grade,
                &row.title,
                &row.artist,
                &row.version,
                &row.creator,
                &row.mods,
                row.hits,
                row.score,
                row.accuracy,
                row.pp,
                row.unix_time,
                Some(row.status),
                Some(row.beatmap_id),
                Detail::Weighted {
                    place: row.place,
                    weight: row.weight,
                    weighted: row.weighted,
                    lost: row.lost,
                },
                zone,
            )
        })
        .collect();
    let gain = potential.potential_pp - potential.current_pp;
    let footer = vec![
        pair("Current", format!("{} pp", thousands(potential.current_pp))),
        pair(
            "Potential",
            format!("{} pp", thousands(potential.potential_pp)),
        ),
        pair("Gain", format!("{gain:+.2} pp")),
        pair(
            "Accuracy",
            format!(
                "{:.2}% → {:.2}%",
                potential.current_accuracy, potential.potential_accuracy
            ),
        ),
        pair("Lost plays in the top", potential.lost_in_top.to_string()),
    ];
    TabRows {
        items,
        footer,
        sort: None,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use jiff::tz::TimeZone;
    use lost_scores_app::history::{SavedScan, VERSION};
    use lost_scores_app::pipeline::PotentialResult;
    use lost_scores_app::rows::{Hits, LostRow, OnlineRow, PotentialRow};
    use lost_scores_core::grade::Grade;
    use lost_scores_core::maps::MapStatus;

    use super::{Column, Tab, Table, wrap};

    const SEPTEMBER_2024: i64 = 1_725_406_740;

    fn hits() -> Hits {
        Hits {
            n300: 400,
            n100: 10,
            n50: 1,
            misses: 0,
        }
    }

    fn lost(pp: f64, kept: f64, artist: &str, title: &str, mods: &str) -> LostRow {
        LostRow {
            pp,
            kept_pp: kept,
            score: 1_000_000,
            kept_score: 900_000,
            mods: mods.into(),
            unix_time: SEPTEMBER_2024,
            played: "2024-09-03".into(),
            status: MapStatus::Ranked,
            beatmap_md5: "0b577cbe9bc774600d20cfa17c0aa94c".into(),
            beatmap_id: Some(42),
            artist: artist.into(),
            title: title.into(),
            version: "Insane".into(),
            file: "a.osr".into(),
            creator: "Mapper".into(),
            hits: hits(),
            accuracy: 98.54,
            grade: Grade::S,
            max_combo: 500,
        }
    }

    fn scan(rows: Vec<LostRow>, potential: Option<PotentialResult>) -> SavedScan {
        SavedScan {
            version: VERSION,
            made_at: SEPTEMBER_2024,
            player_id: 1,
            player_name: "Player".into(),
            include_all: false,
            check_missing: false,
            lost_total: rows.len(),
            lost_ranked: rows.len() - 1,
            lost: rows,
            potential,
        }
    }

    fn table(rows: Vec<LostRow>) -> Table {
        Table::in_zone(&scan(rows, None), &TimeZone::UTC)
    }

    fn pps(table: &Table) -> Vec<f64> {
        table.rows(Tab::Lost).iter().map(|item| item.pp).collect()
    }

    #[test]
    fn pp_is_sorted_highest_first_by_default() {
        let table = table(vec![
            lost(100.0, 90.0, "A", "x", ""),
            lost(300.0, 90.0, "B", "y", ""),
            lost(200.0, 90.0, "C", "z", ""),
        ]);
        assert_eq!(pps(&table), [300.0, 200.0, 100.0]);
        assert_eq!(table.sorting(Tab::Lost), (Column::Pp, true));
    }

    #[test]
    fn a_second_click_reverses_the_order() {
        let mut table = table(vec![
            lost(100.0, 90.0, "A", "x", ""),
            lost(300.0, 90.0, "B", "y", ""),
            lost(200.0, 90.0, "C", "z", ""),
        ]);
        table.sort(Tab::Lost, Column::Pp);
        assert_eq!(pps(&table), [100.0, 200.0, 300.0]);
        table.sort(Tab::Lost, Column::Pp);
        assert_eq!(pps(&table), [300.0, 200.0, 100.0]);
    }

    #[test]
    fn ties_keep_the_previous_order() {
        let mut table = table(vec![
            lost(100.0, 90.0, "Same", "first", ""),
            lost(300.0, 90.0, "Same", "second", ""),
            lost(200.0, 90.0, "Other", "third", ""),
        ]);
        table.sort(Tab::Lost, Column::Beatmap);
        let order: Vec<&str> = table
            .rows(Tab::Lost)
            .iter()
            .map(|item| item.artist.as_str())
            .collect();
        assert_eq!(order, ["Other", "Same", "Same"]);
        table.sort(Tab::Lost, Column::Accuracy);
        let titles: Vec<&str> = table
            .rows(Tab::Lost)
            .iter()
            .map(|item| item.title.as_str())
            .collect();
        assert_eq!(titles, ["third", "first", "second"]);
    }

    #[test]
    fn mods_sort_by_count() {
        let mut table = table(vec![
            lost(1.0, 0.0, "A", "a", "HDDT"),
            lost(2.0, 0.0, "A", "b", ""),
            lost(3.0, 0.0, "A", "c", "HD"),
            lost(4.0, 0.0, "A", "d", "DT"),
        ]);
        table.sort(Tab::Lost, Column::Mods);
        let mods: Vec<&str> = table
            .rows(Tab::Lost)
            .iter()
            .map(|item| item.mods.as_str())
            .collect();
        assert_eq!(mods, ["", "DT", "HD", "HDDT"]);
    }

    #[test]
    fn hits_sort_by_misses_then_fifties_then_hundreds() {
        let mut few = lost(1.0, 0.0, "A", "few", "");
        few.hits.misses = 1;
        let mut many = lost(2.0, 0.0, "A", "many", "");
        many.hits.misses = 3;
        let mut clean = lost(3.0, 0.0, "A", "clean", "");
        clean.hits.misses = 0;
        let mut table = table(vec![many, clean, few]);
        table.sort(Tab::Lost, Column::Hits);
        let titles: Vec<&str> = table
            .rows(Tab::Lost)
            .iter()
            .map(|item| item.title.as_str())
            .collect();
        assert_eq!(titles, ["clean", "few", "many"]);
    }

    #[test]
    fn search_ignores_case_and_wraps() {
        let table = table(vec![
            lost(100.0, 90.0, "Camellia", "Alpha", ""),
            lost(300.0, 90.0, "Other", "Beta", "HDDT"),
            lost(200.0, 90.0, "camellia", "Gamma", ""),
        ]);
        assert_eq!(table.search(Tab::Lost, "CAMELLIA"), [1, 2]);
        assert_eq!(table.search(Tab::Lost, "hddt"), [0]);
        assert_eq!(table.search(Tab::Lost, "3 september 2024"), [0, 1, 2]);
        assert!(table.search(Tab::Lost, "nothing").is_empty());
        assert!(table.search(Tab::Lost, "  ").is_empty());
        assert_eq!(wrap(1, 2, 1), 0);
        assert_eq!(wrap(0, 2, -1), 1);
        assert_eq!(wrap(0, 0, 1), 0);
    }

    #[test]
    fn copy_text_has_title_mods_accuracy_pp_and_date() {
        let table = table(vec![lost(411.87, 400.0, "Artist", "Title", "HDDT")]);
        assert_eq!(
            table.copy_text(Tab::Lost, 0),
            "Artist - Title [Insane] +HDDT | 98.54% | 411.87pp | 3 September 2024 23:39"
        );
        assert_eq!(table.copy_text(Tab::Lost, 5), "");
    }

    fn online(place: usize, pp: f64) -> OnlineRow {
        OnlineRow {
            place,
            pp,
            weight: 0.95_f64.powi(place as i32 - 1),
            weighted: pp * 0.95_f64.powi(place as i32 - 1),
            beatmap_id: 7,
            artist: "A".into(),
            title: "T".into(),
            version: "V".into(),
            creator: "C".into(),
            mods: "HD".into(),
            accuracy: 99.0,
            grade: Grade::S,
            hits: hits(),
            score: 2_000_000,
            unix_time: SEPTEMBER_2024,
            played: "2024-09-03".into(),
        }
    }

    fn weighted(place: usize, pp: f64, lost: bool) -> PotentialRow {
        PotentialRow {
            place,
            pp,
            weight: 0.95_f64.powi(place as i32 - 1),
            weighted: pp * 0.95_f64.powi(place as i32 - 1),
            lost,
            beatmap_id: 7,
            artist: "A".into(),
            title: "T".into(),
            version: "V".into(),
            creator: "C".into(),
            mods: "".into(),
            accuracy: 99.0,
            unix_time: SEPTEMBER_2024,
            played: "2024-09-03".into(),
            hits: hits(),
            score: 2_000_000,
            grade: Grade::A,
            status: MapStatus::Loved,
            kept_pp: lost.then_some(10.0),
        }
    }

    #[test]
    fn footers_match_the_saved_scan() {
        let potential = PotentialResult {
            current_pp: 8557.4,
            potential_pp: 8668.1,
            current_accuracy: 98.38,
            potential_accuracy: 97.95,
            global_rank: Some(14_856),
            lost_in_top: 2,
            rows: vec![weighted(1, 400.0, true), weighted(2, 380.0, false)],
            online: vec![online(1, 380.0)],
        };
        let table = Table::in_zone(
            &scan(
                vec![
                    lost(300.0, 280.0, "A", "x", ""),
                    lost(200.0, 190.0, "B", "y", ""),
                    lost(100.0, 99.0, "C", "z", ""),
                ],
                Some(potential),
            ),
            &TimeZone::UTC,
        );
        assert_eq!(
            table.footer(Tab::Lost),
            [
                ("Lost plays".to_owned(), "3".to_owned()),
                ("On ranked maps".to_owned(), "2".to_owned()),
                ("Average pp lost".to_owned(), "10.33".to_owned()),
            ]
        );
        assert_eq!(
            table.footer(Tab::Online),
            [
                ("Performance".to_owned(), "8,557 pp".to_owned()),
                ("Accuracy".to_owned(), "98.38%".to_owned()),
                ("Global rank".to_owned(), "#14,856".to_owned()),
            ]
        );
        assert_eq!(
            table.footer(Tab::Potential),
            [
                ("Current".to_owned(), "8,557 pp".to_owned()),
                ("Potential".to_owned(), "8,668 pp".to_owned()),
                ("Gain".to_owned(), "+110.70 pp".to_owned()),
                ("Accuracy".to_owned(), "98.38% → 97.95%".to_owned()),
                ("Lost plays in the top".to_owned(), "2".to_owned()),
            ]
        );
        assert_eq!(table.rows(Tab::Potential).len(), 2);
        assert_eq!(table.rows(Tab::Online).len(), 1);
    }

    #[test]
    #[ignore = "timing check, run with --release -- --ignored"]
    fn sorting_and_search_on_20000_rows_take_under_50_ms() {
        let rows: Vec<LostRow> = (0..20_000)
            .map(|i| {
                lost(
                    f64::from((i * 7919) % 1000),
                    0.0,
                    &format!("Artist {}", i % 977),
                    &format!("Title {i}"),
                    if i % 3 == 0 { "HDDT" } else { "" },
                )
            })
            .collect();
        let mut table = Table::in_zone(&scan(rows, None), &TimeZone::UTC);
        for column in [
            Column::Beatmap,
            Column::Mods,
            Column::Hits,
            Column::Score,
            Column::Accuracy,
            Column::Pp,
        ] {
            let one = Instant::now();
            table.sort(Tab::Lost, column);
            assert!(one.elapsed().as_millis() < 50, "{column:?}");
        }
        let one = Instant::now();
        let found = table.search(Tab::Lost, "title 19");
        assert!(one.elapsed().as_millis() < 50);
        assert!(!found.is_empty());
    }
}
