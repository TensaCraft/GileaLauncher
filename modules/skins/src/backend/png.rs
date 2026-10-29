//! Skin files: checked, normalised to a 64×64 RGBA PNG, the model guessed and a front view drawn.

use std::io::Cursor;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::{ImageFormat, Rgba, RgbaImage};
use launcher_shared::{AppError, AppResult, ErrorCode};
use sha2::{Digest, Sha256};

use crate::dto::{MESSAGE, SkinVariant};

/// The largest skin file taken.
pub const MAX_BYTES: usize = 64 * 1024;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// The front view's size in skin pixels, and how many picture pixels each skin pixel takes.
const FRONT: (u32, u32) = (16, 32);
const SCALE: u32 = 4;

/// A skin as the library keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkinImage {
    /// A 64×64 RGBA PNG.
    pub png: Vec<u8>,
    /// SHA-256 of `png`, lowercase hex.
    pub sha256: String,
}

fn bad_png(detail: impl ToString) -> AppError {
    AppError::new(ErrorCode::InvalidInput, detail.to_string()).with_param(MESSAGE, "skins_error_bad_png")
}

fn encode(image: &RgbaImage) -> AppResult<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Png).map_err(|e| AppError::internal(e.to_string()))?;
    Ok(out.into_inner())
}

fn decode(png: &[u8]) -> Option<RgbaImage> {
    image::load_from_memory_with_format(png, ImageFormat::Png).ok().map(|i| i.to_rgba8())
}

/// `bytes` as a skin: a PNG of 64×64, or of 64×32 (the old layout, widened as Minecraft does),
/// at most 64 KB, kept as a 64×64 RGBA PNG.
pub fn read_skin(bytes: &[u8]) -> AppResult<SkinImage> {
    if bytes.len() > MAX_BYTES {
        return Err(bad_png(format!("the file is {} bytes", bytes.len())));
    }
    if !bytes.starts_with(PNG_SIGNATURE) {
        return Err(bad_png("not a PNG"));
    }
    let image = image::load_from_memory_with_format(bytes, ImageFormat::Png).map_err(bad_png)?.to_rgba8();
    let image = match image.dimensions() {
        (64, 64) => image,
        (64, 32) => widen_legacy(&image),
        (w, h) => return Err(bad_png(format!("the skin is {w}x{h}"))),
    };
    let png = encode(&image)?;
    let sha256 = hex::encode(Sha256::digest(&png));
    Ok(SkinImage { png, sha256 })
}

/// Minecraft's reading of a 64×32 skin: the right leg and arm copied to the left, mirrored, and
/// a hat with no clear pixel made clear.
fn widen_legacy(old: &RgbaImage) -> RgbaImage {
    // (from x, from y, by x, by y, width, height), each copied mirrored left to right.
    const COPIES: [(u32, u32, i32, i32, u32, u32); 12] = [
        (4, 16, 16, 32, 4, 4),
        (8, 16, 16, 32, 4, 4),
        (0, 20, 24, 32, 4, 12),
        (4, 20, 16, 32, 4, 12),
        (8, 20, 8, 32, 4, 12),
        (12, 20, 16, 32, 4, 12),
        (44, 16, -8, 32, 4, 4),
        (48, 16, -8, 32, 4, 4),
        (40, 20, 0, 32, 4, 12),
        (44, 20, -8, 32, 4, 12),
        (48, 20, -16, 32, 4, 12),
        (52, 20, -8, 32, 4, 12),
    ];
    let mut image = RgbaImage::from_pixel(64, 64, Rgba([0, 0, 0, 0]));
    for (x, y, pixel) in old.enumerate_pixels() {
        image.put_pixel(x, y, *pixel);
    }
    for (x0, y0, dx, dy, w, h) in COPIES {
        for y in 0..h {
            for x in 0..w {
                let to_x = x0.saturating_add_signed(dx) + (w - 1 - x);
                let to_y = y0.saturating_add_signed(dy) + y;
                image.put_pixel(to_x, to_y, *old.get_pixel(x0 + x, y0 + y));
            }
        }
    }
    if (32..64).all(|x| (0..32).all(|y| image.get_pixel(x, y)[3] >= 128)) {
        for y in 0..16 {
            for x in 32..64 {
                image.get_pixel_mut(x, y)[3] = 0;
            }
        }
    }
    image
}

/// Alex's model when the right arm's fourth column is clear (pixel (54, 20)), else Steve's.
pub fn guess_variant(png: &[u8]) -> SkinVariant {
    match decode(png) {
        Some(image) if image.width() > 54 && image.height() > 20 && image.get_pixel(54, 20)[3] == 0 => {
            SkinVariant::Slim
        }
        _ => SkinVariant::Classic,
    }
}

/// A part of the front view: where it is on the skin and its overlay, where it goes, its size.
struct Part {
    from: (u32, u32),
    overlay: (u32, u32),
    to: (u32, u32),
    size: (u32, u32),
}

