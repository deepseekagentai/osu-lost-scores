use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result, anyhow};
use image::{ImageFormat, RgbaImage};
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

use crate::share::art::Art;
use crate::share::model::{
    ShareOptions, ShareRow, badge_lost_line, badge_when_line, lost_rows, mods_width,
    potential_rows, stats, subtitle,
};
use crate::share::rasters::{self, Fetch, HttpFetch};
use crate::table::when_day;
use crate::view::{color, status_chip};
use crate::{ShareArt, ShareBadge, ShareItem, ShareLost, ShareMod, SharePotential};

pub const SCALE: f32 = 2.0;
pub const HEADER_AVATAR: (u32, u32) = (128, 28);
pub const BADGE_AVATAR: (u32, u32) = (208, 44);
pub const LOST_FILE: &str = "lost_scores.png";
pub const POTENTIAL_FILE: &str = "potential_top.png";
pub const BADGE_FILE: &str = "summary_badge.png";

type Pixels = SharedPixelBuffer<Rgba8Pixel>;

#[derive(Default)]
pub struct Prepared {
    pub strips: HashMap<u32, Pixels>,
    pub header_avatar: Option<Pixels>,
    pub badge_avatar: Option<Pixels>,
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
        .set_id
        .and_then(|set| prepared.strips.get(&set))
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
    lost.set_backdrop(art.backdrop_wide.clone());
    lost.set_avatar(avatar(&prepared.header_avatar));
    lost.set_art(share_art(&art));
    lost.set_mods_width(mods_width(&rows));
    lost.set_rows(items(&rows, prepared, &art));
    lost.set_nick(SharedString::from(scan.player_name.as_str()));
    lost.set_subtitle(subtitle(scan, options).into());
    lost.set_date(day.as_str().into());
    images.push(draw(&lost, 980.0, lost.get_image_height(), LOST_FILE)?);

    if let Some(potential) = &scan.potential {
        let rows = potential_rows(scan, options);
        let card = stats(potential);
        let top = SharePotential::new()?;
        top.set_backdrop(art.backdrop_wide.clone());
        top.set_avatar(avatar(&prepared.header_avatar));
        top.set_art(share_art(&art));
        top.set_mods_width(mods_width(&rows));
        top.set_rows(items(&rows, prepared, &art));
        top.set_nick(SharedString::from(scan.player_name.as_str()));
        top.set_date(day.as_str().into());
        top.set_pp_from(card.pp_from.as_str().into());
        top.set_pp_to(card.pp_to.as_str().into());
        top.set_pp_delta(card.pp_delta.as_str().into());
        top.set_accuracy_from(card.accuracy_from.as_str().into());
        top.set_accuracy_to(card.accuracy_to.as_str().into());
        top.set_accuracy_delta(card.accuracy_delta.as_str().into());
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
    let mut set_ids: Vec<u32> = lost_rows(scan, options)
        .iter()
        .chain(potential_rows(scan, options).iter())
        .filter_map(|row| row.set_id)
        .collect();
    set_ids.sort_unstable();
    set_ids.dedup();
    let strips = caches
        .covers
        .as_deref()
        .map(|dir| rasters::covers(&set_ids, dir, fetch, &mut |_, _| {}))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(set, bytes)| Some((set, rasters::cover_strip(&bytes).ok()?)))
        .collect();
    let bytes = crate::avatar::fetch(scan.player_id, caches.avatars.as_deref()).ok();
    let square = |(size, radius): (u32, u32)| {
        bytes
            .as_deref()
            .and_then(|bytes| rasters::rounded_square(bytes, size, radius).ok())
    };
    Prepared {
        strips,
        header_avatar: square(HEADER_AVATAR),
        badge_avatar: square(BADGE_AVATAR),
    }
}

fn png(rendered: &Rendered) -> Result<Vec<u8>> {
    let image = RgbaImage::from_raw(rendered.width, rendered.height, rendered.rgba.clone())
        .context("the rendered image has the wrong size")?;
    let mut bytes = Vec::new();
    image.write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)?;
    Ok(bytes)
}

pub fn render_with(
    folder: &Path,
    options: &ShareOptions,
    caches: &Caches,
    fetch: &mut dyn Fetch,
) -> Result<Vec<PathBuf>> {
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
    Ok(written)
}

pub fn render_all(folder: &Path, options: &ShareOptions) -> Result<Vec<PathBuf>> {
    render_with(folder, options, &Caches::standard(), &mut HttpFetch::new())
}

pub fn render_folder(folder: &Path) -> Result<Vec<PathBuf>> {
    let scan = history::load(folder)?;
    let settings = setup::config_file()
        .map(|path| setup::load_settings(&path))
        .unwrap_or_default();
    let options = ShareOptions {
        count: usize::from(settings.scores_to_show),
        include_all: scan.include_all,
        show_lost_in_top: settings.show_lost_in_top_image,
    };
    use_offscreen_platform()?;
    render_all(folder, &options)
}
