use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result, anyhow};
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};
use jiff::tz::TimeZone;
use lost_scores_app::history::{self, SavedScan};
use lost_scores_app::{cache, setup};
use lost_scores_core::files::write_atomic;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::{
    ComponentHandle, Image, ModelRc, PhysicalSize, PlatformError, Rgba8Pixel, SharedPixelBuffer,
    SharedString, VecModel,
};

use crate::avatar;
use crate::share::art::{self, Art};
use crate::share::model::{
    ShareOptions, ShareRow, badge_lost_line, badge_when_line, cover_keys, lost_empty_text,
    lost_rows, mods_width, potential_rows, stats, subtitle,
};
use crate::share::rasters::{self, CachedCovers, CoverKey, Fetch, HttpFetch};
use crate::table::when_day;
use crate::view::{color, status_chip};
use crate::{ShareArt, ShareBadge, ShareItem, ShareLost, ShareMod, SharePotential};

pub const FAILURE_PREFIX: &str = "cannot render the share images: ";
pub const SCALE: f32 = 2.0;
pub const HEADER_AVATAR: (u32, f32) = (128, 28.0);
pub const BADGE_AVATAR: (u32, f32) = (208, 44.0);
pub const LOST_FILE: &str = "lost_scores.png";
pub const POTENTIAL_FILE: &str = "potential_top.png";
pub const BADGE_FILE: &str = "summary_badge.png";

const IMAGES: [(&str, &str); 3] = [
    (BADGE_FILE, "Badge"),
    (LOST_FILE, "Lost Scores"),
    (POTENTIAL_FILE, "Potential Top"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareImage {
    pub label: &'static str,
    pub path: PathBuf,
}

pub fn images_in(folder: &Path) -> Vec<ShareImage> {
    IMAGES
        .iter()
        .map(|(file, label)| ShareImage {
            label,
            path: folder.join(file),
        })
        .filter(|image| image.path.is_file())
        .collect()
}

type Pixels = SharedPixelBuffer<Rgba8Pixel>;

#[derive(Default)]
pub struct Prepared {
    pub strips: HashMap<CoverKey, Pixels>,
    pub header_avatar: Option<Pixels>,
    pub badge_avatar: Option<Pixels>,
    pub covers_total: usize,
    pub covers_failed: usize,
    pub avatar_error: Option<String>,
}

impl Prepared {
    pub fn warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        if self.covers_failed > 0 {
            warnings.push(format!(
                "{} of {} covers could not be used",
                self.covers_failed, self.covers_total
            ));
        }
        if let Some(error) = &self.avatar_error {
            warnings.push(format!("The avatar could not be downloaded: {error}"));
        }
        warnings
    }
}

pub struct Output {
    pub paths: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

pub struct Rendered {
    pub file: &'static str,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct Caches {
    pub covers: Option<PathBuf>,
    pub avatars: Option<PathBuf>,
}

impl Caches {
    pub fn standard() -> Self {
        Self {
            covers: cache::covers_dir(),
            avatars: cache::avatars_dir(),
        }
    }
}

thread_local! {
    static LAST_WINDOW: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
}

struct Offscreen;

impl Platform for Offscreen {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        LAST_WINDOW.with(|slot| *slot.borrow_mut() = Some(window.clone()));
        Ok(window)
    }
}

pub fn use_offscreen_platform() -> Result<()> {
    slint::platform::set_platform(Box::new(Offscreen))
        .map_err(|error| anyhow!("cannot set up the offscreen renderer: {error}"))
}

fn draw<C: ComponentHandle>(
    component: &C,
    logical_width: f32,
    logical_height: f32,
    file: &'static str,
) -> Result<Rendered> {
    let window = LAST_WINDOW
        .with(|slot| slot.borrow_mut().take())
        .context("the offscreen renderer has not been set up")?;
    window.dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: SCALE,
    });
    component.show()?;
    let width = (logical_width * SCALE).round() as u32;
    let height = (logical_height * SCALE).round() as u32;
    window.set_size(PhysicalSize::new(width, height));
    for _ in 0..4 {
        slint::platform::update_timers_and_animations();
        window.request_redraw();
    }
    let mut buffer = vec![PremultipliedRgbaColor::default(); width as usize * height as usize];
    let drawn = window.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, width as usize);
    });
    component.hide()?;
    if !drawn {
        return Err(anyhow!("the renderer produced no frame"));
    }
    let mut rgba = Vec::with_capacity(buffer.len() * 4);
    for pixel in &buffer {
        let alpha = u32::from(pixel.alpha);
        let straight = |channel: u8| {
            (u32::from(channel) * 255 + alpha / 2)
                .checked_div(alpha)
                .map_or(0, |value| value.min(255) as u8)
        };
        rgba.extend_from_slice(&[
            straight(pixel.red),
            straight(pixel.green),
            straight(pixel.blue),
            pixel.alpha,
        ]);
    }
    Ok(Rendered {
        file,
        width,
        height,
        rgba,
    })
}

