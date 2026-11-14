use std::path::PathBuf;

use image::RgbaImage;
use jiff::tz::TimeZone;
use lost_scores_app::history::{SavedScan, VERSION};
use lost_scores_app::pipeline::PotentialResult;
use lost_scores_app::rows::{Hits, LostRow, PotentialRow};
use lost_scores_core::grade::Grade;
use lost_scores_core::maps::MapStatus;
use lost_scores_gui::share::model::{ShareOptions, ShareRow, lost_rows, potential_rows};
use lost_scores_gui::share::rasters::{STRIP_HEIGHT, STRIP_WIDTH, rounded_square};
use lost_scores_gui::share::render::{
    BADGE_AVATAR, BADGE_FILE, HEADER_AVATAR, LOST_FILE, POTENTIAL_FILE, Prepared, Rendered,
    render_images, use_offscreen_platform,
};
use slint::{Rgba8Pixel, SharedPixelBuffer};

const MADE_AT: i64 = 1_794_649_471;
const GRADES: [(Grade, &str); 8] = [
    (Grade::XH, "XH"),
    (Grade::SH, "SH"),
    (Grade::X, "X"),
    (Grade::S, "S"),
    (Grade::A, "A"),
    (Grade::B, "B"),
    (Grade::C, "C"),
    (Grade::D, "D"),
];
const MOD_LISTS: [&str; 6] = ["NFEZTD", "HDHRSD", "DTNCHT", "FLSOPF", "", "HDDT"];
const CLOCK_COLOR: [u8; 3] = [0x9c, 0x97, 0xa8];
const TREND_COLOR: [u8; 3] = [0xff, 0x66, 0x66];
const PP_BLOCK: [u8; 3] = [0x1d, 0x1c, 0x22];
const STRIP_BANDS: [(u32, u32); 2] = [(0, 12), (104, 116)];

struct Template {
    name: String,
    width: usize,
    height: usize,
    points: Vec<(usize, usize, [u8; 3])>,
}

fn scrambled(mut points: Vec<(usize, usize, [u8; 3])>) -> Vec<(usize, usize, [u8; 3])> {
    let count = points.len();
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by_key(|&index| (index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 20);
    let taken: Vec<_> = order.iter().map(|&index| points[index]).collect();
    points.clear();
    taken
}

fn template(
    name: &str,
    image: &RgbaImage,
    recolor: Option<[u8; 3]>,
    keep: impl Fn(u32, u32) -> bool,
) -> Template {
    let points = image
        .enumerate_pixels()
        .filter(|(x, y, pixel)| pixel.0[3] == 255 && keep(*x, *y))
        .map(|(x, y, pixel)| {
            (
                x as usize,
                y as usize,
                recolor.unwrap_or([pixel.0[0], pixel.0[1], pixel.0[2]]),
            )
        })
        .collect();
    Template {
        name: name.to_owned(),
        width: image.width() as usize,
        height: image.height() as usize,
        points: scrambled(points),
    }
}

fn on_background(mut template: Template, image: &RgbaImage, background: [u8; 3]) -> Template {
    let clear = image
        .enumerate_pixels()
        .filter(|(_, _, pixel)| pixel.0[3] == 0)
        .map(|(x, y, _)| (x as usize, y as usize, background));
    template.points.extend(clear);
    template.points = scrambled(template.points);
    template
}

fn count_matches(image: &Rendered, template: &Template) -> usize {
    let (width, height) = (image.width as usize, image.height as usize);
    assert!(
        !template.points.is_empty(),
        "{} has no opaque pixels",
        template.name
    );
    let mut found = 0;
    for y in 0..=height - template.height {
        'positions: for x in 0..=width - template.width {
            for &(dx, dy, rgb) in &template.points {
                let at = ((y + dy) * width + x + dx) * 4;
                if image.rgba[at..at + 3] != rgb || image.rgba[at + 3] != 255 {
                    continue 'positions;
                }
            }
            found += 1;
        }
    }
    found
}

fn asset(path: &str) -> RgbaImage {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets/share")
        .join(path);
    image::open(&file)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()))
        .to_rgba8()
}

