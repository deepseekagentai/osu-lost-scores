use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

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
    pub date_short: String,
    pub status: Option<MapStatus>,
    pub beatmap_id: Option<u32>,
    pub detail: Detail,
    pub fresh: bool,
    artist_key: String,
    title_key: String,
    version_key: String,
    mod_key: (usize, Vec<(u8, String)>),
    haystack: String,
}

const MOD_ORDER: [&str; 12] = [
    "NF", "EZ", "TD", "HD", "HR", "SD", "DT", "HT", "NC", "FL", "SO", "PF",
];

pub fn mod_acronyms(mods: &str) -> Vec<&str> {
    let mut list = Vec::new();
    let mut rest = mods;
    while !rest.is_empty() {
        let end = rest
            .char_indices()
            .nth(2)
            .map_or(rest.len(), |(index, _)| index);
        list.push(&rest[..end]);
        rest = &rest[end..];
    }
    list
}

fn mod_key(mods: &str) -> (usize, Vec<(u8, String)>) {
    let mut keys: Vec<(u8, String)> = mod_acronyms(mods)
        .into_iter()
        .map(|acronym| {
            let rank = MOD_ORDER
                .iter()
                .position(|known| *known == acronym)
                .map_or(u8::MAX, |position| position as u8);
            (rank, acronym.to_owned())
        })
        .collect();
    keys.sort();
    (keys.len(), keys)
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
        let date_short = when_short(unix_time, zone);
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
            date_short,
            status,
            beatmap_id,
            detail,
            fresh: false,
            artist_key: artist.to_lowercase(),
            title_key: title.to_lowercase(),
            version_key: version.to_lowercase(),
            mod_key: mod_key(mods),
            haystack,
        }
    }

    fn compare(&self, other: &Item, column: Column, descending: bool) -> Ordering {
        let order = match column {
            Column::Beatmap => self
                .title_key
                .cmp(&other.title_key)
                .then_with(|| self.artist_key.cmp(&other.artist_key))
                .then_with(|| self.version_key.cmp(&other.version_key)),
            Column::Mods => {
                let groups = self.mod_key.cmp(&other.mod_key);
                let groups = if descending { groups.reverse() } else { groups };
                return groups.then_with(|| other.pp.total_cmp(&self.pp));
            }
            Column::Hits => self
                .hits
                .misses
                .cmp(&other.hits.misses)
                .then_with(|| self.hits.n50.cmp(&other.hits.n50))
                .then_with(|| self.hits.n100.cmp(&other.hits.n100)),
            Column::Score => self.score.cmp(&other.score),
            Column::Accuracy => self.accuracy.total_cmp(&other.accuracy),
            Column::Pp => self.pp.total_cmp(&other.pp),
        };
        if descending { order.reverse() } else { order }
    }
}

#[derive(Clone, Debug, Default)]
struct TabRows {
    items: Vec<Item>,
    footer: Vec<(String, String)>,
    tones: Vec<i32>,
    sort: Option<(Column, bool)>,
}

#[derive(Clone, Debug, Default)]
pub struct Table {
    tabs: [TabRows; 3],
    lost_pool: Vec<Item>,
    include_all: bool,
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

pub fn when_short(unix: i64, zone: &TimeZone) -> String {
    let Ok(time) = Timestamp::from_second(unix) else {
        return String::new();
    };
    let local = time.to_zoned(zone.clone());
    format!(
        "{} {} {}",
        local.day(),
        &MONTHS[local.month() as usize - 1][..3],
        local.year()
    )
}

type RowKey<'a> = (i64, Option<u32>, u64, &'a str);

fn row_key(item: &Item) -> RowKey<'_> {
    (item.unix_time, item.beatmap_id, item.score, &item.title)
}

pub fn anchor_index(before: &[Item], after: &[Item], first_visible: usize) -> Option<usize> {
    let positions: HashMap<RowKey, usize> = after
        .iter()
        .enumerate()
        .map(|(index, item)| (row_key(item), index))
        .collect();
    let start = first_visible.min(before.len());
    before[start..]
        .iter()
        .chain(before[..start].iter().rev())
        .find_map(|item| positions.get(&row_key(item)).copied())
}

