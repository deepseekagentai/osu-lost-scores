use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use image::imageops::{self, FilterType};
use lost_scores_core::files::write_atomic;
use slint::{Rgba8Pixel, SharedPixelBuffer};

const CORNER_RATIO: f32 = 16.0 / 120.0;
const MAX_BYTES: u64 = 5_000_000;

pub fn avatar_pixels(logical: f32, scale: f32) -> u32 {
    (logical * scale).round() as u32
}

pub fn render_avatar(bytes: &[u8], pixels: u32) -> Result<SharedPixelBuffer<Rgba8Pixel>> {
    let decoded = image::load_from_memory(bytes)
        .context("cannot decode the avatar")?
        .to_rgba8();
    let side = decoded.width().min(decoded.height());
    let square = imageops::crop_imm(
        &decoded,
        (decoded.width() - side) / 2,
        (decoded.height() - side) / 2,
        side,
        side,
    )
    .to_image();
    let mut scaled = imageops::resize(&square, pixels, pixels, FilterType::Lanczos3);
    round_corners(&mut scaled, pixels as f32 * CORNER_RATIO);
    Ok(SharedPixelBuffer::clone_from_slice(
        scaled.as_raw(),
        pixels,
        pixels,
    ))
}

fn round_corners(image: &mut image::RgbaImage, radius: f32) {
    let size = image.width() as f32;
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let dx = (x as f32 + 0.5 - radius)
            .min(0.0)
            .min(size - radius - (x as f32 + 0.5));
        let dy = (y as f32 + 0.5 - radius)
            .min(0.0)
            .min(size - radius - (y as f32 + 0.5));
        let outside = (dx * dx + dy * dy).sqrt() - radius;
        let coverage = (0.5 - outside).clamp(0.0, 1.0);
        pixel.0[3] = (f32::from(pixel.0[3]) * coverage).round() as u8;
    }
}

pub fn fetch(id: u32, cache: Option<&Path>) -> Result<Vec<u8>> {
    fetch_with(id, cache, download)
}

fn download(id: u32) -> Result<Vec<u8>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .user_agent(lost_scores_app::online::USER_AGENT)
        .build()
        .into();
    Ok(agent
        .get(format!("https://a.ppy.sh/{id}"))
        .call()?
        .body_mut()
        .with_config()
        .limit(MAX_BYTES)
        .read_to_vec()?)
}

fn fetch_with(
    id: u32,
    cache: Option<&Path>,
    download: impl FnOnce(u32) -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    let file = cache.map(|dir| dir.join(id.to_string()));
    if let Some(file) = &file
        && let Ok(bytes) = fs::read(file)
    {
        if is_image(&bytes) {
            return Ok(bytes);
        }
        let _ = fs::remove_file(file);
    }
    let bytes = download(id)?;
    if !is_image(&bytes) {
        anyhow::bail!("the avatar download is not an image");
    }
    if let Some(file) = &file {
        if let Some(parent) = file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = write_atomic(file, &bytes);
    }
    Ok(bytes)
}

fn is_image(bytes: &[u8]) -> bool {
    image::load_from_memory(bytes).is_ok()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{ImageFormat, Rgba, RgbaImage};

    use super::{avatar_pixels, fetch_with, render_avatar};

    fn png(size: u32) -> Vec<u8> {
        let image = RgbaImage::from_pixel(size, size, Rgba([200, 120, 40, 255]));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn the_avatar_size_follows_the_scale_factor() {
        assert_eq!(avatar_pixels(48.0, 1.0), 48);
        assert_eq!(avatar_pixels(48.0, 1.25), 60);
        assert_eq!(avatar_pixels(48.0, 1.5), 72);
        assert_eq!(avatar_pixels(48.0, 2.0), 96);
    }

    #[test]
    fn render_avatar_returns_the_exact_size() {
        let buffer = render_avatar(&png(256), 60).unwrap();
        assert_eq!((buffer.width(), buffer.height()), (60, 60));
    }

    #[test]
    fn render_avatar_rounds_the_corners_and_keeps_the_middle() {
        let buffer = render_avatar(&png(256), 120).unwrap();
        let pixels = buffer.as_slice();
        assert_eq!(pixels[0].a, 0);
        assert_eq!(pixels[60 * 120 + 60].a, 255);
        assert_eq!(pixels[60 * 120 + 60].r, 200);
    }

    #[test]
    fn a_broken_image_is_an_error() {
        assert!(render_avatar(b"not an image", 60).is_err());
    }

    #[test]
    fn a_bad_download_is_not_cached() {
        let dir = tempfile::tempdir().unwrap();
        let result = fetch_with(7, Some(dir.path()), |_| Ok(b"<html>oops</html>".to_vec()));
        assert!(result.is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_good_download_is_cached_and_a_broken_cache_is_fetched_again() {
        let dir = tempfile::tempdir().unwrap();
        let good = png(8);
        let bytes = fetch_with(7, Some(dir.path()), |_| Ok(good.clone())).unwrap();
        assert_eq!(bytes, good);
        assert_eq!(std::fs::read(dir.path().join("7")).unwrap(), good);
        std::fs::write(dir.path().join("7"), b"broken").unwrap();
        let again = fetch_with(7, Some(dir.path()), |_| Ok(good.clone())).unwrap();
        assert_eq!(again, good);
        assert_eq!(std::fs::read(dir.path().join("7")).unwrap(), good);
    }
}