fn buffer_image(buffer: &SharedPixelBuffer<Rgba8Pixel>) -> RgbaImage {
    let raw: Vec<u8> = buffer
        .as_slice()
        .iter()
        .flat_map(|pixel| [pixel.r, pixel.g, pixel.b, pixel.a])
        .collect();
    RgbaImage::from_raw(buffer.width(), buffer.height(), raw).unwrap()
}

fn noise(seed: u64, width: u32, height: u32) -> RgbaImage {
    let mut state = seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1;
    RgbaImage::from_fn(width, height, |_, _| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let bytes = state.to_le_bytes();
        image::Rgba([bytes[0], bytes[3], bytes[5], 255])
    })
}

fn pixels(image: &RgbaImage) -> SharedPixelBuffer<Rgba8Pixel> {
    SharedPixelBuffer::clone_from_slice(image.as_raw(), image.width(), image.height())
}

fn hits() -> Hits {
    Hits {
        n300: 400,
        n100: 6,
        n50: 1,
        misses: 2,
    }
}

fn lost(index: usize) -> LostRow {
    LostRow {
        pp: 500.0 - index as f64,
        kept_pp: 480.0 - index as f64,
        score: 900_000,
        kept_score: 800_000,
        mods: MOD_LISTS[index % MOD_LISTS.len()].into(),
        unix_time: MADE_AT - 4000 * (index as i64 + 1),
        played: "2026-11-13".into(),
        status: MapStatus::Ranked,
        beatmap_md5: format!("{index:032x}"),
        beatmap_id: Some(index as u32),
        beatmapset_id: match index {
            3 => Some(1002),
            5 => None,
            _ => Some(1000 + index as u32),
        },
        artist: "Synthetic Artist".into(),
        title: format!("Synthetic Title {index}"),
        version: "Insane".into(),
        file: "x.osr".into(),
        creator: "Synthetic Mapper".into(),
        hits: hits(),
        accuracy: 97.25,
        grade: GRADES[index % GRADES.len()].0,
        max_combo: 321,
    }
}

fn weighted(place: usize) -> PotentialRow {
    PotentialRow {
        place,
        pp: 600.0 - place as f64,
        weight: 0.95_f64.powi(place as i32 - 1),
        weighted: 500.0,
        lost: place == 4,
        beatmap_id: place as u32,
        beatmapset_id: Some(2000 + place as u32),
        artist: "Synthetic Artist".into(),
        title: format!("Synthetic Top {place}"),
        version: "Expert".into(),
        creator: "Synthetic Mapper".into(),
        mods: MOD_LISTS[(place + 2) % MOD_LISTS.len()].into(),
        accuracy: 98.5,
        unix_time: MADE_AT - 86_400 * place as i64,
        played: "2026-11-13".into(),
        hits: hits(),
        score: 1_200_000,
        grade: GRADES[(place + 3) % GRADES.len()].0,
        status: MapStatus::Ranked,
        kept_pp: None,
    }
}

fn synthetic_scan() -> SavedScan {
    let lost_rows: Vec<LostRow> = (0..12).map(lost).collect();
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
        potential: Some(PotentialResult {
            current_pp: 5000.0,
            potential_pp: 5050.0,
            current_accuracy: 98.0,
            potential_accuracy: 98.1,
            global_rank: Some(1234),
            lost_in_top: 1,
            rows: (1..=12).map(weighted).collect(),
            online: Vec::new(),
        }),
    }
}

fn probes(rows: &[ShareRow], prepared: &mut Prepared) {
    for set in rows.iter().filter_map(|row| row.set_id) {
        prepared
            .strips
            .entry(set)
            .or_insert_with(|| pixels(&noise(u64::from(set), STRIP_WIDTH, STRIP_HEIGHT)));
    }
}