fn parts(variant: SkinVariant) -> [Part; 6] {
    let arm = if variant == SkinVariant::Slim { 3 } else { 4 };
    [
        Part { from: (8, 8), overlay: (40, 8), to: (4, 0), size: (8, 8) },
        Part { from: (20, 20), overlay: (20, 36), to: (4, 8), size: (8, 12) },
        // The player's right arm and leg are on the viewer's left.
        Part { from: (44, 20), overlay: (44, 36), to: (4 - arm, 8), size: (arm, 12) },
        Part { from: (36, 52), overlay: (52, 52), to: (12, 8), size: (arm, 12) },
        Part { from: (4, 20), overlay: (4, 36), to: (4, 20), size: (4, 12) },
        Part { from: (20, 52), overlay: (4, 52), to: (8, 20), size: (4, 12) },
    ]
}

/// `top` laid over `under`.
fn over(under: Rgba<u8>, top: Rgba<u8>) -> Rgba<u8> {
    let a = u32::from(top[3]);
    match a {
        255 => top,
        0 => under,
        _ => {
            let b = u32::from(under[3]) * (255 - a) / 255;
            let out = a + b;
            let mix = |c: usize| ((u32::from(top[c]) * a + u32::from(under[c]) * b) / out) as u8;
            Rgba([mix(0), mix(1), mix(2), out as u8])
        }
    }
}

/// The skin seen from the front with every overlay on top, four picture pixels to a skin pixel
/// (64×128), on a clear background. An unreadable skin draws as nothing.
pub fn thumbnail(png: &[u8], variant: SkinVariant) -> Vec<u8> {
    let mut front = RgbaImage::from_pixel(FRONT.0, FRONT.1, Rgba([0, 0, 0, 0]));
    if let Some(skin) = decode(png).filter(|s| s.dimensions() == (64, 64)) {
        for part in parts(variant) {
            for y in 0..part.size.1 {
                for x in 0..part.size.0 {
                    let base = *skin.get_pixel(part.from.0 + x, part.from.1 + y);
                    let top = *skin.get_pixel(part.overlay.0 + x, part.overlay.1 + y);
                    front.put_pixel(part.to.0 + x, part.to.1 + y, over(base, top));
                }
            }
        }
    }
    let (w, h) = (FRONT.0 * SCALE, FRONT.1 * SCALE);
    let big = image::imageops::resize(&front, w, h, image::imageops::FilterType::Nearest);
    encode(&big).unwrap_or_default()
}

