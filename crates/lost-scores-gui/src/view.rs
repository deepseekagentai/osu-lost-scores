use std::cell::{Cell, RefCell};
use std::rc::Rc;

use lost_scores_app::rows::thousands;
use lost_scores_core::grade::Grade;
use lost_scores_core::maps::MapStatus;
use slint::{Color, Model, ModelNotify, ModelRc, ModelTracker, VecModel};

use crate::table::{Detail, Item, Tab, Table};
use crate::{FootItem, ModPill, ScoreRow};

const MOD_FALLBACK: u32 = 0x9aa0aa;

pub fn rank_style(grade: Grade) -> (&'static str, u32) {
    match grade {
        Grade::XH => ("SS", 0xdfe6ee),
        Grade::X => ("SS", 0xde31ae),
        Grade::SH => ("S", 0xc8d3dd),
        Grade::S => ("S", 0x02b5c3),
        Grade::A => ("A", 0x88da20),
        Grade::B => ("B", 0xe3b130),
        Grade::C => ("C", 0xff8e5d),
        Grade::D => ("D", 0xff5a5a),
    }
}

pub fn mod_color(acronym: &str) -> u32 {
    match acronym {
        "HD" | "DT" | "NC" | "HR" | "FL" | "SD" | "PF" => 0xff6666,
        "EZ" | "NF" | "HT" => 0xb3ff66,
        "SO" | "AP" | "RX" => 0x66ccff,
        "TD" => 0xffcc22,
        _ => MOD_FALLBACK,
    }
}

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

pub struct Chip {
    pub text: &'static str,
    pub background: u32,
    pub ink: u32,
}

pub fn status_chip(status: Option<MapStatus>) -> Option<Chip> {
    let (text, background, ink) = match status? {
        MapStatus::Loved => ("loved", 0xff66ab, 0x1b1825),
        MapStatus::Qualified => ("qualified", 0x66ccff, 0x1b1825),
        MapStatus::Graveyard => ("graveyard", 0x55535c, 0xe5e2ec),
        MapStatus::Unknown => ("unknown", 0x3a3743, 0x9f9aac),
        _ => return None,
    };
    Some(Chip {
        text,
        background,
        ink,
    })
}

pub fn beatmap_link(id: Option<u32>) -> String {
    id.map_or_else(String::new, |id| format!("b/{id}"))
}

pub fn weighted_text(weight: f64, weighted: f64) -> String {
    format!("{:.0}% · {weighted:.1}", weight * 100.0)
}

pub fn lost_text(pp: f64, kept: f64) -> String {
    format!("-{:.1}pp", (pp - kept).max(0.0))
}

