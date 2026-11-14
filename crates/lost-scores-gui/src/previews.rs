use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use image::imageops::{self, FilterType};
use slint::{Rgba8Pixel, SharedPixelBuffer};

use crate::share::render::{BADGE_FILE, ShareImage};

pub const WIDE_LOGICAL: f32 = 980.0;
pub const BADGE_LOGICAL: f32 = 520.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub label: &'static str,
    pub path: PathBuf,
    pub width: u32,
    pub stamp: Option<SystemTime>,
}

pub fn natural_width(path: &Path) -> f32 {
    if path.file_name().is_some_and(|name| name == BADGE_FILE) {
        BADGE_LOGICAL
    } else {
        WIDE_LOGICAL
    }
}

pub fn preview_pixels(natural: f32, room: f32, scale: f32) -> u32 {
    ((natural.min(room) * scale).round() as u32).max(1)
}

pub fn plan(images: &[ShareImage], room: f32, scale: f32) -> Vec<Wanted> {
    if room < 1.0 {
        return Vec::new();
    }
    images
        .iter()
        .map(|image| Wanted {
            label: image.label,
            path: image.path.clone(),
            width: preview_pixels(natural_width(&image.path), room, scale),
            stamp: std::fs::metadata(&image.path)
                .and_then(|meta| meta.modified())
                .ok(),
        })
        .collect()
}

pub const MAX_HEIGHT: u32 = 16_000;
pub const CARD_HEAD: f32 = 40.0;
pub const CARD_FOOT: f32 = 18.0;

pub fn picture_tops(heights: &[f32]) -> Vec<f32> {
    let mut top = 0.0;
    heights
        .iter()
        .map(|height| {
            let here = top;
            top += CARD_HEAD + height + CARD_FOOT;
            here
        })
        .collect()
}

const EDGE: f32 = 1.0;

pub fn step_target(
    tops: &[f32],
    top: f32,
    visible: f32,
    content: f32,
    forward: bool,
) -> Option<f32> {
    if forward {
        let furthest = (content - visible).max(0.0);
        if top >= furthest - EDGE {
            return None;
        }
        tops.iter().copied().find(|at| *at > top + EDGE)
    } else {
        tops.iter().rev().copied().find(|at| *at < top - EDGE)
    }
}

pub fn render_preview(path: &Path, width: u32) -> Result<SharedPixelBuffer<Rgba8Pixel>> {
    let decoded = image::open(path)
        .with_context(|| format!("cannot read {}", path.display()))?
        .into_rgba8();
    let (source_width, source_height) = (f64::from(decoded.width()), f64::from(decoded.height()));
    let mut width = width;
    let mut height = ((source_height * f64::from(width) / source_width).round() as u32).max(1);
    if height > MAX_HEIGHT {
        width = ((f64::from(MAX_HEIGHT) * source_width / source_height).floor() as u32).max(1);
        height = ((source_height * f64::from(width) / source_width).round() as u32).max(1);
    }
    let fitted = if decoded.dimensions() == (width, height) {
        decoded
    } else {
        imageops::resize(&decoded, width, height, FilterType::Lanczos3)
    };
    Ok(SharedPixelBuffer::clone_from_slice(
        fitted.as_raw(),
        fitted.width(),
        fitted.height(),
    ))
}

