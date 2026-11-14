use std::fs;
use std::io::Cursor;
use std::path::Path;

use anyhow::Result;
use image::{ImageFormat, Rgb, RgbImage};
use lost_scores_app::history::{SavedScan, VERSION, save};
use lost_scores_app::pipeline::PotentialResult;
use lost_scores_app::rows::{Hits, LostRow, PotentialRow};
use lost_scores_core::grade::Grade;
use lost_scores_core::maps::MapStatus;
use lost_scores_gui::share::model::ShareOptions;
use lost_scores_gui::share::rasters::Fetch;
use lost_scores_gui::share::render::{
    BADGE_FILE, Caches, LOST_FILE, POTENTIAL_FILE, render_with, use_offscreen_platform,
};

const MADE_AT: i64 = 1_794_649_471;

struct Offline;

impl Fetch for Offline {
    fn get(&mut self, _url: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
}

fn encoded(image: &RgbImage, format: ImageFormat) -> Vec<u8> {
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), format)
        .unwrap();
    bytes
}

fn hits() -> Hits {
    Hits {
        n300: 400,
        n100: 6,
        n50: 1,
        misses: 2,
    }
}

fn lost(index: u32, status: MapStatus, mods: &str) -> LostRow {
    LostRow {
        pp: 400.0 - f64::from(index),
        kept_pp: 380.0 - f64::from(index),
        score: 900_000,
        kept_score: 800_000,
        mods: mods.into(),
        unix_time: MADE_AT - 4000 * i64::from(index + 1),
        played: "2026-11-13".into(),
        status,
        beatmap_md5: format!("{index:032x}"),
        beatmap_id: Some(index),
        beatmapset_id: Some(100 + index),
        artist: "Synthetic Artist".into(),
        title: format!("Synthetic Title {index}"),
        version: "Insane".into(),
        file: "x.osr".into(),
        creator: "Synthetic Mapper".into(),
        hits: hits(),
        accuracy: 97.25,
        grade: Grade::S,
        max_combo: 321,
    }
}

fn weighted(place: usize, lost: bool) -> PotentialRow {
    PotentialRow {
        place,
        pp: 600.0 - place as f64,
        weight: 0.95_f64.powi(place as i32 - 1),
        weighted: 500.0,
        lost,
        beatmap_id: place as u32,
        beatmapset_id: Some(100 + place as u32),
        artist: "Synthetic Artist".into(),
        title: format!("Synthetic Top {place}"),
        version: "Expert".into(),
        creator: "Synthetic Mapper".into(),
        mods: if place.is_multiple_of(2) { "HDDT" } else { "" }.into(),
        accuracy: 98.5,
        unix_time: MADE_AT - 86_400,
        played: "2026-11-13".into(),
        hits: hits(),
        score: 1_200_000,
        grade: Grade::A,
        status: MapStatus::Ranked,
        kept_pp: None,
    }
}

fn scan(lost_rows: Vec<LostRow>, potential: Option<PotentialResult>) -> SavedScan {
    SavedScan {
        version: VERSION,
        made_at: MADE_AT,
        player_id: 77,
        player_name: "Synthetic".into(),
        include_all: false,
        check_missing: false,
        lost_total: lost_rows.len(),
        lost_ranked: lost_rows.len(),
        lost: lost_rows,
        potential,
    }
}

fn size(path: &Path) -> (u32, u32) {
    let image = image::open(path).unwrap();
    (image.width(), image.height())
}

#[test]
fn render_all_writes_three_pngs_of_the_right_size() {
    use_offscreen_platform().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let covers = dir.path().join("covers");
    let avatars = dir.path().join("avatars");
    fs::create_dir_all(&covers).unwrap();
    fs::create_dir_all(&avatars).unwrap();
    let picture = RgbImage::from_pixel(900, 250, Rgb([90, 160, 220]));
    fs::write(covers.join("100.jpg"), encoded(&picture, ImageFormat::Jpeg)).unwrap();
    fs::write(
        avatars.join("77"),
        encoded(
            &RgbImage::from_pixel(256, 256, Rgb([220, 150, 40])),
            ImageFormat::Png,
        ),
    )
    .unwrap();
    let caches = Caches {
        covers: Some(covers),
        avatars: Some(avatars),
    };
    let options = ShareOptions {
        count: 5,
        include_all: false,
        show_lost_in_top: true,
    };

    let potential = PotentialResult {
        current_pp: 5000.0,
        potential_pp: 5050.0,
        current_accuracy: 98.0,
        potential_accuracy: 98.1,
        global_rank: Some(1234),
        lost_in_top: 2,
        rows: (1..=12)
            .map(|place| weighted(place, place == 3 || place == 9))
            .collect(),
        online: Vec::new(),
    };
    let folder = save(
        &dir.path().join("scans"),
        &scan(
            (0..3)
                .map(|n| lost(n, MapStatus::Ranked, if n == 0 { "HDDTHR" } else { "" }))
                .collect(),
            Some(potential),
        ),
    )
    .unwrap();
    let written = render_with(&folder, &options, &caches, &mut Offline).unwrap();
    let names: Vec<_> = written
        .iter()
        .map(|path| path.file_name().unwrap().to_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, [LOST_FILE, POTENTIAL_FILE, BADGE_FILE]);
    assert_eq!(size(&written[0]), (1960, 2 * (150 + 64 * 3)));
    assert_eq!(size(&written[1]), (1960, 2 * (222 + 64 * 5)));
    assert_eq!(size(&written[2]), (1040, 280));

    let empty = save(&dir.path().join("scans"), &{
        let mut empty = scan(Vec::new(), None);
        empty.made_at += 60;
        empty
    })
    .unwrap();
    let written = render_with(&empty, &options, &caches, &mut Offline).unwrap();
    assert_eq!(written.len(), 1);
    assert_eq!(size(&written[0]), (1960, 2 * (150 + 64)));
    assert!(!empty.join(POTENTIAL_FILE).exists());
    assert!(!empty.join(BADGE_FILE).exists());
}