/// `png` for an `<img src>` or the 3D viewer.
pub fn data_url(png: &[u8]) -> String {
    format!("data:image/png;base64,{}", STANDARD.encode(png))
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    use super::*;
    use crate::dto::MESSAGE;

    const RED: Rgba<u8> = Rgba([200, 20, 20, 255]);
    const SKIN: Rgba<u8> = Rgba([180, 140, 110, 255]);
    const CLEAR: Rgba<u8> = Rgba([0, 0, 0, 0]);

    fn encode(image: &RgbaImage) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    /// A skin with every base part filled and its fourth arm column filled (classic) or clear (slim).
    fn skin(width: u32, height: u32, slim: bool) -> RgbaImage {
        let mut image = RgbaImage::from_pixel(width, height, CLEAR);
        for (x0, y0, w, h) in [(0, 0, 32, 16), (0, 16, 56, 16)] {
            for y in y0..(y0 + h).min(height) {
                for x in x0..x0 + w {
                    image.put_pixel(x, y, SKIN);
                }
            }
        }
        if slim {
            // Alex's arm is three pixels wide: the column past its back is clear.
            for y in 20..32 {
                for x in 54..56 {
                    image.put_pixel(x, y, CLEAR);
                }
            }
        }
        image
    }

    fn bad_png(result: AppResult<SkinImage>) -> bool {
        result.is_err_and(|e| e.params.get(MESSAGE).map(String::as_str) == Some("skins_error_bad_png"))
    }

    #[test]
    fn a_64x64_skin_is_read_and_hashed() {
        let read = read_skin(&encode(&skin(64, 64, false))).unwrap();
        assert_eq!(image::load_from_memory(&read.png).unwrap().to_rgba8(), skin(64, 64, false));
        assert_eq!(read.sha256.len(), 64);
        assert!(read.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(read_skin(&read.png).unwrap(), read, "the normalised file reads as itself");
    }

    #[test]
    fn a_64x32_skin_becomes_64x64_with_mirrored_limbs() {
        let mut legacy = skin(64, 32, false);
        legacy.put_pixel(44, 20, RED); // the right arm's front, its left column
        legacy.put_pixel(4, 20, RED); // the right leg's front, its left column
        let read = read_skin(&encode(&legacy)).unwrap();
        let image = image::load_from_memory(&read.png).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (64, 64));
        assert_eq!(*image.get_pixel(39, 52), RED, "the left arm's front is the right one mirrored");
        assert_eq!(*image.get_pixel(23, 52), RED, "the left leg's front is the right one mirrored");
        assert_eq!(*image.get_pixel(36, 52), SKIN);
        assert_eq!(*image.get_pixel(0, 40), CLEAR, "the new overlays are clear");
    }

    #[test]
    fn an_opaque_legacy_hat_is_made_clear() {
        let mut legacy = skin(64, 32, false);
        for y in 0..32 {
            for x in 32..64 {
                legacy.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        let image = image::load_from_memory(&read_skin(&encode(&legacy)).unwrap().png).unwrap().to_rgba8();
        assert_eq!(image.get_pixel(40, 8)[3], 0, "Minecraft clears a hat with no clear pixel");
        assert_eq!(image.get_pixel(44, 20)[3], 255, "the arm stays");
        let kept = image::load_from_memory(&read_skin(&encode(&skin(64, 32, false))).unwrap().png).unwrap();
        assert_eq!(kept.to_rgba8().get_pixel(0, 0)[3], 255, "a hat with clear pixels is kept as it is");
    }

    #[test]
    fn a_wrong_size_is_refused() {
        assert!(bad_png(read_skin(&encode(&RgbaImage::new(32, 32)))));
        assert!(bad_png(read_skin(&encode(&RgbaImage::new(128, 128)))));
        assert!(bad_png(read_skin(&[])));
    }

    #[test]
    fn a_large_file_is_refused() {
        let mut big = encode(&skin(64, 64, false));
        big.resize(64 * 1024 + 1, 0);
        assert!(bad_png(read_skin(&big)));
    }

    #[test]
    fn a_renamed_file_is_refused() {
        let mut jpeg = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(skin(64, 64, false))
            .to_rgb8()
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        assert!(bad_png(read_skin(&jpeg.into_inner())));
    }

    #[test]
    fn odd_but_valid_pngs_are_normalised() {
        // An indexed (palette) PNG with a transparent entry and a text chunk.
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 64, 64);
            encoder.set_color(png::ColorType::Indexed);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_palette(vec![0, 0, 0, 180, 140, 110]);
            encoder.set_trns(vec![0, 255]);
            encoder.add_text_chunk("Comment".into(), "made by hand".into()).unwrap();
            let mut writer = encoder.write_header().unwrap();
            let pixels: Vec<u8> = (0..64 * 64).map(|i| u8::from(i % 64 < 32)).collect();
            writer.write_image_data(&pixels).unwrap();
        }
        let first = read_skin(&bytes).unwrap();
        let image = image::load_from_memory(&first.png).unwrap();
        assert_eq!(image.color(), image::ColorType::Rgba8);
        assert_eq!(image.to_rgba8().get_pixel(0, 0), &SKIN);
        assert_eq!(image.to_rgba8().get_pixel(40, 0)[3], 0);
        assert_eq!(read_skin(&bytes).unwrap().sha256, first.sha256, "the hash is stable");
        assert_eq!(read_skin(&first.png).unwrap().sha256, first.sha256, "and the same once normalised");
    }

    #[test]
    fn slim_is_guessed_from_the_arm() {
        assert_eq!(guess_variant(&encode(&skin(64, 64, true))), SkinVariant::Slim);
        assert_eq!(guess_variant(&encode(&skin(64, 64, false))), SkinVariant::Classic);
        assert_eq!(guess_variant(b"not a png"), SkinVariant::Classic);
    }

    #[test]
    fn the_thumbnail_is_a_front_view() {
        let mut face = skin(64, 64, false);
        face.put_pixel(9, 9, RED); // the face, one pixel in from its top left
        let thumb =
            image::load_from_memory(&thumbnail(&encode(&face), SkinVariant::Classic)).unwrap().to_rgba8();
        assert_eq!(thumb.dimensions(), (64, 128));
        assert_eq!(*thumb.get_pixel(4 * 5, 4), RED, "the face's pixel, scaled four times");
        assert_eq!(*thumb.get_pixel(4 * 5 + 3, 4 + 3), RED);
        assert_eq!(thumb.get_pixel(0, 0)[3], 0, "a corner is clear");
        assert_eq!(*thumb.get_pixel(0, 4 * 8), SKIN, "a classic arm reaches the edge");
        let slim = image::load_from_memory(&thumbnail(&encode(&face), SkinVariant::Slim)).unwrap().to_rgba8();
        assert_eq!(slim.get_pixel(0, 4 * 8)[3], 0, "a slim arm is a pixel narrower");
        assert_eq!(*slim.get_pixel(4, 4 * 8), SKIN);
    }

    #[test]
    fn overlays_are_drawn_on_top() {
        let mut hat = skin(64, 64, false);
        hat.put_pixel(41, 9, RED); // the hat's front, over the face's (9, 9)
        let thumb =
            image::load_from_memory(&thumbnail(&encode(&hat), SkinVariant::Classic)).unwrap().to_rgba8();
        assert_eq!(*thumb.get_pixel(4 * 5, 4), RED);
        assert_eq!(*thumb.get_pixel(4 * 6, 4), SKIN, "a clear overlay pixel shows the face");
    }

    #[test]
    fn a_data_url_is_base64() {
        assert_eq!(data_url(&[1, 2, 3]), "data:image/png;base64,AQID");
    }
}