fn item(row: &ShareRow, prepared: &Prepared, art: &Art) -> ShareItem {
    let chip = status_chip(row.status);
    let cover = row
        .cover
        .as_ref()
        .and_then(|key| prepared.strips.get(key))
        .map(|strip| Image::from_rgba8(strip.clone()));
    let mods: Vec<ShareMod> = row
        .mods
        .iter()
        .map(|acronym| {
            let icon = art.module(acronym);
            ShareMod {
                has_icon: icon.is_some(),
                icon: icon.unwrap_or_default(),
                text: acronym.as_str().into(),
            }
        })
        .collect();
    ShareItem {
        place: row.place.as_str().into(),
        title: row.title.as_str().into(),
        artist: row.artist.as_str().into(),
        version: row.version.as_str().into(),
        creator: row.creator.as_str().into(),
        ago: row.ago.as_str().into(),
        accuracy: row.accuracy.as_str().into(),
        pp: row.pp.as_str().into(),
        sub: row.sub.as_str().into(),
        sub_red: row.sub_red.as_str().into(),
        lost: row.lost,
        grade: art.grade(row.grade),
        has_cover: cover.is_some(),
        cover: cover.unwrap_or_default(),
        mods: ModelRc::new(VecModel::from(mods)),
        status: chip.as_ref().map_or("", |chip| chip.text).into(),
        status_color: color(chip.as_ref().map_or(0, |chip| chip.background)),
        status_text: color(chip.as_ref().map_or(0, |chip| chip.ink)),
    }
}

fn items(rows: &[ShareRow], prepared: &Prepared, art: &Art) -> ModelRc<ShareItem> {
    let list: Vec<ShareItem> = rows.iter().map(|row| item(row, prepared, art)).collect();
    ModelRc::new(VecModel::from(list))
}

fn physical(logical: f32) -> u32 {
    (logical * SCALE).round() as u32
}

fn wide_backdrop(image_height: f32) -> Image {
    Image::from_rgba8(art::backdrop_wide(physical(image_height)))
}

fn backdrop_height(image_height: f32) -> f32 {
    physical(image_height).min(art::BACKDROP_WIDE_HEIGHT) as f32 / SCALE
}

fn avatar(buffer: &Option<Pixels>) -> Image {
    buffer.clone().map(Image::from_rgba8).unwrap_or_default()
}

fn share_art(art: &Art) -> ShareArt {
    ShareArt {
        lemon_small: art.lemon_small.clone(),
        lemon_large: art.lemon_large.clone(),
        clock: art.clock.clone(),
        trend_down: art.trend_down.clone(),
    }
}

