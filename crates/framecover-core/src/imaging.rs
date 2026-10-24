//! Image helpers matching the Pillow behaviour of the Python version.

use std::io::Cursor;

use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, RgbImage, codecs::jpeg::JpegEncoder};

use crate::error::{Error, Result};

/// Maximum cover-art size (fit inside, aspect preserved).
pub const MAX_THUMB_W: u32 = 640;
pub const MAX_THUMB_H: u32 = 360;
pub const JPEG_QUALITY: u8 = 90;

/// Size that fits `(w, h)` inside `(max_w, max_h)` preserving aspect ratio,
/// never upscaling — the same rule as `PIL.Image.thumbnail`.
pub fn fit_within(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if w <= max_w && h <= max_h {
        return (w, h);
    }
    let aspect = w as f64 / h as f64;
    let (mut nw, mut nh) = (max_w as f64, max_h as f64);
    if nw / nh >= aspect {
        nw = (nh * aspect).round();
    } else {
        nh = (nw / aspect).round();
    }
    ((nw as u32).max(1), (nh as u32).max(1))
}

/// Downscale with Lanczos to fit inside `(max_w, max_h)`; returns a copy.
pub fn thumbnail(img: &RgbImage, max_w: u32, max_h: u32) -> RgbImage {
    let (w, h) = fit_within(img.width(), img.height(), max_w, max_h);
    if (w, h) == img.dimensions() {
        return img.clone();
    }
    image::imageops::resize(img, w, h, FilterType::Lanczos3)
}

/// Encode as baseline JPEG at `quality` with 4:2:0 chroma subsampling, like
/// Pillow's default. Falls back to the `image` encoder (4:4:4) for images
/// wider or taller than 65535 px, which `jpeg-encoder` cannot represent.
pub fn encode_jpeg(img: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    match (u16::try_from(img.width()), u16::try_from(img.height())) {
        (Ok(w), Ok(h)) => {
            let mut encoder = jpeg_encoder::Encoder::new(&mut buf, quality);
            encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::R_4_2_0);
            encoder
                .encode(img.as_raw(), w, h, jpeg_encoder::ColorType::Rgb)
                .map_err(|e| Error::Other(format!("JPEG encoding failed: {e}")))?;
        }
        _ => JpegEncoder::new_with_quality(&mut buf, quality)
            .encode_image(img)
            .map_err(|e| Error::Other(format!("JPEG encoding failed: {e}")))?,
    }
    Ok(buf)
}

/// Encode as PNG.
pub fn encode_png(img: &RgbImage) -> Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Png)
        .map_err(|e| Error::Other(format!("PNG encoding failed: {e}")))?;
    Ok(buf.into_inner())
}

/// Decode any supported image bytes (JPEG/PNG) into RGB.
pub fn decode_rgb(bytes: &[u8]) -> Result<RgbImage> {
    image::load_from_memory(bytes)
        .map(DynamicImage::into_rgb8)
        .map_err(|e| Error::Other(format!("image decoding failed: {e}")))
}

/// How an image of another aspect ratio is placed into a video frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageFit {
    /// Scale to fit entirely inside the frame; black bars fill the rest.
    #[default]
    Fit,
    /// Scale to cover the whole frame; the overflow is cropped (centred).
    Crop,
}

/// Place `img` into a `width`x`height` frame according to `mode`, scaling
/// up or down with Lanczos.
pub fn compose(img: &RgbImage, width: u32, height: u32, mode: ImageFit) -> RgbImage {
    let (width, height) = (width.max(1), height.max(1));
    let (iw, ih) = (img.width().max(1) as f64, img.height().max(1) as f64);
    let scale = match mode {
        ImageFit::Fit => (width as f64 / iw).min(height as f64 / ih),
        ImageFit::Crop => (width as f64 / iw).max(height as f64 / ih),
    };
    let sw = ((iw * scale).round() as u32).max(1);
    let sh = ((ih * scale).round() as u32).max(1);
    let scaled = if (sw, sh) == img.dimensions() {
        img.clone()
    } else {
        image::imageops::resize(img, sw, sh, FilterType::Lanczos3)
    };
    let mut canvas = RgbImage::new(width, height);
    // Signed offsets: negative when cropping.
    let dx = (i64::from(width) - i64::from(sw)) / 2;
    let dy = (i64::from(height) - i64::from(sh)) / 2;
    image::imageops::overlay(&mut canvas, &scaled, dx, dy);
    canvas
}

