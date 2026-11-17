use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::{self, FilterType};
use image::{DynamicImage, RgbaImage};
use lost_scores_core::files::write_atomic;
use rayon::prelude::*;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use ureq::Error as UreqError;

pub const STRIP_WIDTH: u32 = 612;
pub const STRIP_HEIGHT: u32 = 116;
const STRIP_RADIUS: f32 = 16.0;
const STRIP_OPACITY: f32 = 0.35;
pub(crate) const MAX_BYTES: u64 = 5_000_000;
const TIMEOUT: Duration = Duration::from_secs(15);

const COVER_HOST: &str = "https://assets.ppy.sh/beatmaps/";
const COVER_WIDTH: u32 = 900;
const COVER_HEIGHT: u32 = 250;
const COVER_QUALITY: u8 = 90;
const LOCAL_CHUNK: usize = 32;
const LOCAL_THREADS: usize = 8;
const MAX_MAP_BYTES: u64 = 1024 * 1024;
const MAX_BACKGROUND_BYTES: u64 = 32 * 1024 * 1024;
const MAX_BACKGROUND_SIDE: u32 = 8192;
const MAX_BACKGROUND_ALLOC: u64 = 256 * 1024 * 1024;

pub fn cover_url(set_id: u32) -> String {
    format!("{COVER_HOST}{set_id}/covers/cover.jpg")
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
    for (x, _, pixel) in strip.enumerate_pixels_mut() {
        let fade = STRIP_OPACITY * (1.0 - x as f32 / STRIP_WIDTH as f32);
        pixel.0[3] = (255.0 * fade).round() as u8;
    }
    round_corners(&mut strip, STRIP_RADIUS, Corners::LEFT);
    Ok(into_buffer(&strip))
}

#[derive(Clone, Copy)]
pub(crate) struct Corners {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl Corners {
    pub const ALL: Corners = Corners {
        left: true,
        right: true,
        top: true,
        bottom: true,
    };
    pub const LEFT: Corners = Corners {
        left: true,
        right: false,
        top: true,
        bottom: true,
    };
    pub const BOTTOM: Corners = Corners {
        left: true,
        right: true,
        top: false,
        bottom: true,
    };
}

pub(crate) fn round_corners(image: &mut RgbaImage, radius: f32, corners: Corners) {
    let (width, height) = (image.width() as f32, image.height() as f32);
    let reach = |near: bool, far: bool, at: f32, size: f32| {
        let before = if near { radius - at } else { 0.0 };
        let after = if far { at - (size - radius) } else { 0.0 };
        before.max(after).max(0.0)
    };
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let dx = reach(corners.left, corners.right, x as f32 + 0.5, width);
        let dy = reach(corners.top, corners.bottom, y as f32 + 0.5, height);
        let outside = (dx * dx + dy * dy).sqrt() - radius;
        let coverage = (0.5 - outside).clamp(0.0, 1.0);
        pixel.0[3] = (f32::from(pixel.0[3]) * coverage).round() as u8;
    }
}

pub fn rounded_square(
    image: &[u8],
    size: u32,
    radius: f32,
) -> Result<SharedPixelBuffer<Rgba8Pixel>> {
    let decoded = image::load_from_memory(image)
        .context("cannot decode the avatar")?
        .to_rgba8();
    let side = decoded.width().min(decoded.height());
    let square = crop_centre(&decoded, side, side);
    let mut scaled = imageops::resize(&square, size, size, FilterType::Lanczos3);
    round_corners(&mut scaled, radius, Corners::ALL);
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
            .timeout_global(Some(TIMEOUT))
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

pub struct CachedCovers<'a>(pub &'a mut dyn Fetch);

impl Fetch for CachedCovers<'_> {
    fn get(&mut self, url: &str) -> Result<Option<Vec<u8>>> {
        if url.starts_with(COVER_HOST) {
            return Ok(None);
        }
        self.0.get(url)
    }
}

pub struct Covers {
    pub found: HashMap<u32, Vec<u8>>,
    pub failed: usize,
}

