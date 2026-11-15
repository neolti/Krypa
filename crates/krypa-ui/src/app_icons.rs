//! Icons of installed apps: choosing one from the manifest list, decoding the bytes a host
//! fetched, and caching the textures.
//!
//! The bytes come from an untrusted server through the host (`krypa-engine-api`'s
//! `IconFetchRequest`; this crate does no networking). They are capped in size, decoded
//! with a hard limit on the pixel dimensions and the memory the decoder may allocate, and a
//! decoder panic is contained: every failure ends as "show the placeholder". Decoding is a
//! pure function so the shell can run it off the UI thread.

use std::collections::HashMap;
use std::io::Cursor;
use std::panic::{catch_unwind, AssertUnwindSafe};

use egui::{Color32, ColorImage, Context, Rect, TextureHandle, TextureId, TextureOptions};
use image::{ImageError, ImageFormat, ImageReader, Limits};
use krypa_apps::{IconPurpose, InstalledApp};
use url::Url;

/// Largest icon body accepted, in bytes. Manifest icons are 192 or 512 pixel PNGs of a
/// few tens of kilobytes; this leaves room for a heavy 512 pixel one and stops anything
/// that is plainly not an icon before it is decoded.
pub const MAX_ICON_BYTES: usize = 512 * 1024;

/// Largest width or height accepted, in pixels (512 is the largest size manifests
/// ask for; 512 x 512 x 4 bytes is 1 MiB of texture per icon).
pub const MAX_ICON_SIDE: u32 = 512;

/// The decoder may allocate at most this much (a few times one maximum-size RGBA image).
const MAX_DECODE_ALLOC: u64 = 8 * 1024 * 1024;

/// Share of a maskable icon (width and height, centred) that the W3C manifest spec
/// guarantees to be visible under any mask: the safe zone is a circle of radius 40%.
pub const MASKABLE_SAFE_ZONE: f32 = 0.8;

/// Share of the badge diameter an `any` icon is drawn at: the side of the square inscribed
/// in the circular badge, so no corner of the image pokes out of it.
pub const ANY_ICON_EXTENT: f32 = 0.7;

/// Why an icon could not be used. The card shows its placeholder for all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconDecodeError {
    /// More bytes than [`MAX_ICON_BYTES`], or more pixels per side than [`MAX_ICON_SIDE`].
    TooLarge,
    /// Not a PNG, damaged, or the decoder gave up (also if it panicked).
    Malformed,
    /// The host could not fetch it (network, status, redirect to `http`, too large).
    Unavailable,
}

/// Decodes a PNG icon into RGBA, within the limits above. Never panics: a malformed or
/// hostile file is an `Err`. Only PNG is accepted (the format `image` is built with here;
/// SVG, JPEG, WebP and ICO icons get the placeholder).
///
/// # Errors
/// [`IconDecodeError`].
pub fn decode_icon(bytes: &[u8]) -> Result<ColorImage, IconDecodeError> {
    if bytes.len() > MAX_ICON_BYTES {
        return Err(IconDecodeError::TooLarge);
    }
    // The decoder is third-party code reading hostile input: contain even a panic.
    catch_unwind(AssertUnwindSafe(|| decode_png(bytes))).unwrap_or(Err(IconDecodeError::Malformed))
}

fn decode_png(bytes: &[u8]) -> Result<ColorImage, IconDecodeError> {
    let mut limits = Limits::no_limits();
    limits.max_image_width = Some(MAX_ICON_SIDE);
    limits.max_image_height = Some(MAX_ICON_SIDE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
    reader.limits(limits);
    let image = reader.decode().map_err(|e| match e {
        ImageError::Limits(_) => IconDecodeError::TooLarge,
        _ => IconDecodeError::Malformed,
    })?;
    let rgba = image.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 || w > MAX_ICON_SIDE || h > MAX_ICON_SIDE {
        return Err(IconDecodeError::Malformed);
    }
    Ok(ColorImage::from_rgba_unmultiplied(
        [w as usize, h as usize],
        rgba.as_raw(),
    ))
}

/// The icon to show for an app and how to draw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconChoice {
    /// Where its bytes come from (`https`, guaranteed by `krypa-apps`).
    pub url: Url,
    /// Which kind it is.
    pub purpose: IconPurpose,
}