/// Load a still image file (JPEG, PNG, WebP, BMP) as RGB, applying its EXIF
/// orientation so phone photos are upright.
pub fn load_image(path: &std::path::Path) -> Result<RgbImage> {
    use image::ImageDecoder;
    let fail = |e: &dyn std::fmt::Display| {
        Error::InvalidInput(format!(
            "Could not read the image '{}': {e}",
            path.display()
        ))
    };
    let reader = image::ImageReader::open(path)
        .map_err(|e| fail(&e))?
        .with_guessed_format()
        .map_err(|e| fail(&e))?;
    let mut decoder = reader.into_decoder().map_err(|e| fail(&e))?;
    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| fail(&e))?;
    if let Some(orientation) = orientation {
        img.apply_orientation(orientation);
    }
    Ok(img.into_rgb8())
}

/// File extensions [`load_image`] accepts (for file dialogs and drops).
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp"];

/// Cover art bytes as written by the thumbnail writer: fit 640x360, JPEG q90.
pub fn cover_jpeg(frame: &RgbImage) -> Result<Vec<u8>> {
    encode_jpeg(&thumbnail(frame, MAX_THUMB_W, MAX_THUMB_H), JPEG_QUALITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_matches_pillow() {
        assert_eq!(fit_within(1920, 1080, 640, 360), (640, 360));
        assert_eq!(fit_within(1080, 1920, 640, 360), (203, 360));
        assert_eq!(fit_within(320, 240, 640, 360), (320, 240));
        assert_eq!(fit_within(1000, 400, 640, 360), (640, 256));
    }

    #[test]
    fn compose_fit_letterboxes_and_crop_fills() {
        // A wide red image into a 4:3 frame.
        let wide = RgbImage::from_pixel(400, 100, image::Rgb([255, 0, 0]));
        let fit = compose(&wide, 320, 240, ImageFit::Fit);
        assert_eq!(fit.dimensions(), (320, 240));
        assert_eq!(fit.get_pixel(160, 5).0, [0, 0, 0], "black bar on top");
        assert_eq!(fit.get_pixel(160, 120).0, [255, 0, 0]);
        let crop = compose(&wide, 320, 240, ImageFit::Crop);
        assert_eq!(crop.dimensions(), (320, 240));
        assert_eq!(crop.get_pixel(0, 0).0, [255, 0, 0], "no bars when cropping");
        assert_eq!(crop.get_pixel(319, 239).0, [255, 0, 0]);
    }

    #[test]
    fn crop_keeps_the_centre() {
        // Left half blue, right half green, 200x100 into a square: the centre
        // column shows both halves meeting.
        let mut img = RgbImage::from_pixel(200, 100, image::Rgb([0, 0, 255]));
        for x in 100..200 {
            for y in 0..100 {
                img.put_pixel(x, y, image::Rgb([0, 255, 0]));
            }
        }
        let crop = compose(&img, 100, 100, ImageFit::Crop);
        assert_eq!(crop.get_pixel(10, 50).0, [0, 0, 255]);
        assert_eq!(crop.get_pixel(90, 50).0, [0, 255, 0]);
    }

    #[test]
    fn load_image_reads_png() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        std::fs::write(
            &path,
            encode_png(&RgbImage::from_pixel(3, 2, image::Rgb([1, 2, 3]))).unwrap(),
        )
        .unwrap();
        assert_eq!(load_image(&path).unwrap().dimensions(), (3, 2));
        assert!(load_image(&dir.path().join("missing.png")).is_err());
    }

    #[test]
    fn jpeg_roundtrip() {
        let img = RgbImage::from_pixel(1280, 720, image::Rgb([10, 200, 30]));
        let bytes = cover_jpeg(&img).unwrap();
        let back = decode_rgb(&bytes).unwrap();
        assert_eq!(back.dimensions(), (640, 360));
    }
}