pub fn covers(
    set_ids: &[u32],
    cache_dir: &Path,
    songs: Option<&Path>,
    fetch: &mut dyn Fetch,
    progress: &mut dyn FnMut(usize, usize),
) -> Covers {
    let mut covers = Covers {
        found: HashMap::new(),
        failed: 0,
    };
    let total = set_ids.len();
    let mut done = 0;
    let mut wanted = Vec::new();
    for &set_id in set_ids {
        if covers.found.contains_key(&set_id) || wanted.contains(&set_id) {
            done += 1;
            progress(done, total);
            continue;
        }
        let kept = fs::read(cover_file(cache_dir, set_id))
            .ok()
            .filter(|bytes| image::load_from_memory(bytes).is_ok());
        match kept {
            Some(bytes) => {
                covers.found.insert(set_id, bytes);
                done += 1;
                progress(done, total);
            }
            None => wanted.push(set_id),
        }
    }
    if let Some(songs) = songs {
        let folders = set_folders(songs);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(LOCAL_THREADS)
            .build()
            .ok();
        let mut remote = Vec::new();
        for chunk in wanted.chunks(LOCAL_CHUNK) {
            let cut = || -> Vec<(u32, Option<Vec<u8>>)> {
                chunk
                    .par_iter()
                    .map(|&set_id| (set_id, local_cover(&folders, set_id)))
                    .collect()
            };
            let made = match &pool {
                Some(pool) => pool.install(cut),
                None => chunk
                    .iter()
                    .map(|&set_id| (set_id, local_cover(&folders, set_id)))
                    .collect(),
            };
            for (set_id, bytes) in made {
                match bytes {
                    Some(bytes) => {
                        if keep(&cover_file(cache_dir, set_id), &bytes).is_err() {
                            covers.failed += 1;
                        }
                        covers.found.insert(set_id, bytes);
                        done += 1;
                        progress(done, total);
                    }
                    None => remote.push(set_id),
                }
            }
        }
        wanted = remote;
    }
    let mut offline = false;
    for set_id in wanted {
        let file = cover_file(cache_dir, set_id);
        let download = || {
            if offline {
                bail!("the network is not answering");
            }
            let answer = fetch.get(&cover_url(set_id));
            offline = answer.is_err();
            answer
        };
        match cached(Some(&file), download) {
            Ok(Some(bytes)) => {
                covers.found.insert(set_id, bytes);
            }
            Ok(None) => {}
            Err(_) => covers.failed += 1,
        }
        done += 1;
        progress(done, total);
    }
    covers
}

fn cover_file(cache_dir: &Path, set_id: u32) -> PathBuf {
    cache_dir.join(format!("{set_id}.jpg"))
}

fn set_folders(songs: &Path) -> HashMap<u32, Vec<PathBuf>> {
    let mut folders: HashMap<u32, Vec<PathBuf>> = HashMap::new();
    let Ok(entries) = fs::read_dir(songs) else {
        return folders;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let digits = name.len() - name.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let rest = &name[digits..];
        if digits == 0 || !(rest.is_empty() || rest.starts_with([' ', '_'])) {
            continue;
        }
        if let Ok(set_id) = name[..digits].parse() {
            folders.entry(set_id).or_default().push(entry.path());
        }
    }
    folders
}

fn local_cover(folders: &HashMap<u32, Vec<PathBuf>>, set_id: u32) -> Option<Vec<u8>> {
    folders.get(&set_id)?.iter().find_map(|folder| {
        let background = background_in(folder)?;
        if fs::metadata(&background).ok()?.len() > MAX_BACKGROUND_BYTES {
            return None;
        }
        let mut reader = image::ImageReader::open(background)
            .ok()?
            .with_guessed_format()
            .ok()?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_BACKGROUND_SIDE);
        limits.max_image_height = Some(MAX_BACKGROUND_SIDE);
        limits.max_alloc = Some(MAX_BACKGROUND_ALLOC);
        reader.limits(limits);
        cut_cover(&reader.decode().ok()?)
    })
}

fn background_in(folder: &Path) -> Option<PathBuf> {
    let files: Vec<PathBuf> = fs::read_dir(folder)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    let mut maps: Vec<&PathBuf> = files
        .iter()
        .filter(|file| {
            file.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("osu"))
        })
        .collect();
    maps.sort();
    maps.into_iter().find_map(|map| {
        let mut text = Vec::new();
        File::open(map)
            .ok()?
            .take(MAX_MAP_BYTES)
            .read_to_end(&mut text)
            .ok()?;
        let name = background_name(&String::from_utf8_lossy(&text))?;
        if !Path::new(&name)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        {
            return None;
        }
        let wanted = folder.join(&name);
        let found = if wanted.is_file() {
            wanted
        } else {
            files
                .iter()
                .find(|file| {
                    file.file_name()
                        .is_some_and(|file| file.to_string_lossy().eq_ignore_ascii_case(&name))
                })
                .cloned()?
        };
        inside(folder, &found).then_some(found)
    })
}

