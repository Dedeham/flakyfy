//! The logo an owner uploads in place of Flakyfy's: checked by decoding it, then made a plain PNG
//! of a sensible size. Only pictures are taken. SVG is not: there is nothing here to draw it for
//! the covers, and an SVG served from this address could run script. Writing a new PNG also drops
//! whatever else the upload carried.

use std::io::Cursor;

use image::imageops::FilterType;
use image::{ImageFormat, ImageReader, Limits};

/// The largest upload taken.
pub const MAX_UPLOAD_BYTES: usize = 2 * 1024 * 1024;
/// The file kept in the config folder.
pub const FILE_NAME: &str = "custom-logo.png";

/// A wider or taller upload is refused before it is decoded.
const MAX_SOURCE_SIDE: u32 = 4096;
/// Stops a small file that unpacks to a huge picture.
const MAX_SOURCE_ALLOC: u64 = 128 * 1024 * 1024;
/// The kept logo's longest side; it is shown small everywhere.
const MAX_SIDE: u32 = 512;

/// The upload as the PNG to keep, or None when it is not a picture this takes.
pub fn normalize(upload: &[u8]) -> Option<Vec<u8>> {
    let mut reader = ImageReader::new(Cursor::new(upload)).with_guessed_format().ok()?;
    if !matches!(
        reader.format(),
        Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif)
    ) {
        return None;
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_SIDE);
    limits.max_image_height = Some(MAX_SOURCE_SIDE);
    limits.max_alloc = Some(MAX_SOURCE_ALLOC);
    reader.limits(limits);
    let mut picture = reader.decode().ok()?;
    if picture.width() > MAX_SIDE || picture.height() > MAX_SIDE {
        picture = picture.resize(MAX_SIDE, MAX_SIDE, FilterType::Lanczos3);
    }
    let mut png = Cursor::new(Vec::new());
    picture.to_rgba8().write_to(&mut png, ImageFormat::Png).ok()?;
    Some(png.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbaImage};

    fn encoded(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(RgbaImage::new(width, height)).to_rgb8())
            .write_to(&mut bytes, format)
            .expect("encode");
        bytes.into_inner()
    }

    #[test]
    fn a_picture_comes_back_as_a_png() {
        for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Gif] {
            let kept = normalize(&encoded(64, 32, format)).expect("taken");
            assert_eq!(
                image::guess_format(&kept).ok(),
                Some(ImageFormat::Png),
                "{format:?}"
            );
            assert_eq!(image::load_from_memory(&kept).expect("png").width(), 64);
        }
    }

    #[test]
    fn a_large_picture_is_made_smaller_in_proportion() {
        let kept =
            image::load_from_memory(&normalize(&encoded(2000, 1000, ImageFormat::Png)).unwrap()).unwrap();
        assert_eq!((kept.width(), kept.height()), (512, 256));
    }

    #[test]
    fn what_is_not_a_taken_picture_is_refused() {
        let svg = b"<svg xmlns='http://www.w3.org/2000/svg'><script>alert(1)</script></svg>";
        assert!(normalize(svg).is_none());
        assert!(normalize(b"not a picture").is_none());
        assert!(normalize(&[]).is_none());
        assert!(normalize(&encoded(8, 8, ImageFormat::Bmp)).is_none());
    }

    #[test]
    fn a_picture_too_wide_is_refused() {
        assert!(normalize(&encoded(MAX_SOURCE_SIDE + 1, 1, ImageFormat::Png)).is_none());
    }
}