/// Picks the icon for a badge `target_px` pixels wide: the best `any` icon, else the best
/// `maskable` one (W3C: `any` icons are meant to be shown as they are; maskable ones are
/// full-bleed art that only works inside a mask). `monochrome` icons are never used, they
/// need a tint the card does not have. `None` if the app has no usable icon.
#[must_use]
pub fn choose_icon(app: &InstalledApp, target_px: u32) -> Option<IconChoice> {
    [IconPurpose::Any, IconPurpose::Maskable]
        .into_iter()
        .find_map(|purpose| {
            app.best_icon(target_px, purpose).map(|icon| IconChoice {
                url: icon.src().clone(),
                purpose,
            })
        })
}

/// The part of the image that is drawn: texture coordinates, and the share of the badge
/// diameter it fills.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IconLayout {
    /// Texture rectangle shown, in 0..1 coordinates.
    pub uv: Rect,
    /// Fraction of the badge diameter the image fills.
    pub extent: f32,
    /// The image is clipped to the badge circle (a mask), instead of drawn as a square.
    pub masked: bool,
}

/// How an icon of `purpose` is laid out in a round badge. A maskable icon shows only its
/// safe zone, scaled to fill the badge, so what the mask (here: the badge circle) cannot
/// cut is exactly what is shown; an `any` icon is shown whole, inside the circle.
#[must_use]
pub fn icon_layout(purpose: IconPurpose) -> IconLayout {
    let whole = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    match purpose {
        IconPurpose::Maskable => {
            let inset = (1.0 - MASKABLE_SAFE_ZONE) / 2.0;
            IconLayout {
                uv: Rect::from_min_max(
                    egui::pos2(inset, inset),
                    egui::pos2(1.0 - inset, 1.0 - inset),
                ),
                extent: 1.0,
                masked: true,
            }
        }
        IconPurpose::Any | IconPurpose::Monochrome => IconLayout {
            uv: whole,
            extent: ANY_ICON_EXTENT,
            masked: false,
        },
    }
}

/// An uploaded texture; equal when it is the same texture. (`TextureHandle` itself is
/// neither `Debug` nor `PartialEq`, which `AppState` needs.)
#[derive(Clone)]
struct GpuTexture(TextureHandle);

impl std::fmt::Debug for GpuTexture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GpuTexture({:?})", self.0.id())
    }
}

impl PartialEq for GpuTexture {
    fn eq(&self, other: &Self) -> bool {
        self.0.id() == other.0.id()
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Slot {
    /// A fetch was asked for.
    Requested,
    /// Decoded, not yet uploaded (upload needs the egui context).
    Decoded(ColorImage),
    /// On the GPU. The handle keeps the texture alive.
    Ready(GpuTexture),
    /// Nothing usable: the placeholder stays.
    Failed(IconDecodeError),
}

/// The icons known to the interface, by icon URL: what was asked for, what failed, and the
/// decoded textures (decoded once, never again per frame). Memory only: `krypa-storage`
/// keeps the icon URLs of an app but has no place for image bytes, so a restart fetches
/// them again.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct IconStore {
    slots: HashMap<Url, Slot>,
}

impl IconStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The icon URLs that need fetching now: those chosen for `apps` that were never
    /// requested. Each is returned once; the caller fetches it and answers with
    /// [`IconStore::finish`].
    pub fn wanted(&mut self, apps: &[InstalledApp], target_px: u32) -> Vec<Url> {
        let mut fetch = Vec::new();
        for choice in apps.iter().filter_map(|a| choose_icon(a, target_px)) {
            if !self.slots.contains_key(&choice.url) {
                self.slots.insert(choice.url.clone(), Slot::Requested);
                fetch.push(choice.url);
            }
        }
        fetch
    }

