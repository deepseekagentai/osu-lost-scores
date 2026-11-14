use std::path::Path;

use anyhow::{Result, anyhow};
use slint::{Rgba8Pixel, SharedPixelBuffer};

use crate::share::rasters::{Fetch, HttpFetch, cached, rounded_square};

const CORNER_RATIO: f32 = 16.0 / 120.0;

pub fn avatar_pixels(logical: f32, scale: f32) -> u32 {
    (logical * scale).round() as u32
}

pub fn render_avatar(bytes: &[u8], pixels: u32) -> Result<SharedPixelBuffer<Rgba8Pixel>> {
    rounded_square(bytes, pixels, pixels as f32 * CORNER_RATIO)
}

pub fn fetch(id: u32, cache: Option<&Path>) -> Result<Vec<u8>> {
    fetch_via(id, cache, &mut HttpFetch::new())
}

pub fn fetch_via(id: u32, cache: Option<&Path>, fetch: &mut dyn Fetch) -> Result<Vec<u8>> {
    fetch_with(id, cache, |id| {
        fetch
            .get(&format!("https://a.ppy.sh/{id}"))?
            .ok_or_else(|| anyhow!("the avatar was not found"))
    })
}

fn fetch_with(
    id: u32,
    cache: Option<&Path>,
    download: impl FnOnce(u32) -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    let file = cache.map(|dir| dir.join(id.to_string()));
    cached(file.as_deref(), || download(id).map(Some))?
        .ok_or_else(|| anyhow!("the avatar was not found"))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{ImageFormat, Rgba, RgbaImage};

    use super::{avatar_pixels, fetch_via, fetch_with, render_avatar};
    use crate::share::rasters::Fetch;

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

    struct Canned(Option<Vec<u8>>, Vec<String>);

    impl Fetch for Canned {
        fn get(&mut self, url: &str) -> anyhow::Result<Option<Vec<u8>>> {
            self.1.push(url.to_owned());
            Ok(self.0.clone())
        }
    }

    #[test]
    fn the_avatar_downloads_through_the_fetch_seam() {
        let dir = tempfile::tempdir().unwrap();
        let mut found = Canned(Some(png(8)), Vec::new());
        let bytes = fetch_via(7, Some(dir.path()), &mut found).unwrap();
        assert_eq!(bytes, png(8));
        assert_eq!(found.1, ["https://a.ppy.sh/7"]);
        assert_eq!(std::fs::read(dir.path().join("7")).unwrap(), png(8));
        let mut again = Canned(None, Vec::new());
        assert_eq!(fetch_via(7, Some(dir.path()), &mut again).unwrap(), png(8));
        assert!(again.1.is_empty());
        let mut missing = Canned(None, Vec::new());
        assert!(fetch_via(8, Some(dir.path()), &mut missing).is_err());
    }
}
