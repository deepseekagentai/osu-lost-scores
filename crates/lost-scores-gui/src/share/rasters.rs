use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use image::RgbaImage;
use image::imageops::{self, FilterType};
use lost_scores_core::files::write_atomic;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use ureq::Error as UreqError;

use crate::avatar::round_corners;

pub const STRIP_WIDTH: u32 = 612;
pub const STRIP_HEIGHT: u32 = 116;
const STRIP_RADIUS: f32 = 16.0;
const STRIP_OPACITY: f32 = 0.35;
const MAX_BYTES: u64 = 5_000_000;

pub fn cover_url(set_id: u32) -> String {
    format!("https://assets.ppy.sh/beatmaps/{set_id}/covers/cover.jpg")
}

fn crop_centre(image: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    let (w, h) = image.dimensions();
    let (crop_w, crop_h) = if u64::from(w) * u64::from(height) > u64::from(h) * u64::from(width) {
        (
            ((f64::from(h) * f64::from(width) / f64::from(height)).round() as u32).clamp(1, w),
            h,
        )
    } else {
        (
            w,
            ((f64::from(w) * f64::from(height) / f64::from(width)).round() as u32).clamp(1, h),
        )
    };
    imageops::crop_imm(image, (w - crop_w) / 2, (h - crop_h) / 2, crop_w, crop_h).to_image()
}

fn into_buffer(image: &RgbaImage) -> SharedPixelBuffer<Rgba8Pixel> {
    SharedPixelBuffer::clone_from_slice(image.as_raw(), image.width(), image.height())
}

pub fn cover_strip(jpeg: &[u8]) -> Result<SharedPixelBuffer<Rgba8Pixel>> {
    let decoded = image::load_from_memory(jpeg)
        .context("cannot decode the cover")?
        .to_rgba8();
    let cropped = crop_centre(&decoded, STRIP_WIDTH, STRIP_HEIGHT);
    let mut strip = imageops::resize(&cropped, STRIP_WIDTH, STRIP_HEIGHT, FilterType::Lanczos3);
    for (x, y, pixel) in strip.enumerate_pixels_mut() {
        let fade = STRIP_OPACITY * (1.0 - x as f32 / STRIP_WIDTH as f32);
        let dx = (STRIP_RADIUS - (x as f32 + 0.5)).max(0.0);
        let dy = (STRIP_RADIUS - (y as f32 + 0.5))
            .max(y as f32 + 0.5 - (STRIP_HEIGHT as f32 - STRIP_RADIUS))
            .max(0.0);
        let outside = (dx * dx + dy * dy).sqrt() - STRIP_RADIUS;
        let coverage = if dx > 0.0 {
            (0.5 - outside).clamp(0.0, 1.0)
        } else {
            1.0
        };
        pixel.0[3] = (255.0 * fade * coverage).round() as u8;
    }
    Ok(into_buffer(&strip))
}

pub fn rounded_square(
    image: &[u8],
    size: u32,
    radius: u32,
) -> Result<SharedPixelBuffer<Rgba8Pixel>> {
    let decoded = image::load_from_memory(image)
        .context("cannot decode the avatar")?
        .to_rgba8();
    let side = decoded.width().min(decoded.height());
    let square = crop_centre(&decoded, side, side);
    let mut scaled = imageops::resize(&square, size, size, FilterType::Lanczos3);
    round_corners(&mut scaled, radius as f32);
    Ok(into_buffer(&scaled))
}

pub trait Fetch {
    fn get(&mut self, url: &str) -> Result<Option<Vec<u8>>>;
}

pub struct HttpFetch {
    agent: ureq::Agent,
}

impl HttpFetch {
    pub fn new() -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .user_agent(lost_scores_app::online::USER_AGENT)
            .build()
            .into();
        Self { agent }
    }
}