fn row_templates(
    rows: &[ShareRow],
    prepared: &Prepared,
    with_trend: bool,
) -> Vec<(Template, usize)> {
    let mut wanted: Vec<(Template, usize)> = Vec::new();
    let mut add = |template: Template, count: usize| match wanted
        .iter_mut()
        .find(|(known, _)| known.name == template.name)
    {
        Some((_, total)) => *total += count,
        None => wanted.push((template, count)),
    };
    for row in rows {
        let grade = GRADES
            .iter()
            .find(|(grade, _)| *grade == row.grade)
            .unwrap()
            .1;
        add(
            template(
                &format!("grade {grade}"),
                &asset(&format!("grades/{grade}.png")),
                None,
                |_, _| true,
            ),
            1,
        );
        for acronym in &row.mods {
            add(
                template(
                    &format!("mod {acronym}"),
                    &asset(&format!("mods/{acronym}.png")),
                    None,
                    |_, _| true,
                ),
                1,
            );
        }
        if let Some(strip) = row.set_id.and_then(|set| prepared.strips.get(&set)) {
            let set = row.set_id.unwrap();
            add(
                template(
                    &format!("cover strip {set}"),
                    &buffer_image(strip),
                    None,
                    |_, y| {
                        STRIP_BANDS
                            .iter()
                            .any(|&(from, to)| (from..to).contains(&y))
                    },
                ),
                1,
            );
        }
        add(
            template("clock", &asset("clock.png"), Some(CLOCK_COLOR), |_, _| true),
            1,
        );
        if with_trend {
            let trend = asset("trend-down.png");
            let solid = template("trend-down", &trend, Some(TREND_COLOR), |_, _| true);
            add(on_background(solid, &trend, PP_BLOCK), 1);
        }
    }
    wanted
}

fn check(image: &Rendered, wanted: Vec<(Template, usize)>, failures: &mut Vec<String>) {
    for (template, expected) in wanted {
        let found = count_matches(image, &template);
        if found != expected {
            failures.push(format!(
                "{}: {} appears {found} times, expected {expected}",
                image.file, template.name
            ));
        }
    }
}

#[test]
fn every_prepared_image_appears_pixel_for_pixel() {
    use_offscreen_platform().unwrap();
    let scan = synthetic_scan();
    let options = ShareOptions {
        count: 12,
        include_all: false,
        show_lost_in_top: true,
    };
    let lost = lost_rows(&scan, &options);
    let potential = potential_rows(&scan, &options);
    let avatar_source = noise(99, 256, 256);
    let header =
        rounded_square(&encoded(&avatar_source), HEADER_AVATAR.0, HEADER_AVATAR.1).unwrap();
    let badge = rounded_square(&encoded(&avatar_source), BADGE_AVATAR.0, BADGE_AVATAR.1).unwrap();
    let mut prepared = Prepared {
        header_avatar: Some(header.clone()),
        badge_avatar: Some(badge.clone()),
        ..Prepared::default()
    };
    probes(&lost, &mut prepared);
    probes(&potential, &mut prepared);

    let images = render_images(&scan, &options, &prepared, &TimeZone::UTC).unwrap();
    let find = |file: &str| images.iter().find(|image| image.file == file).unwrap();
    let mut failures = Vec::new();

    let mut wanted = row_templates(&lost, &prepared, true);
    wanted.push((
        template("header avatar", &buffer_image(&header), None, |_, _| true),
        1,
    ));
    wanted.push((
        template("lemon 36", &asset("lemon-36.png"), None, |_, _| true),
        1,
    ));
    check(find(LOST_FILE), wanted, &mut failures);

    let mut wanted = row_templates(&potential, &prepared, false);
    wanted.push((
        template("header avatar", &buffer_image(&header), None, |_, _| true),
        1,
    ));
    wanted.push((
        template("lemon 36", &asset("lemon-36.png"), None, |_, _| true),
        1,
    ));
    check(find(POTENTIAL_FILE), wanted, &mut failures);

    let wanted = vec![
        (
            template("badge avatar", &buffer_image(&badge), None, |_, _| true),
            1,
        ),
        (
            template("lemon 48", &asset("lemon-48.png"), None, |_, _| true),
            1,
        ),
    ];
    check(find(BADGE_FILE), wanted, &mut failures);

    assert!(
        failures.is_empty(),
        "not drawn pixel for pixel:\n{}",
        failures.join("\n")
    );
}

fn encoded(image: &RgbaImage) -> Vec<u8> {
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}
