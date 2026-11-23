//! STM1 cover frames: crop, resize and RGB565 packing. Port of steam.py `build_frame`.
//!
//! Frame: "STM1" | u32 appid | u16 w | u16 h | u16 kind | u16 0 | w*h RGB565 (all little
//! endian), as firmware `on_steam_art` expects.

use anyhow::{Context, Result};
use image::imageops::{self, FilterType};

use crate::collectors::media::rgb_to_rgb565_le;

pub const KIND_HEADER: u16 = 0;
pub const KIND_THUMB: u16 = 1;
pub const KIND_PORTRAIT: u16 = 2;

/// Pixel size of a cover kind. The Home thumbnail is a small 2:3 portrait.
pub fn kind_size(kind: u16) -> (u32, u32) {
    match kind {
        KIND_HEADER => (340, 159),
        KIND_THUMB => (64, 96),
        _ => (96, 144),
    }
}

fn is_tall(kind: u16) -> bool {
    kind == KIND_THUMB || kind == KIND_PORTRAIT
}

/// Centre crop box `(x, y, w, h)` of an `iw` x `ih` image to width/height = `aspect`
/// (steam.py `_crop_to`; Python's round() is ties-to-even).
pub fn crop_box(iw: u32, ih: u32, aspect: f64) -> (u32, u32, u32, u32) {
    let (iwf, ihf) = (f64::from(iw), f64::from(ih));
    if iwf / ihf > aspect {
        let cw = ((ihf * aspect).round_ties_even() as u32).clamp(1, iw);
        ((iw - cw) / 2, 0, cw, ih)
    } else {
        let ch = ((iwf / aspect).round_ties_even() as u32).clamp(1, ih);
        (0, (ih - ch) / 2, iw, ch)
    }
}

/// The 16-byte STM1 header.
pub fn frame_header(appid: u32, w: u16, h: u16, kind: u16) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(b"STM1");
    out[4..8].copy_from_slice(&appid.to_le_bytes());
    out[8..10].copy_from_slice(&w.to_le_bytes());
    out[10..12].copy_from_slice(&h.to_le_bytes());
    out[12..14].copy_from_slice(&kind.to_le_bytes());
    // bytes 14..16 stay 0
    out
}

/// STM1 frame for one (appid, kind) from the raw header / portrait image bytes.
pub fn build_frame(
    appid: u32,
    kind: u16,
    header: Option<&[u8]>,
    portrait: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let (w, h) = kind_size(kind);
    let mut im = if is_tall(kind) && portrait.is_some() {
        image::load_from_memory(portrait.unwrap_or_default()).context("portrait image")?
    } else if is_tall(kind) {
        let im = image::load_from_memory(header.context("no image")?).context("header image")?;
        let (iw, ih) = (im.width(), im.height());
        // No portrait art: take the middle 2:3 slice of the header.
        let cw = iw
            .min((f64::from(ih) * 2.0 / 3.0).round_ties_even() as u32)
            .max(1);
        im.crop_imm((iw - cw) / 2, 0, cw, ih)
    } else {
        image::load_from_memory(header.context("no image")?).context("header image")?
    };
    let (x, y, cw, ch) = crop_box(im.width(), im.height(), f64::from(w) / f64::from(h));
    im = im.crop_imm(x, y, cw, ch);
    let rgb = imageops::resize(&im.to_rgb8(), w, h, FilterType::Lanczos3);
    let mut out = Vec::with_capacity(16 + (w * h * 2) as usize);
    out.extend_from_slice(&frame_header(appid, w as u16, h as u16, kind));
    out.extend_from_slice(&rgb_to_rgb565_le(rgb.as_raw()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgb, RgbImage};
    use std::io::Cursor;

    fn png(w: u32, h: u32, px: [u8; 3]) -> Vec<u8> {
        let img = RgbImage::from_pixel(w, h, Rgb(px));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn header_bytes_match_python_struct() {
        // struct.pack("<IHHHH", 570, 64, 96, 1, 0) after b"STM1"
        assert_eq!(
            frame_header(570, 64, 96, 1),
            [83, 84, 77, 49, 58, 2, 0, 0, 64, 0, 96, 0, 1, 0, 0, 0]
        );
        assert_eq!(
            frame_header(0xC123_4567, 340, 159, 0),
            [83, 84, 77, 49, 0x67, 0x45, 0x23, 0xC1, 0x54, 0x01, 0x9F, 0x00, 0, 0, 0, 0]
        );
    }

    #[test]
    fn kind_sizes() {
        assert_eq!(kind_size(KIND_HEADER), (340, 159));
        assert_eq!(kind_size(KIND_THUMB), (64, 96));
        assert_eq!(kind_size(KIND_PORTRAIT), (96, 144));
    }

    #[test]
    fn crop_boxes() {
        // Wider than the aspect: crop width. 460x215 header to 340:159.
        assert_eq!(crop_box(460, 215, 340.0 / 159.0), (0, 0, 460, 215));
        assert_eq!(crop_box(1000, 100, 2.0), (400, 0, 200, 100));
        // Taller than the aspect: crop height.
        assert_eq!(crop_box(100, 1000, 2.0), (0, 475, 100, 50));
        // Python round(): 2.5 -> 2 (ties to even), 3.5 -> 4.
        assert_eq!(crop_box(10, 5, 0.5), (4, 0, 2, 5));
        assert_eq!(crop_box(10, 7, 0.5), (3, 0, 4, 7));
        assert_eq!(crop_box(600, 900, 64.0 / 96.0), (0, 0, 600, 900));
    }

    #[test]
    fn frame_sizes_and_pixels() {
        let img = png(600, 900, [255, 0, 0]);
        for kind in [KIND_THUMB, KIND_PORTRAIT] {
            let f = build_frame(7, kind, None, Some(&img)).unwrap();
            let (w, h) = kind_size(kind);
            assert_eq!(f.len(), 16 + (w * h * 2) as usize);
            assert_eq!(&f[..4], b"STM1");
            // pure red in RGB565 little endian
            assert_eq!(&f[16..18], &[0x00, 0xF8]);
        }
        let hdr = png(460, 215, [0, 0, 255]);
        let f = build_frame(7, KIND_HEADER, Some(&hdr), None).unwrap();
        assert_eq!(f.len(), 16 + 340 * 159 * 2);
        assert_eq!(&f[16..18], &[0x1F, 0x00]);
    }

    #[test]
    fn tall_kind_falls_back_to_header_slice() {
        let hdr = png(460, 215, [0, 255, 0]);
        let f = build_frame(9, KIND_PORTRAIT, Some(&hdr), None).unwrap();
        assert_eq!(f.len(), 16 + 96 * 144 * 2);
        assert_eq!(&f[16..18], &[0xE0, 0x07]);
        assert!(build_frame(9, KIND_PORTRAIT, None, None).is_err());
        assert!(build_frame(9, KIND_HEADER, Some(b"junk"), None).is_err());
    }
}