fn inside(folder: &Path, file: &Path) -> bool {
    match (fs::canonicalize(folder), fs::canonicalize(file)) {
        (Ok(folder), Ok(file)) => file.starts_with(folder),
        _ => false,
    }
}

pub(crate) fn background_name(map: &str) -> Option<String> {
    let mut events = false;
    for line in map.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            events = line.eq_ignore_ascii_case("[Events]");
            continue;
        }
        if !events || line.starts_with("//") {
            continue;
        }
        let mut fields = line.splitn(4, ',');
        let kind = fields.next().unwrap_or_default().trim();
        if !(kind == "0" || kind.eq_ignore_ascii_case("Background")) {
            continue;
        }
        if fields.next().map(str::trim) != Some("0") {
            continue;
        }
        let name = fields
            .next()
            .unwrap_or_default()
            .trim()
            .trim_matches('"')
            .trim();
        if !name.is_empty() {
            return Some(name.to_owned());
        }
    }
    None
}

fn cut_cover(picture: &DynamicImage) -> Option<Vec<u8>> {
    let (width, height) = (picture.width(), picture.height());
    if width == 0 || height == 0 {
        return None;
    }
    let (crop_width, crop_height) = if u64::from(width) * u64::from(COVER_HEIGHT)
        > u64::from(height) * u64::from(COVER_WIDTH)
    {
        (height * COVER_WIDTH / COVER_HEIGHT, height)
    } else {
        (width, width * COVER_HEIGHT / COVER_WIDTH)
    };
    let cover = picture
        .crop_imm(
            (width - crop_width) / 2,
            (height - crop_height) / 2,
            crop_width.max(1),
            crop_height.max(1),
        )
        .resize_exact(COVER_WIDTH, COVER_HEIGHT, FilterType::Triangle)
        .to_rgb8();
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, COVER_QUALITY)
        .encode_image(&cover)
        .ok()?;
    Some(bytes)
}

pub(crate) fn cached(
    file: Option<&Path>,
    download: impl FnOnce() -> Result<Option<Vec<u8>>>,
) -> Result<Option<Vec<u8>>> {
    if let Some(file) = file
        && let Ok(bytes) = fs::read(file)
    {
        if image::load_from_memory(&bytes).is_ok() {
            return Ok(Some(bytes));
        }
        let _ = fs::remove_file(file);
    }
    let Some(bytes) = download()? else {
        return Ok(None);
    };
    if image::load_from_memory(&bytes).is_err() {
        bail!("the download is not an image");
    }
    if let Some(file) = file {
        let _ = keep(file, &bytes);
    }
    Ok(Some(bytes))
}

