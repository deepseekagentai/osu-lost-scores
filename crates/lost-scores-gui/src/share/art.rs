use image::RgbaImage;
use lost_scores_core::grade::Grade;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use crate::share::rasters::{Corners, round_corners};

macro_rules! picture {
    ($path:literal) => {
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/share/", $path)).as_slice()
    };
}

pub const GRADES: [(Grade, &[u8]); 8] = [
    (Grade::XH, picture!("grades/XH.png")),
    (Grade::SH, picture!("grades/SH.png")),
    (Grade::X, picture!("grades/X.png")),
    (Grade::S, picture!("grades/S.png")),
    (Grade::A, picture!("grades/A.png")),
    (Grade::B, picture!("grades/B.png")),
    (Grade::C, picture!("grades/C.png")),
    (Grade::D, picture!("grades/D.png")),
];

pub const MODS: [(&str, &[u8]); 12] = [
    ("NF", picture!("mods/NF.png")),
    ("EZ", picture!("mods/EZ.png")),
    ("TD", picture!("mods/TD.png")),
    ("HD", picture!("mods/HD.png")),
    ("HR", picture!("mods/HR.png")),
    ("SD", picture!("mods/SD.png")),
    ("DT", picture!("mods/DT.png")),
    ("NC", picture!("mods/NC.png")),
    ("HT", picture!("mods/HT.png")),
    ("FL", picture!("mods/FL.png")),
    ("SO", picture!("mods/SO.png")),
    ("PF", picture!("mods/PF.png")),
];

pub const LEMON_SMALL: &[u8] = picture!("lemon-36.png");
pub const LEMON_LARGE: &[u8] = picture!("lemon-48.png");
pub const CLOCK: &[u8] = picture!("clock.png");
pub const TREND_DOWN: &[u8] = picture!("trend-down.png");
pub const BACKDROP_WIDE: &[u8] = picture!("backdrop-wide.png");
pub const BACKDROP_BADGE: &[u8] = picture!("backdrop-badge.png");

pub fn buffer(png: &[u8]) -> SharedPixelBuffer<Rgba8Pixel> {
    let decoded: RgbaImage = image::load_from_memory(png)
        .expect("the share pictures are valid PNG files")
        .to_rgba8();
    SharedPixelBuffer::clone_from_slice(decoded.as_raw(), decoded.width(), decoded.height())
}

pub const BACKDROP_WIDE_HEIGHT: u32 = 880;
const FRAME_RADIUS: f32 = 36.0;
const PAGE: [u8; 3] = [0x1d, 0x1c, 0x22];
const FADE_START: f32 = 0.4;
const FADE_CLEAR: u32 = 72;

fn fade_into_page(image: &mut RgbaImage) {
    let height = image.height() as f32;
    let start = height * FADE_START;
    let end = (height - FADE_CLEAR as f32).max(start + 1.0);
    for (_, y, pixel) in image.enumerate_pixels_mut() {
        let t = ((y as f32 - start) / (end - start)).clamp(0.0, 1.0);
        let weight = t * t * (3.0 - 2.0 * t);
        for (channel, page) in pixel.0.iter_mut().zip(PAGE) {
            let blended = f32::from(*channel) + (f32::from(page) - f32::from(*channel)) * weight;
            *channel = blended.round() as u8;
        }
    }
}

pub fn backdrop_wide(height: u32) -> SharedPixelBuffer<Rgba8Pixel> {
    let mut decoded: RgbaImage = image::load_from_memory(BACKDROP_WIDE)
        .expect("the share pictures are valid PNG files")
        .to_rgba8();
    let rows = height.min(BACKDROP_WIDE_HEIGHT);
    decoded = image::imageops::crop_imm(&decoded, 0, 0, decoded.width(), rows).to_image();
    if height < BACKDROP_WIDE_HEIGHT {
        fade_into_page(&mut decoded);
        round_corners(&mut decoded, FRAME_RADIUS, Corners::BOTTOM);
    }
    SharedPixelBuffer::clone_from_slice(decoded.as_raw(), decoded.width(), decoded.height())
}

pub struct Art {
    pub grades: Vec<(Grade, Image)>,
    pub mods: Vec<(&'static str, Image)>,
    pub lemon_small: Image,
    pub lemon_large: Image,
    pub clock: Image,
    pub trend_down: Image,
    pub backdrop_badge: Image,
}

impl Art {
    pub fn load() -> Self {
        let image = |png: &[u8]| Image::from_rgba8(buffer(png));
        Self {
            grades: GRADES
                .iter()
                .map(|(grade, png)| (*grade, image(png)))
                .collect(),
            mods: MODS
                .iter()
                .map(|(acronym, png)| (*acronym, image(png)))
                .collect(),
            lemon_small: image(LEMON_SMALL),
            lemon_large: image(LEMON_LARGE),
            clock: image(CLOCK),
            trend_down: image(TREND_DOWN),
            backdrop_badge: image(BACKDROP_BADGE),
        }
    }

    pub fn grade(&self, grade: Grade) -> Image {
        self.grades
            .iter()
            .find(|(known, _)| *known == grade)
            .map(|(_, image)| image.clone())
            .unwrap_or_default()
    }

    pub fn module(&self, acronym: &str) -> Option<Image> {
        self.mods
            .iter()
            .find(|(known, _)| *known == acronym)
            .map(|(_, image)| image.clone())
    }
}
