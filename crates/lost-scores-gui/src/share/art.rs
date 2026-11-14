use image::RgbaImage;
use lost_scores_core::grade::Grade;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

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

pub struct Art {
    pub grades: Vec<(Grade, Image)>,
    pub mods: Vec<(&'static str, Image)>,
    pub lemon_small: Image,
    pub lemon_large: Image,
    pub clock: Image,
    pub trend_down: Image,
    pub backdrop_wide: Image,
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
            backdrop_wide: image(BACKDROP_WIDE),
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
