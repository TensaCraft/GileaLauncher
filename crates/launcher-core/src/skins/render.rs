//! Skin pictures, drawn without the network: a skin file checked and normalised to a 64×64 RGBA
//! PNG, its model guessed, its head and the front of its body drawn.

use std::io::Cursor;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::{ImageFormat, Rgba, RgbaImage};
use launcher_shared::{AppError, AppResult, ErrorCode};
use sha2::{Digest, Sha256};

/// The largest skin file taken.
pub const MAX_BYTES: usize = 64 * 1024;
/// The parameter `normalise` names why a file is no skin with: `bad_png`.
pub const REASON: &str = "reason";
pub const BAD_PNG: &str = "bad_png";
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// The front view's size in skin pixels, and how many picture pixels each skin pixel takes.
const FRONT: (u32, u32) = (16, 32);
const SCALE: u32 = 4;
/// The largest head drawn, in pixels.
const HEAD_MAX: u32 = 512;
/// Where the face and the hat over it are on a skin.
const FACE: (u32, u32) = (8, 8);
const HAT: (u32, u32) = (40, 8);

fn bad_png(detail: impl ToString) -> AppError {
    AppError::new(ErrorCode::InvalidInput, detail.to_string()).with_param(REASON, BAD_PNG)
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
/// at most 64 KB, kept as a 64×64 RGBA PNG whose clear pixels have no colour. Anything else is
/// `InvalidInput` with `reason` = `bad_png`.
pub fn normalise(bytes: &[u8]) -> AppResult<Vec<u8>> {
    if bytes.len() > MAX_BYTES {
        return Err(bad_png(format!("the file is {} bytes", bytes.len())));
    }
    if !bytes.starts_with(PNG_SIGNATURE) {
        return Err(bad_png("not a PNG"));
    }
    let image = image::load_from_memory_with_format(bytes, ImageFormat::Png).map_err(bad_png)?.to_rgba8();
    let mut image = match image.dimensions() {
        (64, 64) => image,
        (64, 32) => widen_legacy(&image),
        (w, h) => return Err(bad_png(format!("the skin is {w}x{h}"))),
    };
    // What nobody sees has no colour: a host that re-encodes the skin keeps its hash.
    for pixel in image.pixels_mut().filter(|p| p[3] == 0) {
        *pixel = Rgba([0, 0, 0, 0]);
    }
    encode(&image)
}

/// SHA-256 of `png`, lowercase hex.
pub fn sha256(png: &[u8]) -> String {
    hex::encode(Sha256::digest(png))
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
pub fn guess_slim(png: &[u8]) -> bool {
    decode(png)
        .is_some_and(|image| image.width() > 54 && image.height() > 20 && image.get_pixel(54, 20)[3] == 0)
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

/// `skin`'s part at `at`, `size` skin pixels, with its overlay at `overlay` on top.
fn layered(skin: &RgbaImage, at: (u32, u32), overlay: (u32, u32), size: (u32, u32)) -> RgbaImage {
    RgbaImage::from_fn(size.0, size.1, |x, y| {
        over(*skin.get_pixel(at.0 + x, at.1 + y), *skin.get_pixel(overlay.0 + x, overlay.1 + y))
    })
}

/// `image` stretched to `width`×`height`, each pixel a block of them, as a PNG.
fn scaled(image: &RgbaImage, width: u32, height: u32) -> Vec<u8> {
    let big = image::imageops::resize(image, width, height, image::imageops::FilterType::Nearest);
    encode(&big).unwrap_or_default()
}

/// The face with the hat over it, `size`×`size` pixels (1 to 512), each skin pixel a block of
/// them. An unreadable skin draws as a clear square.
pub fn head(png: &[u8], size: u32) -> Vec<u8> {
    let size = size.clamp(1, HEAD_MAX);
    let face = match decode(png).filter(|s| s.dimensions() == (64, 64)) {
        Some(skin) => layered(&skin, FACE, HAT, (8, 8)),
        None => RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 0])),
    };
    scaled(&face, size, size)
}

/// A part of the front view: where it is on the skin and its overlay, where it goes, its size.
struct Part {
    from: (u32, u32),
    overlay: (u32, u32),
    to: (u32, u32),
    size: (u32, u32),
}

fn parts(slim: bool) -> [Part; 6] {
    let arm = if slim { 3 } else { 4 };
    [
        Part { from: FACE, overlay: HAT, to: (4, 0), size: (8, 8) },
        Part { from: (20, 20), overlay: (20, 36), to: (4, 8), size: (8, 12) },
        // The player's right arm and leg are on the viewer's left.
        Part { from: (44, 20), overlay: (44, 36), to: (4 - arm, 8), size: (arm, 12) },
        Part { from: (36, 52), overlay: (52, 52), to: (12, 8), size: (arm, 12) },
        Part { from: (4, 20), overlay: (4, 36), to: (4, 20), size: (4, 12) },
        Part { from: (20, 52), overlay: (4, 52), to: (8, 20), size: (4, 12) },
    ]
}

