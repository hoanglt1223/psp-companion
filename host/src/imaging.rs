//! Small images for the PSP (cover art, Stream Deck keys): palettised to 8 bits and LZ4-compressed,
//! so a 76x86 key costs a few KB once and is then cached on the PSP by its ID.
//!
//! Format 1 (`IDX8_LZ4`), see protocol/PROTOCOL.md: `raw_len: u32` then an LZ4 block that expands to
//! `width: u16, height: u16, colors: u16, colors x RGB888, width*height x u8 index`.

use std::collections::HashMap;

use color_quant::NeuQuant;
use image::imageops::FilterType;
use image::{DynamicImage, RgbaImage};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const FORMAT_IDX8_LZ4: u8 = 1;
pub const MAX_DIM: u16 = 128;

#[derive(Debug, Error)]
pub enum ImageError {
    #[error("cannot decode image: {0}")]
    Decode(#[from] image::ImageError),
    #[error("image size {0}x{1} is outside 1-{MAX_DIM}")]
    Size(u32, u32),
    #[error("pixel buffer has {got} bytes, expected {want}")]
    Buffer { got: usize, want: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Indexed {
    pub width: u16,
    pub height: u16,
    pub palette: Vec<[u8; 3]>,
    pub indices: Vec<u8>,
}

fn check_size(w: u32, h: u32) -> Result<(u16, u16), ImageError> {
    if w == 0 || h == 0 || w > u32::from(MAX_DIM) || h > u32::from(MAX_DIM) {
        return Err(ImageError::Size(w, h));
    }
    Ok((w as u16, h as u16))
}

/// Exact palette when the image has at most 256 colours (typical for buttons), NeuQuant otherwise.
pub fn quantize_rgb(rgb: &[u8], width: u32, height: u32) -> Result<Indexed, ImageError> {
    let (w, h) = check_size(width, height)?;
    let want = width as usize * height as usize * 3;
    if rgb.len() != want {
        return Err(ImageError::Buffer { got: rgb.len(), want });
    }
    let mut exact: HashMap<[u8; 3], u8> = HashMap::new();
    let mut palette = Vec::new();
    let mut indices = Vec::with_capacity(rgb.len() / 3);
    for px in rgb.as_chunks::<3>().0 {
        let c = [px[0], px[1], px[2]];
        let next = palette.len();
        let idx = *exact.entry(c).or_insert_with(|| {
            palette.push(c);
            next.min(255) as u8
        });
        if palette.len() > 256 {
            break;
        }
        indices.push(idx);
    }
    if palette.len() <= 256 {
        return Ok(Indexed { width: w, height: h, palette, indices });
    }

    let rgba: Vec<u8> = rgb.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect();
    let nq = NeuQuant::new(10, 256, &rgba);
    let palette: Vec<[u8; 3]> = nq.color_map_rgb().as_chunks::<3>().0.iter().map(|c| [c[0], c[1], c[2]]).collect();
    let indices = rgba.as_chunks::<4>().0.iter().map(|p| nq.index_of(p) as u8).collect();
    Ok(Indexed { width: w, height: h, palette, indices })
}

impl Indexed {
    pub fn encode(&self) -> Vec<u8> {
        let mut raw = Vec::with_capacity(6 + self.palette.len() * 3 + self.indices.len());
        raw.extend_from_slice(&self.width.to_le_bytes());
        raw.extend_from_slice(&self.height.to_le_bytes());
        raw.extend_from_slice(&(self.palette.len() as u16).to_le_bytes());
        for c in &self.palette {
            raw.extend_from_slice(c);
        }
        raw.extend_from_slice(&self.indices);
        let mut out = (raw.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(&lz4_flex::block::compress(&raw));
        out
    }

    pub fn decode(payload: &[u8]) -> Option<Self> {
        let raw_len = u32::from_le_bytes(payload.get(..4)?.try_into().ok()?) as usize;
        if raw_len > 6 + 256 * 3 + usize::from(MAX_DIM) * usize::from(MAX_DIM) {
            return None;
        }
        let raw = lz4_flex::block::decompress(&payload[4..], raw_len).ok()?;
        let width = u16::from_le_bytes([raw[0], raw[1]]);
        let height = u16::from_le_bytes([raw[2], raw[3]]);
        let colors = usize::from(u16::from_le_bytes([raw[4], raw[5]]));
        let pal_end = 6 + colors * 3;
        if colors == 0 || colors > 256 || raw.len() != pal_end + usize::from(width) * usize::from(height) {
            return None;
        }
        let palette = raw[6..pal_end].as_chunks::<3>().0.iter().map(|c| [c[0], c[1], c[2]]).collect();
        let indices = raw[pal_end..].to_vec();
        if indices.iter().any(|&i| usize::from(i) >= colors) {
            return None;
        }
        Some(Self { width, height, palette, indices })
    }

    pub fn to_rgb(&self) -> Vec<u8> {
        self.indices.iter().flat_map(|&i| self.palette[usize::from(i)]).collect()
    }
}

/// Stable 64-bit ID of an encoded image; the PSP caches images under it.
pub fn image_id(payload: &[u8]) -> u64 {
    let digest = Sha256::digest(payload);
    // 0 means "no image" on the wire.
    u64::from_le_bytes(digest[..8].try_into().expect("8 bytes")).max(1)
}

/// Decodes any supported file format and fits it into `w`x`h` (aspect kept, centred on black).
pub fn fit_encoded(bytes: &[u8], w: u32, h: u32) -> Result<Indexed, ImageError> {
    let img = image::load_from_memory(bytes)?;
    fit(&img, w, h)
}

pub fn fit(img: &DynamicImage, w: u32, h: u32) -> Result<Indexed, ImageError> {
    check_size(w, h)?;
    let scaled = img.resize(w, h, FilterType::Triangle).to_rgba8();
    let mut canvas = RgbaImage::from_pixel(w, h, image::Rgba([0, 0, 0, 255]));
    let (x, y) = ((w - scaled.width()) / 2, (h - scaled.height()) / 2);
    image::imageops::overlay(&mut canvas, &scaled, i64::from(x), i64::from(y));
    let rgb: Vec<u8> = canvas.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
    quantize_rgb(&rgb, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| [(i % w * 255 / w) as u8, (i / w * 255 / h) as u8, ((i % w + i / w) * 255 / (w + h)) as u8])
            .collect()
    }

    #[test]
    fn few_colours_are_exact_and_round_trip() {
        let rgb: Vec<u8> = (0..76 * 86).flat_map(|i| if i % 3 == 0 { [255, 0, 0] } else { [0, 0, 0] }).collect();
        let img = quantize_rgb(&rgb, 76, 86).unwrap();
        assert_eq!(img.palette.len(), 2);
        let enc = img.encode();
        assert!(enc.len() < 600, "flat button compresses well: {}", enc.len());
        let back = Indexed::decode(&enc).unwrap();
        assert_eq!(back, img);
        assert_eq!(back.to_rgb(), rgb);
    }

    #[test]
    fn photo_like_images_are_quantised_to_256() {
        let rgb = gradient(64, 64);
        let img = quantize_rgb(&rgb, 64, 64).unwrap();
        assert!(img.palette.len() <= 256);
        let back = img.to_rgb();
        let err: u64 = rgb.iter().zip(&back).map(|(a, b)| u64::from(a.abs_diff(*b))).sum();
        assert!(err / (rgb.len() as u64) < 12, "mean error per channel too high: {}", err / rgb.len() as u64);
        let enc = img.encode();
        assert!(enc.len() < 64 * 64 + 900, "{}", enc.len());
        assert_eq!(Indexed::decode(&enc).unwrap(), img);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(quantize_rgb(&[0; 3], 0, 1).is_err());
        assert!(quantize_rgb(&[0; 3], 200, 1).is_err());
        assert!(quantize_rgb(&[0; 5], 1, 1).is_err());
        assert!(Indexed::decode(&[1, 2]).is_none());
        let mut enc = quantize_rgb(&[9; 12], 2, 2).unwrap().encode();
        enc[0] ^= 1;
        assert!(Indexed::decode(&enc).is_none());
    }

    #[test]
    fn fits_and_ids_images() {
        let src = DynamicImage::ImageRgb8(image::RgbImage::from_raw(100, 50, gradient(100, 50)).unwrap());
        let img = fit(&src, 64, 64).unwrap();
        assert_eq!((img.width, img.height), (64, 64));
        let a = img.encode();
        assert_eq!(image_id(&a), image_id(&a.clone()));
        assert_ne!(image_id(&a), image_id(&quantize_rgb(&[1; 12], 2, 2).unwrap().encode()));
    }
}