pub fn tone(now: f64, potential: f64, decimals: i32) -> i32 {
    let scale = 10f64.powi(decimals);
    let now = (now * scale).round();
    let potential = (potential * scale).round();
    match potential.total_cmp(&now) {
        Ordering::Greater => 1,
        Ordering::Less => -1,
        Ordering::Equal => 0,
    }
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
        let mut table = Table {
            lost_pool: lost_items(scan, zone),
            include_all: true,
            ..Table::default()
        };
        table.rebuild_lost();
        if let Some(potential) = &scan.potential {
            table.tabs[Tab::Online.index()] = online_tab(scan, zone);
            table.tabs[Tab::Potential.index()] = potential_tab(zone, potential);
        }
        for tab in Tab::ALL {
            table.apply(tab, Column::Pp, true);
        }
        table
    }

    pub fn set_include_all(&mut self, include_all: bool) {
        if self.include_all != include_all {
            self.include_all = include_all;
            self.rebuild_lost();
        }
    }

    fn rebuild_lost(&mut self) {
        let mut items: Vec<Item> = self
            .lost_pool
            .iter()
            .filter(|item| self.include_all || item.status.is_some_and(MapStatus::gives_pp))
            .cloned()
            .collect();
        let previous = &self.tabs[Tab::Lost.index()].items;
        if !previous.is_empty() {
            let known: HashSet<RowKey> = previous.iter().map(row_key).collect();
            for item in &mut items {
                item.fresh = !known.contains(&row_key(item));
            }
        }
        let footer = lost_footer(&items);
        let sort = self.tabs[Tab::Lost.index()].sort;
        self.tabs[Tab::Lost.index()] = TabRows {
            items,
            footer,
            tones: Vec::new(),
            sort: None,
        };
        let (column, descending) = sort.unwrap_or((Column::Pp, true));
        self.apply(Tab::Lost, column, descending);
    }

    pub fn rows(&self, tab: Tab) -> &[Item] {
        &self.tabs[tab.index()].items
    }

    pub fn sorting(&self, tab: Tab) -> (Column, bool) {
        self.tabs[tab.index()].sort.unwrap_or((Column::Pp, true))
    }

    pub fn clear_fresh(&mut self) {
        for item in &mut self.tabs[Tab::Lost.index()].items {
            item.fresh = false;
        }
        for item in &mut self.lost_pool {
            item.fresh = false;
        }
    }

    pub fn sort(&mut self, tab: Tab, column: Column) {
        self.clear_fresh();
        let descending = match self.tabs[tab.index()].sort {
            Some((current, descending)) if current == column => !descending,
            _ => column.first_click_descending(),
        };
        self.apply(tab, column, descending);
    }

    fn apply(&mut self, tab: Tab, column: Column, descending: bool) {
        let rows = &mut self.tabs[tab.index()];
        rows.items.sort_by(|a, b| a.compare(b, column, descending));
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

    pub fn footer_tones(&self, tab: Tab) -> Vec<i32> {
        let rows = &self.tabs[tab.index()];
        let mut tones = rows.tones.clone();
        tones.resize(rows.footer.len(), 0);
        tones
    }
}

fn pair(label: &str, value: String) -> (String, String) {
    (label.to_owned(), value)
}

fn lost_items(scan: &SavedScan, zone: &TimeZone) -> Vec<Item> {
    scan.lost
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
        .collect()
}