/// The skin seen from the front with every overlay on top, four picture pixels to a skin pixel
/// (64×128), on a clear background. An unreadable skin draws as nothing.
pub fn front(png: &[u8], slim: bool) -> Vec<u8> {
    let mut view = RgbaImage::from_pixel(FRONT.0, FRONT.1, Rgba([0, 0, 0, 0]));
    if let Some(skin) = decode(png).filter(|s| s.dimensions() == (64, 64)) {
        for part in parts(slim) {
            let drawn = layered(&skin, part.from, part.overlay, part.size);
            image::imageops::replace(&mut view, &drawn, i64::from(part.to.0), i64::from(part.to.1));
        }
    }
    scaled(&view, FRONT.0 * SCALE, FRONT.1 * SCALE)
}

/// `png` for an `<img src>` or the 3D viewer.
pub fn data_url(png: &[u8]) -> String {
    format!("data:image/png;base64,{}", STANDARD.encode(png))
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    use super::*;

    const RED: Rgba<u8> = Rgba([200, 20, 20, 255]);
    const BLUE: Rgba<u8> = Rgba([20, 40, 220, 255]);
    const SKIN: Rgba<u8> = Rgba([180, 140, 110, 255]);
    const CLEAR: Rgba<u8> = Rgba([0, 0, 0, 0]);

    fn encode(image: &RgbaImage) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn pixels(png: &[u8]) -> RgbaImage {
        image::load_from_memory(png).unwrap().to_rgba8()
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

    fn is_bad_png<T>(result: AppResult<T>) -> bool {
        result.is_err_and(|e| {
            e.code == ErrorCode::InvalidInput && e.params.get(REASON).map(String::as_str) == Some(BAD_PNG)
        })
    }

    #[test]
    fn a_64x64_skin_is_kept_and_hashed() {
        let png = normalise(&encode(&skin(64, 64, false))).unwrap();
        assert_eq!(pixels(&png), skin(64, 64, false));
        let hash = sha256(&png);
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(normalise(&png).unwrap(), png, "the normalised file reads as itself");
    }

    #[test]
    fn a_64x32_skin_becomes_64x64_with_mirrored_limbs() {
        let mut legacy = skin(64, 32, false);
        legacy.put_pixel(44, 20, RED); // the right arm's front, its left column
        legacy.put_pixel(4, 20, RED); // the right leg's front, its left column
        let image = pixels(&normalise(&encode(&legacy)).unwrap());
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
        let image = pixels(&normalise(&encode(&legacy)).unwrap());
        assert_eq!(image.get_pixel(40, 8)[3], 0, "Minecraft clears a hat with no clear pixel");
        assert_eq!(image.get_pixel(44, 20)[3], 255, "the arm stays");
        let kept = pixels(&normalise(&encode(&skin(64, 32, false))).unwrap());
        assert_eq!(kept.get_pixel(0, 0)[3], 255, "a hat with clear pixels is kept as it is");
    }

    #[test]
    fn a_wrong_size_is_refused() {
        assert!(is_bad_png(normalise(&encode(&RgbaImage::new(32, 32)))));
        assert!(is_bad_png(normalise(&encode(&RgbaImage::new(128, 128)))));
        assert!(is_bad_png(normalise(&[])));
    }

    #[test]
    fn a_large_file_is_refused() {
        let mut big = encode(&skin(64, 64, false));
        big.resize(MAX_BYTES + 1, 0);
        assert!(is_bad_png(normalise(&big)));
    }

    #[test]
    fn a_renamed_file_is_refused() {
        let mut jpeg = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(skin(64, 64, false))
            .to_rgb8()
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        assert!(is_bad_png(normalise(&jpeg.into_inner())));
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
        let first = normalise(&bytes).unwrap();
        let image = image::load_from_memory(&first).unwrap();
        assert_eq!(image.color(), image::ColorType::Rgba8);
        assert_eq!(image.to_rgba8().get_pixel(0, 0), &SKIN);
        assert_eq!(image.to_rgba8().get_pixel(40, 0)[3], 0);
        assert_eq!(sha256(&normalise(&bytes).unwrap()), sha256(&first), "the hash is stable");
        assert_eq!(normalise(&first).unwrap(), first, "and the same once normalised");
    }

    #[test]
    fn clear_pixels_hash_alike_whatever_their_colour() {
        // A host that re-encodes a skin may drop the colour of the pixels nobody sees.
        let mut coloured = skin(64, 64, false);
        coloured.put_pixel(40, 0, Rgba([200, 30, 30, 0]));
        let png = normalise(&encode(&coloured)).unwrap();
        assert_eq!(sha256(&png), sha256(&normalise(&encode(&skin(64, 64, false))).unwrap()));
        assert_eq!(pixels(&png).get_pixel(40, 0), &CLEAR);
    }

    #[test]
    fn slim_is_guessed_from_the_arm() {
        assert!(guess_slim(&encode(&skin(64, 64, true))));
        assert!(!guess_slim(&encode(&skin(64, 64, false))));
        assert!(!guess_slim(b"not a png"));
    }

    #[test]
    fn the_head_is_the_face_scaled_up() {
        let mut face = skin(64, 64, false);
        face.put_pixel(9, 9, RED); // the face, one pixel in from its top left
        let small = pixels(&head(&encode(&face), 8));
        assert_eq!(small.dimensions(), (8, 8));
        assert_eq!(*small.get_pixel(1, 1), RED);
        assert_eq!(*small.get_pixel(0, 0), SKIN);
        let big = pixels(&head(&encode(&face), 64));
        assert_eq!(big.dimensions(), (64, 64));
        assert_eq!(*big.get_pixel(8, 8), RED, "each skin pixel is eight picture pixels");
        assert_eq!(*big.get_pixel(15, 15), RED);
        assert_eq!(*big.get_pixel(16, 16), SKIN);
        assert_eq!(*big.get_pixel(7, 7), SKIN);
    }

    #[test]
    fn the_hat_is_drawn_over_the_face() {
        let mut hat = skin(64, 64, false);
        hat.put_pixel(9, 9, RED);
        hat.put_pixel(41, 9, BLUE); // the hat's front, over the face's (9, 9)
        hat.put_pixel(42, 10, Rgba([0, 0, 255, 128])); // half see-through, over a plain face pixel
        let drawn = pixels(&head(&encode(&hat), 8));
        assert_eq!(*drawn.get_pixel(1, 1), BLUE);
        let blended = drawn.get_pixel(2, 2);
        assert_eq!(blended[3], 255);
        assert!(blended[2] > SKIN[2] && blended[0] < SKIN[0], "the face shows through: {blended:?}");
    }

    #[test]
    fn a_clear_hat_pixel_keeps_the_face() {
        let mut hat = skin(64, 64, false);
        hat.put_pixel(9, 9, RED);
        hat.put_pixel(41, 9, Rgba([0, 255, 0, 0])); // clear, whatever its colour
        assert_eq!(*pixels(&head(&encode(&hat), 8)).get_pixel(1, 1), RED);
    }

    #[test]
    fn an_unreadable_skin_draws_a_clear_head() {
        let drawn = pixels(&head(b"not a png", 16));
        assert_eq!(drawn.dimensions(), (16, 16));
        assert!(drawn.pixels().all(|p| p[3] == 0));
        assert_eq!(pixels(&head(&encode(&skin(64, 64, false)), 4096)).dimensions(), (512, 512));
        assert_eq!(pixels(&head(&encode(&skin(64, 64, false)), 0)).dimensions(), (1, 1));
    }

    #[test]
    fn the_front_is_the_whole_body() {
        let mut face = skin(64, 64, false);
        face.put_pixel(9, 9, RED); // the face, one pixel in from its top left
        let front_view = pixels(&front(&encode(&face), false));
        assert_eq!(front_view.dimensions(), (64, 128));
        assert_eq!(*front_view.get_pixel(4 * 5, 4), RED, "the face's pixel, scaled four times");
        assert_eq!(*front_view.get_pixel(4 * 5 + 3, 4 + 3), RED);
        assert_eq!(front_view.get_pixel(0, 0)[3], 0, "a corner is clear");
        assert_eq!(*front_view.get_pixel(0, 4 * 8), SKIN, "a classic arm reaches the edge");
        let slim = pixels(&front(&encode(&face), true));
        assert_eq!(slim.get_pixel(0, 4 * 8)[3], 0, "a slim arm is a pixel narrower");
        assert_eq!(*slim.get_pixel(4, 4 * 8), SKIN);
    }

    #[test]
    fn overlays_are_drawn_on_the_front() {
        let mut hat = skin(64, 64, false);
        hat.put_pixel(41, 9, RED); // the hat's front, over the face's (9, 9)
        let front_view = pixels(&front(&encode(&hat), false));
        assert_eq!(*front_view.get_pixel(4 * 5, 4), RED);
        assert_eq!(*front_view.get_pixel(4 * 6, 4), SKIN, "a clear overlay pixel shows the face");
    }

    #[test]
    fn a_data_url_is_base64() {
        assert_eq!(data_url(&[1, 2, 3]), "data:image/png;base64,AQID");
    }
}
