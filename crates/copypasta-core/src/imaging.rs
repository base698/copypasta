//! Image normalization: everything is stored as PNG, with a 64-bit
//! difference hash so re-encoded copies of the same picture still match.

use image::{imageops::FilterType, DynamicImage, ImageFormat};
use std::io::Cursor;

pub struct Normalized {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Decode PNG/TIFF/JPEG/... and re-encode as PNG (PNG input is kept as-is).
pub fn to_png(bytes: &[u8]) -> anyhow::Result<Normalized> {
    let format = image::guess_format(bytes)?;
    let img = image::load_from_memory(bytes)?;
    let (width, height) = (img.width(), img.height());
    let png = if format == ImageFormat::Png {
        bytes.to_vec()
    } else {
        encode_png(&img)?
    };
    Ok(Normalized { png, width, height })
}

fn encode_png(img: &DynamicImage) -> anyhow::Result<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png)?;
    Ok(out.into_inner())
}

pub fn dhash(png: &[u8]) -> Option<u64> {
    let img = image::load_from_memory(png).ok()?;
    let small = img.resize_exact(9, 8, FilterType::Triangle).to_luma8();
    let mut h = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            h <<= 1;
            if small.get_pixel(x, y)[0] > small.get_pixel(x + 1, y)[0] {
                h |= 1;
            }
        }
    }
    Some(h)
}

/// Small PNG for list views (max 320px on the long side).
pub fn thumbnail(png: &[u8], max: u32) -> anyhow::Result<Vec<u8>> {
    let img = image::load_from_memory(png)?;
    if img.width() <= max && img.height() <= max {
        return Ok(png.to_vec());
    }
    encode_png(&img.thumbnail(max, max))
}