fn lost_footer(items: &[Item]) -> Vec<(String, String)> {
    let ranked = items
        .iter()
        .filter(|item| item.status.is_some_and(MapStatus::gives_pp))
        .count();
    let lost_pp: f64 = items
        .iter()
        .map(|item| match item.detail {
            Detail::Lost { kept_pp } => item.pp - kept_pp,
            Detail::Weighted { .. } => 0.0,
        })
        .sum();
    let average = if items.is_empty() {
        0.0
    } else {
        lost_pp / items.len() as f64
    };
    vec![
        pair("Lost plays", thousands(items.len() as f64)),
        pair("On ranked maps", thousands(ranked as f64)),
        pair("Average pp lost", format!("{average:.2}")),
    ]
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
        tones: Vec::new(),
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
    let tones = vec![
        0,
        0,
        tone(0.0, gain, 2),
        tone(potential.current_accuracy, potential.potential_accuracy, 2),
        0,
    ];
    TabRows {
        items,
        footer,
        tones,
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

    use super::{Column, Tab, Table, anchor_index, tone, when_short, wrap};

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

    fn order(table: &Table) -> Vec<String> {
        table
            .rows(Tab::Lost)
            .iter()
            .map(|item| format!("{}/{}/{}", item.title, item.artist, item.version))
            .collect()
    }

    #[test]
    fn a_second_click_reverses() {
        let mut table = table(vec![
            lost(100.0, 90.0, "A", "x", ""),
            lost(300.0, 90.0, "B", "y", ""),
            lost(200.0, 90.0, "C", "z", ""),
        ]);
        table.sort(Tab::Lost, Column::Pp);
        assert_eq!(pps(&table), [100.0, 200.0, 300.0]);
        table.sort(Tab::Lost, Column::Pp);
        assert_eq!(pps(&table), [300.0, 200.0, 100.0]);
        table.sort(Tab::Lost, Column::Beatmap);
        assert_eq!(table.sorting(Tab::Lost), (Column::Beatmap, false));
        assert_eq!(order(&table), ["x/A/Insane", "y/B/Insane", "z/C/Insane"]);
        table.sort(Tab::Lost, Column::Beatmap);
        assert_eq!(order(&table), ["z/C/Insane", "y/B/Insane", "x/A/Insane"]);
        table.sort(Tab::Lost, Column::Hits);
        assert_eq!(table.sorting(Tab::Lost), (Column::Hits, false));
        table.sort(Tab::Lost, Column::Mods);
        assert_eq!(table.sorting(Tab::Lost), (Column::Mods, false));
        table.sort(Tab::Lost, Column::Score);
        assert_eq!(table.sorting(Tab::Lost), (Column::Score, true));
        table.sort(Tab::Lost, Column::Accuracy);
        assert_eq!(table.sorting(Tab::Lost), (Column::Accuracy, true));
    }

    #[test]
    fn ties_keep_the_previous_order() {
        let mut table = table(vec![
            lost(100.0, 90.0, "Same", "first", ""),
            lost(300.0, 90.0, "Same", "second", ""),
            lost(200.0, 90.0, "Other", "third", ""),
        ]);
        table.sort(Tab::Lost, Column::Beatmap);
        assert_eq!(
            order(&table),
            [
                "first/Same/Insane",
                "second/Same/Insane",
                "third/Other/Insane"
            ]
        );
        table.sort(Tab::Lost, Column::Accuracy);
        assert_eq!(
            order(&table),
            [
                "first/Same/Insane",
                "second/Same/Insane",
                "third/Other/Insane"
            ]
        );
    }

    #[test]
    fn beatmap_sorts_by_title() {
        let mut hard = lost(1.0, 0.0, "A", "alpha", "");
        hard.version = "Hard".into();
        let mut easy = lost(2.0, 0.0, "A", "alpha", "");
        easy.version = "Easy".into();
        let mut table = table(vec![
            lost(3.0, 0.0, "Z", "beta", ""),
            lost(4.0, 0.0, "Y", "Alpha", ""),
            hard,
            lost(5.0, 0.0, "B", "alpha", ""),
            easy,
        ]);
        table.sort(Tab::Lost, Column::Beatmap);
        assert_eq!(
            order(&table),
            [
                "alpha/A/Easy",
                "alpha/A/Hard",
                "alpha/B/Insane",
                "Alpha/Y/Insane",
                "beta/Z/Insane"
            ]
        );
    }

    #[test]
    fn mods_group_identical_combinations_nm_first() {
        let mut table = table(vec![
            lost(100.0, 0.0, "A", "a", "HDDT"),
            lost(10.0, 0.0, "A", "b", "HD"),
            lost(300.0, 0.0, "A", "c", "DTHD"),
            lost(50.0, 0.0, "A", "d", ""),
            lost(200.0, 0.0, "A", "e", "DT"),
            lost(20.0, 0.0, "A", "f", "HR"),
            lost(70.0, 0.0, "A", "g", ""),
            lost(5.0, 0.0, "A", "h", "NFEZ"),
        ]);
        table.sort(Tab::Lost, Column::Mods);
        assert_eq!(table.sorting(Tab::Lost), (Column::Mods, false));
        assert_eq!(
            pps(&table),
            [70.0, 50.0, 10.0, 20.0, 200.0, 5.0, 300.0, 100.0]
        );
        table.sort(Tab::Lost, Column::Mods);
        assert_eq!(
            pps(&table),
            [300.0, 100.0, 5.0, 200.0, 20.0, 10.0, 70.0, 50.0]
        );
    }

    #[test]
    fn hits_sort_by_misses_then_50s_then_100s() {
        let mut rows = Vec::new();
        for (name, misses, n50, n100) in [
            ("m1", 1, 0, 0),
            ("clean", 0, 0, 0),
            ("f2", 0, 2, 0),
            ("f1h9", 0, 1, 9),
            ("f1h3", 0, 1, 3),
            ("m3", 3, 0, 0),
        ] {
            let mut row = lost(1.0, 0.0, "A", name, "");
            row.hits.misses = misses;
            row.hits.n50 = n50;
            row.hits.n100 = n100;
            rows.push(row);
        }
        let mut table = table(rows);
        table.sort(Tab::Lost, Column::Hits);
        let titles: Vec<&str> = table
            .rows(Tab::Lost)
            .iter()
            .map(|item| item.title.as_str())
            .collect();
        assert_eq!(titles, ["clean", "f1h3", "f1h9", "f2", "m1", "m3"]);
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
    fn the_short_date_has_a_three_letter_month_and_no_time() {
        assert_eq!(when_short(SEPTEMBER_2024, &TimeZone::UTC), "3 Sep 2024");
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

    fn mixed_table() -> Table {
        let mut loved = lost(200.0, 190.0, "B", "loved", "");
        loved.status = MapStatus::Loved;
        let mut approved = lost(150.0, 140.0, "C", "approved", "");
        approved.status = MapStatus::Approved;
        let mut graveyard = lost(100.0, 100.0, "D", "grave", "");
        graveyard.status = MapStatus::Graveyard;
        table(vec![
            lost(300.0, 280.0, "A", "ranked", ""),
            loved,
            approved,
            graveyard,
        ])
    }

    fn titles(table: &Table) -> Vec<&str> {
        table
            .rows(Tab::Lost)
            .iter()
            .map(|item| item.title.as_str())
            .collect()
    }

    #[test]
    fn the_lost_tab_shows_ranked_plays_unless_include() {
        let mut table = mixed_table();
        assert_eq!(table.rows(Tab::Lost).len(), 4);
        table.set_include_all(false);
        assert_eq!(titles(&table), ["ranked", "approved"]);
        table.sort(Tab::Lost, Column::Beatmap);
        table.set_include_all(true);
        assert_eq!(titles(&table), ["approved", "grave", "loved", "ranked"]);
        assert_eq!(table.sorting(Tab::Lost), (Column::Beatmap, false));
        table.set_include_all(false);
        assert_eq!(titles(&table), ["approved", "ranked"]);
    }

    #[test]
    fn the_footer_counts_the_shown_rows() {
        let mut table = mixed_table();
        table.set_include_all(false);
        assert_eq!(
            table.footer(Tab::Lost),
            [
                ("Lost plays".to_owned(), "2".to_owned()),
                ("On ranked maps".to_owned(), "2".to_owned()),
                ("Average pp lost".to_owned(), "15.00".to_owned()),
            ]
        );
        table.set_include_all(true);
        assert_eq!(
            table.footer(Tab::Lost),
            [
                ("Lost plays".to_owned(), "4".to_owned()),
                ("On ranked maps".to_owned(), "2".to_owned()),
                ("Average pp lost".to_owned(), "10.00".to_owned()),
            ]
        );
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
        let mut loved = lost(100.0, 99.0, "C", "z", "");
        loved.status = MapStatus::Loved;
        let table = Table::in_zone(
            &scan(
                vec![
                    lost(300.0, 280.0, "A", "x", ""),
                    lost(200.0, 190.0, "B", "y", ""),
                    loved,
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
        assert_eq!(table.footer_tones(Tab::Potential), [0, 0, 1, -1, 0]);
        assert_eq!(table.footer_tones(Tab::Lost), [0, 0, 0]);
    }

    #[test]
    fn include_keeps_the_first_visible_row_in_place() {
        let mut rows = Vec::new();
        for (pp, title, status) in [
            (500.0, "A", MapStatus::Ranked),
            (400.0, "B", MapStatus::Loved),
            (300.0, "C", MapStatus::Ranked),
            (200.0, "D", MapStatus::Loved),
            (100.0, "E", MapStatus::Ranked),
        ] {
            let mut row = lost(pp, 0.0, "X", title, "");
            row.status = status;
            row.unix_time += pp as i64;
            rows.push(row);
        }
        let mut table = table(rows);
        let all = table.rows(Tab::Lost).to_vec();
        table.set_include_all(false);
        let ranked = table.rows(Tab::Lost).to_vec();
        assert_eq!(ranked.len(), 3);
        assert_eq!(anchor_index(&all, &ranked, 0), Some(0));
        assert_eq!(anchor_index(&all, &ranked, 1), Some(1));
        assert_eq!(anchor_index(&all, &ranked, 2), Some(1));
        assert_eq!(anchor_index(&all, &ranked, 3), Some(2));
        assert_eq!(anchor_index(&ranked, &all, 1), Some(2));
        assert_eq!(anchor_index(&ranked, &all, 2), Some(4));
        let tail = vec![all[1].clone(), all[3].clone()];
        assert_eq!(anchor_index(&tail, &ranked, 0), None);
        assert_eq!(anchor_index(&all, &ranked, 9), Some(2));
        assert_eq!(anchor_index(&[], &ranked, 0), None);
        table.set_include_all(true);
        assert!(
            table
                .rows(Tab::Lost)
                .iter()
                .filter(|item| item.fresh)
                .count()
                == 2
        );
    }

    #[test]
    fn potential_tone_follows_the_change() {
        assert_eq!(tone(8557.0, 8668.0, 0), 1);
        assert_eq!(tone(98.38, 97.95, 2), -1);
        assert_eq!(tone(98.38, 98.38, 2), 0);
        assert_eq!(tone(8557.2, 8557.4, 0), 0);
        assert_eq!(tone(98.381, 98.384, 2), 0);
        assert_eq!(tone(98.38, 98.39, 2), 1);
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