pub fn render_images(
    scan: &SavedScan,
    options: &ShareOptions,
    prepared: &Prepared,
    zone: &TimeZone,
) -> Result<Vec<Rendered>> {
    let art = Art::load();
    let day = when_day(scan.made_at, zone);
    let mut images = Vec::new();

    let rows = lost_rows(scan, options);
    let lost = ShareLost::new()?;
    lost.set_avatar(avatar(&prepared.header_avatar));
    lost.set_art(share_art(&art));
    lost.set_mods_width(mods_width(&rows));
    lost.set_rows(items(&rows, prepared, &art));
    lost.set_empty_text(lost_empty_text(scan).into());
    lost.set_backdrop(wide_backdrop(lost.get_image_height()));
    lost.set_backdrop_height(backdrop_height(lost.get_image_height()));
    lost.set_nick(SharedString::from(scan.player_name.as_str()));
    lost.set_subtitle(subtitle(scan, options).into());
    lost.set_date(day.as_str().into());
    images.push(draw(&lost, 980.0, lost.get_image_height(), LOST_FILE)?);

    if let Some(potential) = &scan.potential {
        let rows = potential_rows(scan, options);
        let card = stats(potential);
        let top = SharePotential::new()?;
        top.set_avatar(avatar(&prepared.header_avatar));
        top.set_art(share_art(&art));
        top.set_mods_width(mods_width(&rows));
        top.set_rows(items(&rows, prepared, &art));
        top.set_backdrop(wide_backdrop(top.get_image_height()));
        top.set_backdrop_height(backdrop_height(top.get_image_height()));
        top.set_nick(SharedString::from(scan.player_name.as_str()));
        top.set_date(day.as_str().into());
        top.set_pp_from(card.pp_from.as_str().into());
        top.set_pp_to(card.pp_to.as_str().into());
        top.set_pp_delta(card.pp_delta.as_str().into());
        top.set_accuracy_from(card.accuracy_from.as_str().into());
        top.set_accuracy_to(card.accuracy_to.as_str().into());
        top.set_accuracy_delta(card.accuracy_delta.as_str().into());
        top.set_accuracy_rises(card.accuracy_rises);
        top.set_lost_in_top(card.lost_in_top.as_str().into());
        images.push(draw(&top, 980.0, top.get_image_height(), POTENTIAL_FILE)?);

        let badge = ShareBadge::new()?;
        badge.set_backdrop(art.backdrop_badge.clone());
        badge.set_avatar(avatar(&prepared.badge_avatar));
        badge.set_art(share_art(&art));
        badge.set_nick(SharedString::from(scan.player_name.as_str()));
        badge.set_pp_from(card.pp_from.as_str().into());
        badge.set_pp_to(card.pp_to.as_str().into());
        badge.set_pp_delta(card.pp_delta.as_str().into());
        badge.set_accuracy_from(card.accuracy_from.as_str().into());
        badge.set_accuracy_to(card.accuracy_to.as_str().into());
        badge.set_accuracy_delta(card.accuracy_delta.as_str().into());
        badge.set_accuracy_rises(card.accuracy_rises);
        badge.set_lost(badge_lost_line(scan, options).into());
        badge.set_rank_date(badge_when_line(scan, zone).into());
        images.push(draw(&badge, 520.0, badge.get_image_height(), BADGE_FILE)?);
    }
    Ok(images)
}

pub fn prepare(
    scan: &SavedScan,
    options: &ShareOptions,
    caches: &Caches,
    fetch: &mut dyn Fetch,
) -> Prepared {
    let keys = cover_keys(scan, options);
    let mut covers_failed = 0;
    let mut strips = HashMap::new();
    if let Some(dir) = caches.covers.as_deref() {
        let covers = rasters::covers(&keys, dir, None, fetch, &mut |_, _| {});
        covers_failed = covers.failed;
        for (key, bytes) in covers.found {
            match rasters::cover_strip(&bytes) {
                Ok(strip) => {
                    strips.insert(key, strip);
                }
                Err(_) => covers_failed += 1,
            }
        }
    }
    let mut avatar_error = None;
    let bytes = match avatar::fetch_via(scan.player_id, caches.avatars.as_deref(), fetch) {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            avatar_error = Some(format!("{error:#}"));
            None
        }
    };
    let mut square = |(size, radius): (u32, f32)| {
        let bytes = bytes.as_deref()?;
        match rasters::rounded_square(bytes, size, radius) {
            Ok(square) => Some(square),
            Err(error) => {
                avatar_error.get_or_insert(format!("{error:#}"));
                None
            }
        }
    };
    let header_avatar = square(HEADER_AVATAR);
    let badge_avatar = square(BADGE_AVATAR);
    Prepared {
        strips,
        header_avatar,
        badge_avatar,
        covers_total: keys.len(),
        covers_failed,
        avatar_error,
    }
}

fn png(rendered: &Rendered) -> Result<Vec<u8>> {
    let expected = u64::from(rendered.width) * u64::from(rendered.height) * 4;
    if rendered.rgba.len() as u64 != expected {
        return Err(anyhow!("the rendered image has the wrong size"));
    }
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes).write_image(
        &rendered.rgba,
        rendered.width,
        rendered.height,
        ExtendedColorType::Rgba8,
    )?;
    Ok(bytes)
}