    /// Records the outcome of a fetch and decode. Ignored for a URL nobody asked for or
    /// already answered, so a late or repeated answer cannot overwrite a good icon.
    pub fn finish(&mut self, url: &Url, result: Result<ColorImage, IconDecodeError>) {
        if let Some(slot @ Slot::Requested) = self.slots.get_mut(url) {
            *slot = match result {
                Ok(image) => Slot::Decoded(image),
                Err(e) => Slot::Failed(e),
            };
        }
    }

    /// The reason an icon failed, for the shell to log.
    #[must_use]
    pub fn failure(&self, url: &Url) -> Option<IconDecodeError> {
        match self.slots.get(url) {
            Some(Slot::Failed(e)) => Some(*e),
            _ => None,
        }
    }

    /// Uploads decoded icons as textures. Call once per frame before drawing.
    pub fn upload(&mut self, ctx: &Context) {
        for (url, slot) in &mut self.slots {
            if let Slot::Decoded(image) = slot {
                let image =
                    std::mem::replace(image, ColorImage::filled([1, 1], Color32::TRANSPARENT));
                let handle =
                    ctx.load_texture(format!("app-icon:{url}"), image, TextureOptions::LINEAR);
                *slot = Slot::Ready(GpuTexture(handle));
            }
        }
    }