pub fn render_all(wanted: &[Wanted]) -> Vec<(Wanted, Result<SharedPixelBuffer<Rgba8Pixel>>)> {
    wanted
        .iter()
        .map(|item| (item.clone(), render_preview(&item.path, item.width)))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use image::{Rgba, RgbaImage};

    use super::{
        BADGE_LOGICAL, WIDE_LOGICAL, natural_width, picture_tops, preview_pixels, render_preview,
        step_target,
    };

    fn save(dir: &Path, name: &str, width: u32, height: u32) -> std::path::PathBuf {
        let path = dir.join(name);
        RgbaImage::from_fn(width, height, |x, y| {
            Rgba([(x % 251) as u8, (y % 241) as u8, 90, 255])
        })
        .save(&path)
        .unwrap();
        path
    }

    #[test]
    fn the_width_is_the_room_up_to_the_natural_width_times_the_scale() {
        assert_eq!(preview_pixels(WIDE_LOGICAL, 584.0, 1.25), 730);
        assert_eq!(preview_pixels(WIDE_LOGICAL, 1500.0, 1.25), 1225);
        assert_eq!(preview_pixels(BADGE_LOGICAL, 584.0, 1.25), 650);
        assert_eq!(preview_pixels(WIDE_LOGICAL, 583.3, 1.25), 729);
        assert_eq!(preview_pixels(WIDE_LOGICAL, 0.0, 1.25), 1);
    }

    #[test]
    fn the_badge_is_narrower_than_the_two_big_pictures() {
        assert_eq!(natural_width(Path::new("/s/summary_badge.png")), 520.0);
        assert_eq!(natural_width(Path::new("/s/lost_scores.png")), 980.0);
        assert_eq!(natural_width(Path::new("/s/potential_top.png")), 980.0);
    }

    #[test]
    fn a_preview_has_the_asked_width_and_the_height_follows_the_ratio() {
        let dir = tempfile::tempdir().unwrap();
        let path = save(dir.path(), "lost_scores.png", 1960, 1000);
        let preview = render_preview(&path, 730).unwrap();
        assert_eq!((preview.width(), preview.height()), (730, 372));
        let same = render_preview(&path, 1960).unwrap();
        assert_eq!((same.width(), same.height()), (1960, 1000));
    }

    #[test]
    fn a_tall_picture_is_prepared_narrower_to_stay_under_the_texture_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = save(dir.path(), "lost_scores.png", 98, 40_000);
        let preview = render_preview(&path, 61).unwrap();
        assert_eq!((preview.width(), preview.height()), (39, 15_918));
        let fits = save(dir.path(), "potential_top.png", 98, 20_000);
        let preview = render_preview(&fits, 61).unwrap();
        assert_eq!((preview.width(), preview.height()), (61, 12_449));
    }

    #[test]
    fn an_unreadable_picture_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lost_scores.png");
        std::fs::write(&path, b"not a png").unwrap();
        assert!(render_preview(&path, 100).is_err());
        assert!(render_preview(&dir.path().join("none.png"), 100).is_err());
    }

    #[test]
    fn each_picture_starts_after_the_card_of_the_one_before() {
        assert_eq!(picture_tops(&[100.0, 200.0, 50.0]), [0.0, 158.0, 416.0]);
        assert!(picture_tops(&[]).is_empty());
    }

    #[test]
    fn next_goes_to_the_following_picture_and_stops_at_the_end() {
        let tops = [0.0, 158.0, 416.0];
        assert_eq!(step_target(&tops, 0.0, 500.0, 900.0, true), Some(158.0));
        assert_eq!(step_target(&tops, 158.0, 500.0, 900.0, true), Some(416.0));
        assert_eq!(step_target(&tops, 416.0, 500.0, 900.0, true), None);
        assert_eq!(step_target(&tops, 400.0, 500.0, 900.0, true), None);
    }

    #[test]
    fn previous_is_off_at_the_top_and_goes_to_the_picture_start_otherwise() {
        let tops = [0.0, 158.0, 416.0];
        assert_eq!(step_target(&tops, 0.0, 500.0, 900.0, false), None);
        assert_eq!(step_target(&tops, 158.0, 500.0, 900.0, false), Some(0.0));
        assert_eq!(step_target(&tops, 200.0, 500.0, 900.0, false), Some(158.0));
    }

    #[test]
    fn a_short_list_has_nowhere_to_go() {
        let tops = [0.0, 158.0];
        assert_eq!(step_target(&tops, 0.0, 500.0, 400.0, true), None);
    }
}