pub fn render_with(
    folder: &Path,
    options: &ShareOptions,
    caches: &Caches,
    fetch: &mut dyn Fetch,
) -> Result<Output> {
    let scan = history::load(folder)?;
    let prepared = prepare(&scan, options, caches, fetch);
    let images = render_images(&scan, options, &prepared, &TimeZone::system())?;
    let mut written = Vec::new();
    for rendered in &images {
        let path = folder.join(rendered.file);
        write_atomic(&path, &png(rendered)?)
            .with_context(|| format!("cannot write {}", path.display()))?;
        written.push(path);
    }
    Ok(Output {
        paths: written,
        warnings: prepared.warnings(),
    })
}

pub fn render_all(folder: &Path, options: &ShareOptions, cached_covers: bool) -> Result<Output> {
    let mut http = HttpFetch::new();
    let caches = Caches::standard();
    if cached_covers {
        return render_with(folder, options, &caches, &mut CachedCovers(&mut http));
    }
    render_with(folder, options, &caches, &mut http)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub count: usize,
    pub show_lost_in_top: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderArgs {
    pub folder: PathBuf,
    pub cached: bool,
    pub choice: Option<Choice>,
}

impl RenderArgs {
    pub fn parse<I>(args: I) -> Result<Self, String>
    where
        I: IntoIterator,
        I::Item: Into<OsString>,
    {
        let mut args = args.into_iter().map(Into::into);
        let mut folder = None;
        let mut cached = false;
        let mut count = None;
        let mut show_lost_in_top = false;
        while let Some(arg) = args.next() {
            match arg.to_str().unwrap_or_default() {
                "--cached" => cached = true,
                "--show-lost-in-top" => show_lost_in_top = true,
                "--count" => {
                    let value = args
                        .next()
                        .map(|value| value.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    count = Some(
                        setup::parse_scores_to_show(&value)
                            .map(usize::from)
                            .ok_or_else(|| {
                                format!(
                                    "--count needs a number from {} to {}, got '{value}'",
                                    setup::SCORES_TO_SHOW.start(),
                                    setup::SCORES_TO_SHOW.end()
                                )
                            })?,
                    );
                }
                flag if flag.starts_with("--") => {
                    return Err(format!("render-images does not know {flag}"));
                }
                _ => folder = Some(PathBuf::from(arg)),
            }
        }
        let folder = folder
            .ok_or_else(|| "render-images needs the scan folder as its argument".to_owned())?;
        if show_lost_in_top && count.is_none() {
            return Err("--show-lost-in-top needs --count".to_owned());
        }
        let choice = count.map(|count| Choice {
            count,
            show_lost_in_top,
        });
        Ok(Self {
            folder,
            cached,
            choice,
        })
    }
}

pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "the renderer stopped unexpectedly".to_owned())
}

pub fn guarded<T>(work: impl FnOnce() -> Result<T>) -> Result<T, String> {
    match panic::catch_unwind(AssertUnwindSafe(work)) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(format!("{error:#}")),
        Err(payload) => Err(panic_message(payload.as_ref())),
    }
}