    /// The texture of an icon, once it is uploaded.
    #[must_use]
    pub fn texture(&self, url: &Url) -> Option<TextureId> {
        match self.slots.get(url) {
            Some(Slot::Ready(texture)) => Some(texture.0.id()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use krypa_apps::Manifest;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 120, 240, 255]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn app(icons_json: &str) -> InstalledApp {
        let doc = Url::parse("https://app.example/").unwrap();
        let manifest_url = Url::parse("https://app.example/manifest.json").unwrap();
        let m = Manifest::parse(&format!(r#"{{"name":"A","icons":{icons_json}}}"#)).unwrap();
        InstalledApp::from_manifest(&doc, &manifest_url, &m).unwrap()
    }

    #[test]
    fn a_valid_png_decodes_to_its_pixels() {
        let img = decode_icon(&png(48, 32)).unwrap();
        assert_eq!(img.size, [48, 32]);
        assert_eq!(img.pixels[0], Color32::from_rgb(10, 120, 240));
    }

    /// Why: the decoder must not be handed an image that would cost far more memory than an
    /// icon is worth, however small the file is.
    #[test]
    fn oversize_dimensions_and_oversize_files_become_errors_not_textures() {
        assert!(decode_icon(&png(MAX_ICON_SIDE, MAX_ICON_SIDE)).is_ok());
        assert_eq!(
            decode_icon(&png(MAX_ICON_SIDE + 1, 1)),
            Err(IconDecodeError::TooLarge)
        );
        assert_eq!(
            decode_icon(&png(1, MAX_ICON_SIDE + 1)),
            Err(IconDecodeError::TooLarge)
        );
        assert_eq!(
            decode_icon(&vec![0; MAX_ICON_BYTES + 1]),
            Err(IconDecodeError::TooLarge)
        );
    }

    /// Why: icon bytes are hostile; no input may panic the UI.
    #[test]
    fn malformed_images_are_errors_not_panics() {
        let good = png(16, 16);
        let mut cases: Vec<Vec<u8>> = vec![
            Vec::new(),
            b"not an image".to_vec(),
            b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec(),
            b"\x89PNG\r\n\x1a\n".to_vec(),
            vec![0xff; 4096],
            good[..good.len() / 2].to_vec(),
            good[..33].to_vec(),
        ];
        let mut flipped = good.clone();
        for i in (20..flipped.len()).step_by(7) {
            flipped[i] ^= 0xa5;
        }
        cases.push(flipped);
        for (i, bytes) in cases.iter().enumerate() {
            assert!(decode_icon(bytes).is_err(), "case {i}");
        }
    }

    #[test]
    fn any_is_preferred_over_maskable_even_when_the_maskable_size_fits_better() {
        let a = app(r#"[{"src":"/m.png","sizes":"128x128","purpose":"maskable"},
                {"src":"/a.png","sizes":"96x96","purpose":"any"}]"#);
        let c = choose_icon(&a, 128).unwrap();
        assert_eq!(c.purpose, IconPurpose::Any);
        assert_eq!(c.url.path(), "/a.png");
    }

    #[test]
    fn maskable_is_the_fallback_and_monochrome_is_never_used() {
        let only_maskable = app(r#"[{"src":"/m.png","sizes":"192x192","purpose":"maskable"}]"#);
        assert_eq!(
            choose_icon(&only_maskable, 132).unwrap().purpose,
            IconPurpose::Maskable
        );
        let only_mono = app(r#"[{"src":"/g.png","sizes":"192x192","purpose":"monochrome"}]"#);
        assert_eq!(choose_icon(&only_mono, 132), None);
        assert_eq!(choose_icon(&app("[]"), 132), None);
    }

    #[test]
    fn the_best_size_for_the_badge_wins_among_any_icons() {
        let a = app(
            r#"[{"src":"/16.png","sizes":"16x16"},{"src":"/192.png","sizes":"192x192"},
                {"src":"/512.png","sizes":"512x512"},{"src":"/96.png","sizes":"96x96"}]"#,
        );
        // 3x display: a 44 point badge is 132 pixels: smallest that is large enough.
        assert_eq!(choose_icon(&a, 132).unwrap().url.path(), "/192.png");
        assert_eq!(choose_icon(&a, 400).unwrap().url.path(), "/512.png");
        assert_eq!(choose_icon(&a, 1000).unwrap().url.path(), "/512.png");
    }

    /// Why: a maskable icon's edges are bleed the mask may cut; showing them makes the
    /// badge look wrong. Only the safe zone may be shown, and an `any` icon must be whole.
    #[test]
    fn a_maskable_icon_shows_only_its_safe_zone_and_an_any_icon_all_of_it() {
        let m = icon_layout(IconPurpose::Maskable);
        assert!(m.masked);
        assert!((m.uv.width() - MASKABLE_SAFE_ZONE).abs() < 1e-6);
        assert!((m.uv.center().x - 0.5).abs() < 1e-6 && (m.uv.center().y - 0.5).abs() < 1e-6);
        let a = icon_layout(IconPurpose::Any);
        assert!(!a.masked);
        assert_eq!((a.uv.width(), a.uv.height()), (1.0, 1.0));
        assert!(
            a.extent * std::f32::consts::SQRT_2 <= 1.0 + 1e-6,
            "the whole square stays inside the circle"
        );
    }

    #[test]
    fn each_icon_is_requested_once_and_a_late_answer_cannot_replace_a_good_one() {
        let apps = vec![app(r#"[{"src":"/a.png","sizes":"192x192"}]"#)];
        let mut store = IconStore::new();
        let wanted = store.wanted(&apps, 132);
        assert_eq!(wanted.len(), 1);
        assert!(
            store.wanted(&apps, 132).is_empty(),
            "no second fetch while one is out or done"
        );
        let url = wanted[0].clone();
        store.finish(&url, Ok(decode_icon(&png(8, 8)).unwrap()));
        store.finish(&url, Err(IconDecodeError::Malformed));
        assert_eq!(store.failure(&url), None);
        let ctx = Context::default();
        store.upload(&ctx);
        assert!(store.texture(&url).is_some());
        // Decoded once: a second upload changes nothing.
        let id = store.texture(&url);
        store.upload(&ctx);
        assert_eq!(store.texture(&url), id);
        // An answer for a URL nobody asked about is ignored.
        let other = Url::parse("https://x.example/y.png").unwrap();
        store.finish(&other, Ok(decode_icon(&png(8, 8)).unwrap()));
        assert!(store.texture(&other).is_none());
    }

    #[test]
    fn a_failed_icon_stays_failed_and_has_no_texture() {
        let apps = vec![app(r#"[{"src":"/a.png","sizes":"192x192"}]"#)];
        let mut store = IconStore::new();
        let url = store.wanted(&apps, 132).remove(0);
        store.finish(&url, Err(IconDecodeError::TooLarge));
        store.upload(&Context::default());
        assert_eq!(store.texture(&url), None);
        assert_eq!(store.failure(&url), Some(IconDecodeError::TooLarge));
        assert!(store.wanted(&apps, 132).is_empty(), "no retry loop");
    }
}