impl Default for HttpFetch {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetch for HttpFetch {
    fn get(&mut self, url: &str) -> Result<Option<Vec<u8>>> {
        match self.agent.get(url).call() {
            Ok(mut response) => Ok(Some(
                response
                    .body_mut()
                    .with_config()
                    .limit(MAX_BYTES)
                    .read_to_vec()?,
            )),
            Err(UreqError::StatusCode(404)) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

pub fn covers(
    set_ids: &[u32],
    cache_dir: &Path,
    fetch: &mut dyn Fetch,
    progress: &mut dyn FnMut(usize, usize),
) -> HashMap<u32, Vec<u8>> {
    let mut found = HashMap::new();
    let total = set_ids.len();
    for (index, &set_id) in set_ids.iter().enumerate() {
        if !found.contains_key(&set_id)
            && let Some(bytes) = cover_bytes(set_id, cache_dir, fetch)
        {
            found.insert(set_id, bytes);
        }
        progress(index + 1, total);
    }
    found
}

fn cover_bytes(set_id: u32, cache_dir: &Path, fetch: &mut dyn Fetch) -> Option<Vec<u8>> {
    let file = cache_dir.join(format!("{set_id}.jpg"));
    if let Ok(bytes) = fs::read(&file)
        && image::load_from_memory(&bytes).is_ok()
    {
        return Some(bytes);
    }
    let bytes = fetch.get(&cover_url(set_id)).ok()??;
    image::load_from_memory(&bytes).ok()?;
    let _ = fs::create_dir_all(cache_dir);
    let _ = write_atomic(&file, &bytes);
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Cursor;

    use anyhow::{Result, bail};
    use image::{ImageFormat, Rgb, RgbImage};

    use super::{Fetch, STRIP_HEIGHT, STRIP_WIDTH, cover_strip, cover_url, covers, rounded_square};

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let image = RgbImage::from_fn(width, height, |x, y| {
            Rgb([(x % 251) as u8, (y % 241) as u8, 128])
        });
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Jpeg)
            .unwrap();
        bytes
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = RgbImage::from_pixel(width, height, Rgb([200, 100, 50]));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    struct Counting {
        calls: Vec<String>,
        missing: Vec<String>,
        failing: Vec<String>,
        body: Vec<u8>,
    }

    impl Counting {
        fn new(body: Vec<u8>) -> Self {
            Self {
                calls: Vec::new(),
                missing: Vec::new(),
                failing: Vec::new(),
                body,
            }
        }
    }

    impl Fetch for Counting {
        fn get(&mut self, url: &str) -> Result<Option<Vec<u8>>> {
            self.calls.push(url.to_string());
            if self.failing.iter().any(|u| u == url) {
                bail!("network down");
            }
            if self.missing.iter().any(|u| u == url) {
                return Ok(None);
            }
            Ok(Some(self.body.clone()))
        }
    }

    #[test]
    fn the_cover_url_points_at_the_ppy_cdn() {
        assert_eq!(
            cover_url(2190501),
            "https://assets.ppy.sh/beatmaps/2190501/covers/cover.jpg"
        );
    }

    #[test]
    fn cover_strip_is_exactly_612_by_116() {
        for (w, h) in [(900, 250), (400, 400), (1200, 200)] {
            let strip = cover_strip(&jpeg(w, h)).unwrap();
            assert_eq!((strip.width(), strip.height()), (STRIP_WIDTH, STRIP_HEIGHT));
        }
    }

    #[test]
    fn cover_fade_goes_from_35_percent_to_nothing() {
        let strip = cover_strip(&jpeg(900, 250)).unwrap();
        let pixels = strip.as_slice();
        let middle = (STRIP_HEIGHT / 2) as usize * STRIP_WIDTH as usize;
        let first = pixels[middle].a;
        let last = pixels[middle + STRIP_WIDTH as usize - 1].a;
        assert!((88..=90).contains(&first), "first alpha {first}");
        assert_eq!(last, 0);
    }

    #[test]
    fn cover_strip_rounds_only_its_left_corners() {
        let strip = cover_strip(&jpeg(900, 250)).unwrap();
        let pixels = strip.as_slice();
        let at = |x: usize, y: usize| pixels[y * STRIP_WIDTH as usize + x].a;
        assert_eq!(at(0, 0), 0);
        assert_eq!(at(0, STRIP_HEIGHT as usize - 1), 0);
        assert!(at(0, STRIP_HEIGHT as usize / 2) > 80);
    }

    #[test]
    fn avatar_corners_are_transparent() {
        let avatar = rounded_square(&png(300, 300), 128, 28).unwrap();
        assert_eq!((avatar.width(), avatar.height()), (128, 128));
        let pixels = avatar.as_slice();
        for (x, y) in [(0, 0), (127, 0), (0, 127), (127, 127)] {
            assert_eq!(pixels[y * 128 + x].a, 0, "corner {x},{y}");
        }
        assert_eq!(pixels[64 * 128 + 64].a, 255);
        assert_eq!(pixels[64 * 128].a, 255);
    }

    #[test]
    fn a_non_square_avatar_is_cropped_to_the_size() {
        let avatar = rounded_square(&png(300, 200), 208, 44).unwrap();
        assert_eq!((avatar.width(), avatar.height()), (208, 208));
    }

    #[test]
    fn cached_covers_are_not_downloaded_again() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("7.jpg"), jpeg(20, 10)).unwrap();
        let mut fetch = Counting::new(jpeg(30, 15));
        let mut seen = Vec::new();
        let found = covers(&[7, 8], dir.path(), &mut fetch, &mut |done, total| {
            seen.push((done, total))
        });
        assert_eq!(fetch.calls, vec![cover_url(8)]);
        assert_eq!(found.len(), 2);
        assert_eq!(found[&7], jpeg(20, 10));
        assert!(dir.path().join("8.jpg").exists());
        assert_eq!(seen.last(), Some(&(2, 2)));

        let mut again = Counting::new(Vec::new());
        let found = covers(&[7, 8], dir.path(), &mut again, &mut |_, _| {});
        assert!(again.calls.is_empty());
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn a_missing_cover_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let mut fetch = Counting::new(jpeg(30, 15));
        fetch.missing.push(cover_url(1));
        fetch.failing.push(cover_url(2));
        let found = covers(&[1, 2, 3], dir.path(), &mut fetch, &mut |_, _| {});
        assert_eq!(found.keys().copied().collect::<Vec<_>>(), vec![3]);
        assert!(!dir.path().join("1.jpg").exists());
        assert!(!dir.path().join("2.jpg").exists());
    }
}