fn keep(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    write_atomic(file, bytes)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Cursor;

    use anyhow::{Result, bail};
    use image::{ImageFormat, Rgb, RgbImage};

    use super::{
        CachedCovers, Fetch, STRIP_HEIGHT, STRIP_WIDTH, background_name, cover_strip, cover_url,
        covers, rounded_square,
    };

    fn songs_with(
        dir: &std::path::Path,
        folder: &str,
        events: &str,
        background: Option<(&str, Vec<u8>)>,
    ) {
        let folder = dir.join(folder);
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join("Artist - Title (Mapper) [Hard].osu"),
            format!("osu file format v14\n\n[General]\nAudioFilename: audio.mp3\n\n[Events]\n{events}\n\n[TimingPoints]\n0,500,4,2,0,100,1,0\n"),
        )
        .unwrap();
        if let Some((name, bytes)) = background {
            fs::write(folder.join(name), bytes).unwrap();
        }
    }

    #[test]
    fn a_background_is_found_in_the_events_of_a_map() {
        let map = |events: &str| {
            format!("osu file format v14\n[General]\nMode: 0\n[Events]\n{events}\n[TimingPoints]\n")
        };
        assert_eq!(
            background_name(&map("//Background and Video events\n0,0,\"bg.jpg\",0,0")).as_deref(),
            Some("bg.jpg")
        );
        assert_eq!(
            background_name(&map("0,0,bg 2.png")).as_deref(),
            Some("bg 2.png")
        );
        assert_eq!(
            background_name(&map("Background,0,\"wall.jpg\"")).as_deref(),
            Some("wall.jpg")
        );
        assert_eq!(
            background_name(&map("Video,0,\"clip.mp4\"\n0,0,\"bg.jpg\",0,0")).as_deref(),
            Some("bg.jpg")
        );
        assert_eq!(
            background_name(&map(
                "//0,0,\"old.jpg\"\nSprite,Foreground,Centre,\"s.png\",320,240"
            )),
            None
        );
        assert_eq!(
            background_name("osu file format v14\n[General]\n0,0,\"not-events.jpg\""),
            None
        );
    }

    #[test]
    fn a_cover_is_cut_from_the_local_background_without_the_network() {
        let songs = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        songs_with(
            songs.path(),
            "123 Artist - Title",
            "0,0,\"bg.png\",0,0",
            Some(("bg.png", png(1920, 1080))),
        );
        let mut fetch = Counting::new(jpeg(900, 250));
        let found = covers(
            &[123],
            cache.path(),
            Some(songs.path()),
            &mut fetch,
            &mut |_, _| {},
        );
        assert!(fetch.calls.is_empty(), "{:?}", fetch.calls);
        let cover = image::load_from_memory(&found.found[&123]).unwrap();
        assert_eq!((cover.width(), cover.height()), (900, 250));
        assert!(cache.path().join("123.jpg").is_file());
    }

    #[test]
    fn covers_cut_on_the_first_run_reach_the_image_step() {
        let songs = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("covers");
        songs_with(
            songs.path(),
            "123 Artist - Title",
            "0,0,\"bg.jpg\",0,0",
            Some(("bg.jpg", jpeg(1920, 1080))),
        );
        let mut fetch = Counting::new(jpeg(900, 250));
        let cut = covers(
            &[123],
            &cache,
            Some(songs.path()),
            &mut fetch,
            &mut |_, _| {},
        );
        assert_eq!(cut.failed, 0);
        let mut offline = Counting::new(Vec::new());
        let drawn = covers(
            &[123],
            &cache,
            None,
            &mut CachedCovers(&mut offline),
            &mut |_, _| {},
        );
        assert!(
            drawn.found.contains_key(&123),
            "the image step got no cover"
        );
        assert!(fetch.calls.is_empty() && offline.calls.is_empty());
    }

    #[test]
    fn a_cut_cover_that_cannot_be_kept_counts_as_failed() {
        let songs = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("file"), b"x").unwrap();
        songs_with(
            songs.path(),
            "123 Artist - Title",
            "0,0,\"bg.jpg\",0,0",
            Some(("bg.jpg", jpeg(1920, 1080))),
        );
        let mut fetch = Counting::new(jpeg(900, 250));
        let cut = covers(
            &[123],
            &root.path().join("file").join("covers"),
            Some(songs.path()),
            &mut fetch,
            &mut |_, _| {},
        );
        assert_eq!(cut.failed, 1);
    }

    #[test]
    fn a_background_named_like_another_format_still_gives_a_cover() {
        let songs = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        songs_with(
            songs.path(),
            "321 Artist - Title",
            "0,0,\"BG.jpg\",0,0",
            Some(("BG.jpg", png(1366, 768))),
        );
        let mut fetch = Counting::new(jpeg(900, 250));
        let found = covers(
            &[321],
            cache.path(),
            Some(songs.path()),
            &mut fetch,
            &mut |_, _| {},
        );
        assert!(fetch.calls.is_empty(), "{:?}", fetch.calls);
        assert!(found.found.contains_key(&321));
    }

    #[test]
    fn a_background_outside_the_map_folder_is_never_read() {
        let songs = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        fs::write(songs.path().join("private.png"), png(1920, 1080)).unwrap();
        let outside = songs.path().join("private.png");
        songs_with(
            songs.path(),
            "111 Artist - Title",
            "0,0,\"../private.png\",0,0",
            None,
        );
        songs_with(
            songs.path(),
            "222 Artist - Title",
            &format!("0,0,\"{}\",0,0", outside.display()),
            None,
        );
        let mut fetch = Counting::new(jpeg(900, 250));
        covers(
            &[111, 222],
            cache.path(),
            Some(songs.path()),
            &mut fetch,
            &mut |_, _| {},
        );
        assert_eq!(fetch.calls, [cover_url(111), cover_url(222)]);
    }

    #[test]
    fn a_background_too_large_to_decode_safely_is_skipped() {
        let songs = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        songs_with(
            songs.path(),
            "333 Artist - Title",
            "0,0,\"wide.png\",0,0",
            Some(("wide.png", png(20_000, 4))),
        );
        let mut fetch = Counting::new(jpeg(900, 250));
        covers(
            &[333],
            cache.path(),
            Some(songs.path()),
            &mut fetch,
            &mut |_, _| {},
        );
        assert_eq!(fetch.calls, [cover_url(333)]);
    }

    #[test]
    fn a_set_without_a_local_background_is_downloaded() {
        let songs = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        songs_with(
            songs.path(),
            "456 Artist - Title",
            "0,0,\"gone.jpg\",0,0",
            None,
        );
        let mut fetch = Counting::new(jpeg(900, 250));
        let found = covers(
            &[456, 789],
            cache.path(),
            Some(songs.path()),
            &mut fetch,
            &mut |_, _| {},
        );
        assert_eq!(fetch.calls, [cover_url(456), cover_url(789)]);
        assert_eq!(found.found.len(), 2);
    }

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
        let avatar = rounded_square(&png(300, 300), 128, 28.0).unwrap();
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
        let avatar = rounded_square(&png(300, 200), 208, 44.0).unwrap();
        assert_eq!((avatar.width(), avatar.height()), (208, 208));
    }

    #[test]
    fn cached_covers_are_not_downloaded_again() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("7.jpg"), jpeg(20, 10)).unwrap();
        let mut fetch = Counting::new(jpeg(30, 15));
        let mut seen = Vec::new();
        let found = covers(&[7, 8], dir.path(), None, &mut fetch, &mut |done, total| {
            seen.push((done, total))
        });
        assert_eq!(fetch.calls, vec![cover_url(8)]);
        assert_eq!(found.found.len(), 2);
        assert_eq!(found.found[&7], jpeg(20, 10));
        assert!(dir.path().join("8.jpg").exists());
        assert_eq!(seen.last(), Some(&(2, 2)));

        let mut again = Counting::new(Vec::new());
        let found = covers(&[7, 8], dir.path(), None, &mut again, &mut |_, _| {});
        assert!(again.calls.is_empty());
        assert_eq!(found.found.len(), 2);
        assert_eq!(found.failed, 0);
    }

    #[test]
    fn a_missing_cover_is_skipped_and_the_failure_counts_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let mut fetch = Counting::new(jpeg(30, 15));
        fetch.missing.push(cover_url(1));
        fetch.failing.push(cover_url(2));
        let found = covers(&[1, 2, 3], dir.path(), None, &mut fetch, &mut |_, _| {});
        assert!(found.found.is_empty());
        assert_eq!(found.failed, 2);
        assert!(!dir.path().join("1.jpg").exists());
        assert!(!dir.path().join("2.jpg").exists());
    }

    #[test]
    fn a_network_error_stops_the_downloads_and_serves_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("4.jpg"), jpeg(20, 10)).unwrap();
        let mut fetch = Counting::new(jpeg(30, 15));
        fetch.failing.push(cover_url(2));
        fetch.missing.push(cover_url(1));
        let found = covers(
            &[1, 2, 3, 4, 5],
            dir.path(),
            None,
            &mut fetch,
            &mut |_, _| {},
        );
        assert_eq!(fetch.calls, vec![cover_url(1), cover_url(2)]);
        assert_eq!(found.failed, 3);
        assert_eq!(found.found.keys().copied().collect::<Vec<_>>(), vec![4]);
    }

    #[test]
    fn cached_only_covers_never_reach_the_network() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("4.jpg"), jpeg(20, 10)).unwrap();
        let mut inner = Counting::new(jpeg(30, 15));
        let mut fetch = CachedCovers(&mut inner);
        let found = covers(&[3, 4], dir.path(), None, &mut fetch, &mut |_, _| {});
        assert_eq!(found.found.len(), 1);
        assert_eq!(found.failed, 0);
        let avatar = fetch.get("https://a.ppy.sh/7").unwrap();
        assert!(avatar.is_some());
        assert_eq!(inner.calls, vec!["https://a.ppy.sh/7".to_owned()]);
    }
}