fn color(rgb: u32) -> Color {
    Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub fn row(item: &Item) -> ScoreRow {
    let (rank, rank_color) = rank_style(item.grade);
    let chip = status_chip(item.status);
    let mods: Vec<ModPill> = mod_acronyms(&item.mods)
        .into_iter()
        .map(|acronym| ModPill {
            text: acronym.into(),
            color: color(mod_color(acronym)),
        })
        .collect();
    let (place, sub, sub_red, kept_tip, lost) = match &item.detail {
        Detail::Lost { kept_pp } => (
            String::new(),
            String::new(),
            lost_text(item.pp, *kept_pp),
            format!("Kept by the server: {kept_pp:.2}pp"),
            false,
        ),
        Detail::Weighted {
            place,
            weight,
            weighted,
            lost,
        } => (
            place.to_string(),
            weighted_text(*weight, *weighted),
            String::new(),
            String::new(),
            *lost,
        ),
    };
    ScoreRow {
        rank: rank.into(),
        rank_color: color(rank_color),
        place: place.into(),
        title: item.title.as_str().into(),
        artist: item.artist.as_str().into(),
        version: item.version.as_str().into(),
        creator: item.creator.as_str().into(),
        date: item.date.as_str().into(),
        date_short: item.date_short.as_str().into(),
        link: beatmap_link(item.beatmap_id).into(),
        beatmap_id: item.beatmap_id.map_or(0, |id| id as i32),
        mods: ModelRc::new(VecModel::from(mods)),
        c100: item.hits.n100 as i32,
        c50: item.hits.n50 as i32,
        miss: item.hits.misses as i32,
        score: thousands(item.score as f64).into(),
        acc: format!("{:.2}%", item.accuracy).into(),
        pp: format!("{:.0}", item.pp).into(),
        sub: sub.into(),
        sub_red: sub_red.into(),
        kept_tip: kept_tip.into(),
        lost,
        status: chip.as_ref().map_or("", |chip| chip.text).into(),
        status_color: color(chip.as_ref().map_or(0, |chip| chip.background)),
        status_text: color(chip.as_ref().map_or(0, |chip| chip.ink)),
    }
}

pub fn foot(table: &Table, tab: Tab) -> ModelRc<FootItem> {
    let items: Vec<FootItem> = table
        .footer(tab)
        .into_iter()
        .map(|(label, value)| FootItem {
            positive: value.starts_with('+'),
            label: label.into(),
            value: value.into(),
        })
        .collect();
    ModelRc::new(VecModel::from(items))
}

pub struct RowsModel {
    table: Rc<RefCell<Table>>,
    tab: Cell<Tab>,
    notify: ModelNotify,
}

impl RowsModel {
    pub fn new(table: Rc<RefCell<Table>>) -> Self {
        Self {
            table,
            tab: Cell::new(Tab::Lost),
            notify: ModelNotify::default(),
        }
    }

    pub fn show(&self, tab: Tab) {
        self.tab.set(tab);
        self.notify.reset();
    }

    pub fn refresh(&self) {
        self.notify.reset();
    }
}

impl Model for RowsModel {
    type Data = ScoreRow;

    fn row_count(&self) -> usize {
        self.table.borrow().rows(self.tab.get()).len()
    }

    fn row_data(&self, row_index: usize) -> Option<ScoreRow> {
        self.table
            .borrow()
            .rows(self.tab.get())
            .get(row_index)
            .map(row)
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

#[cfg(test)]
mod tests {
    use jiff::tz::TimeZone;
    use lost_scores_app::history::{SavedScan, VERSION};
    use lost_scores_app::rows::{Hits, LostRow};
    use lost_scores_core::grade::Grade;
    use lost_scores_core::maps::MapStatus;
    use slint::Model;

    use super::{
        beatmap_link, lost_text, mod_acronyms, mod_color, rank_style, row, status_chip,
        weighted_text,
    };
    use crate::table::{Tab, Table};

    #[test]
    fn grades_use_the_osu_colours() {
        assert_eq!(rank_style(Grade::X), ("SS", 0xde31ae));
        assert_eq!(rank_style(Grade::S), ("S", 0x02b5c3));
        assert_eq!(rank_style(Grade::A), ("A", 0x88da20));
        assert_eq!(rank_style(Grade::B), ("B", 0xe3b130));
        assert_eq!(rank_style(Grade::C), ("C", 0xff8e5d));
        assert_eq!(rank_style(Grade::D), ("D", 0xff5a5a));
        assert_eq!(rank_style(Grade::XH).0, "SS");
        assert_eq!(rank_style(Grade::SH).0, "S");
        assert_ne!(rank_style(Grade::SH).1, rank_style(Grade::S).1);
    }

    #[test]
    fn mods_are_split_in_pairs_and_coloured_by_type() {
        assert_eq!(mod_acronyms("HDDT"), ["HD", "DT"]);
        assert!(mod_acronyms("").is_empty());
        assert_eq!(mod_color("HD"), 0xff6666);
        assert_eq!(mod_color("EZ"), 0xb3ff66);
        assert_eq!(mod_color("SO"), 0x66ccff);
        assert_eq!(mod_color("TD"), 0xffcc22);
    }

    #[test]
    fn only_special_statuses_get_a_chip() {
        assert_eq!(
            status_chip(Some(MapStatus::Loved)).unwrap().background,
            0xff66ab
        );
        assert_eq!(
            status_chip(Some(MapStatus::Qualified)).unwrap().background,
            0x66ccff
        );
        assert_eq!(
            status_chip(Some(MapStatus::Graveyard)).unwrap().text,
            "graveyard"
        );
        assert_eq!(
            status_chip(Some(MapStatus::Unknown)).unwrap().text,
            "unknown"
        );
        assert!(status_chip(Some(MapStatus::Ranked)).is_none());
        assert!(status_chip(None).is_none());
    }

    #[test]
    fn sub_texts_follow_the_spec() {
        assert_eq!(weighted_text(0.95, 440.84), "95% · 440.8");
        assert_eq!(lost_text(411.87, 400.0), "-11.9pp");
        assert_eq!(beatmap_link(Some(77)), "b/77");
        assert_eq!(beatmap_link(None), "");
    }

    #[test]
    fn a_row_keeps_cyrillic_and_japanese_titles_whole() {
        let lost = LostRow {
            pp: 111.87,
            kept_pp: 100.0,
            score: 1_234_567,
            kept_score: 1_000_000,
            mods: "HDDT".into(),
            unix_time: 1_725_406_740,
            played: "2024-09-03".into(),
            status: MapStatus::Loved,
            beatmap_md5: "0b577cbe9bc774600d20cfa17c0aa94c".into(),
            beatmap_id: Some(5),
            artist: "Кино".into(),
            title: "ようこそジャパリパークへ".into(),
            version: "Insane".into(),
            file: "a.osr".into(),
            creator: "Mapper".into(),
            hits: Hits {
                n300: 400,
                n100: 12,
                n50: 3,
                misses: 1,
            },
            accuracy: 98.5,
            grade: Grade::A,
            max_combo: 500,
        };
        let scan = SavedScan {
            version: VERSION,
            made_at: 1_725_406_740,
            player_id: 1,
            player_name: "Player".into(),
            include_all: false,
            check_missing: false,
            lost: vec![lost],
            lost_total: 1,
            lost_ranked: 0,
            potential: None,
        };
        let table = Table::in_zone(&scan, &TimeZone::UTC);
        let shown = row(&table.rows(Tab::Lost)[0]);
        assert_eq!(shown.artist, "Кино");
        assert_eq!(shown.title, "ようこそジャパリパークへ");
        assert_eq!(shown.sub_red, "-11.9pp");
        assert_eq!(shown.kept_tip, "Kept by the server: 100.00pp");
        assert_eq!(shown.status, "loved");
        assert_eq!(shown.link, "b/5");
        assert_eq!(shown.mods.row_count(), 2);
        assert_eq!((shown.c100, shown.c50, shown.miss), (12, 3, 1));
    }
}