pub fn render_folder(args: &RenderArgs) -> Result<Output> {
    let scan = history::load(&args.folder)?;
    let settings = setup::config_file()
        .map(|path| setup::load_settings(&path))
        .unwrap_or_default();
    let (count, show_lost_in_top) = match args.choice {
        Some(choice) => (choice.count, choice.show_lost_in_top),
        None => (
            usize::from(settings.scores_to_show),
            settings.show_lost_in_top_image,
        ),
    };
    let options = ShareOptions {
        count,
        include_all: scan.include_all,
        show_lost_in_top,
    };
    use_offscreen_platform()?;
    render_all(&args.folder, &options, args.cached)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        BADGE_FILE, Choice, LOST_FILE, POTENTIAL_FILE, Prepared, RenderArgs, Rendered, guarded,
        images_in, png,
    };

    fn parse(args: &[&str]) -> Result<RenderArgs, String> {
        RenderArgs::parse(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn render_images_reads_the_options_the_window_passes() {
        let parsed = parse(&["--cached", "--count", "3", "--show-lost-in-top", "/scans/x"]);
        assert_eq!(
            parsed,
            Ok(RenderArgs {
                folder: PathBuf::from("/scans/x"),
                cached: true,
                choice: Some(Choice {
                    count: 3,
                    show_lost_in_top: true
                }),
            })
        );
        let parsed = parse(&["--count", "12", "/scans/x"]).unwrap();
        assert!(!parsed.cached);
        assert_eq!(
            parsed.choice,
            Some(Choice {
                count: 12,
                show_lost_in_top: false
            })
        );
    }

    #[test]
    fn render_images_without_flags_keeps_the_saved_settings() {
        let parsed = parse(&["--cached", "/scans/x"]).unwrap();
        assert_eq!(parsed.choice, None);
        assert_eq!(parsed.folder, PathBuf::from("/scans/x"));
    }

    #[test]
    fn render_images_refuses_bad_arguments() {
        assert!(parse(&[]).is_err());
        assert!(parse(&["--cached"]).is_err());
        assert!(parse(&["--count", "0", "/scans/x"]).is_err());
        assert!(parse(&["--count", "251", "/scans/x"]).is_err());
        assert!(parse(&["--count", "250", "/scans/x"]).is_ok());
        assert!(parse(&["--count", "/scans/x"]).is_err());
        assert!(parse(&["--show-lost-in-top", "/scans/x"]).is_err());
        assert!(parse(&["--unknown", "/scans/x"]).is_err());
    }

    #[test]
    fn the_folder_lists_the_images_it_has_in_a_fixed_order() {
        let dir = tempfile::tempdir().unwrap();
        assert!(images_in(dir.path()).is_empty());
        std::fs::write(dir.path().join(BADGE_FILE), b"x").unwrap();
        std::fs::write(dir.path().join(LOST_FILE), b"x").unwrap();
        std::fs::write(dir.path().join("scan.json"), b"{}").unwrap();
        let found = images_in(dir.path());
        let labels: Vec<_> = found.iter().map(|image| image.label).collect();
        assert_eq!(labels, ["Badge", "Lost Scores"]);
        assert_eq!(found[1].path, dir.path().join(LOST_FILE));
        std::fs::write(dir.path().join(POTENTIAL_FILE), b"x").unwrap();
        let labels: Vec<_> = images_in(dir.path())
            .iter()
            .map(|image| image.label)
            .collect();
        assert_eq!(labels, ["Badge", "Lost Scores", "Potential Top"]);
    }

    #[test]
    fn warnings_name_what_was_skipped() {
        assert!(Prepared::default().warnings().is_empty());
        let prepared = Prepared {
            covers_total: 19,
            covers_failed: 3,
            avatar_error: Some("network down".into()),
            ..Prepared::default()
        };
        assert_eq!(
            prepared.warnings(),
            [
                "3 of 19 covers could not be used",
                "The avatar could not be downloaded: network down"
            ]
        );
    }

    #[test]
    fn a_panic_becomes_an_error_message() {
        let caught: Result<(), String> = guarded(|| panic!("boom {}", 7));
        assert_eq!(caught, Err("boom 7".to_owned()));
        let caught: Result<(), String> = guarded(|| panic!("plain"));
        assert_eq!(caught, Err("plain".to_owned()));
        let failed: Result<(), String> = guarded(|| Err(anyhow::anyhow!("no folder")));
        assert_eq!(failed, Err("no folder".to_owned()));
        assert_eq!(guarded(|| Ok(3)), Ok(3));
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_is_not_utf8_is_kept_as_it_is() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let folder = OsString::from_vec(b"/scans/\xff\xfe".to_vec());
        let parsed = RenderArgs::parse([OsString::from("--cached"), folder.clone()]).unwrap();
        assert_eq!(parsed.folder, PathBuf::from(folder));
        assert!(parsed.cached);
    }

    #[test]
    fn the_png_carries_the_pixels_of_the_frame() {
        let rendered = Rendered {
            file: LOST_FILE,
            width: 3,
            height: 2,
            rgba: (0..24).map(|value| value as u8 * 9).collect(),
        };
        let bytes = png(&rendered).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (3, 2));
        assert_eq!(decoded.as_raw(), &rendered.rgba);
        let short = Rendered {
            rgba: vec![0; 5],
            ..rendered
        };
        assert!(png(&short).is_err());
    }
}
