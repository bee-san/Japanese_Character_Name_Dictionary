use image::imageops::FilterType;
use image::ImageFormat;
use std::io::Cursor;

/// Maximum dimensions for character portrait thumbnails (2× for retina).
const MAX_WIDTH: u32 = 160;
const MAX_HEIGHT: u32 = 200;

pub struct ImageHandler;

impl ImageHandler {
    /// Detect file extension from raw image bytes by checking magic bytes.
    pub fn detect_extension(bytes: &[u8]) -> &'static str {
        if bytes.len() >= 4 {
            // JPEG: FF D8 FF
            if bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
                return "jpg";
            }
            // PNG: 89 50 4E 47
            if bytes[0] == 0x89 && bytes[1] == 0x50 && bytes[2] == 0x4E && bytes[3] == 0x47 {
                return "png";
            }
            // GIF: 47 49 46
            if bytes[0] == 0x47 && bytes[1] == 0x49 && bytes[2] == 0x46 {
                return "gif";
            }
            // WebP: RIFF....WEBP
            if bytes[0] == 0x52
                && bytes[1] == 0x49
                && bytes[2] == 0x46
                && bytes[3] == 0x46
                && bytes.len() >= 12
                && bytes[8] == 0x57
                && bytes[9] == 0x45
                && bytes[10] == 0x42
                && bytes[11] == 0x50
            {
                return "webp";
            }
        }
        "jpg" // fallback
    }

    /// Resize raw image bytes to fit within MAX_WIDTH × MAX_HEIGHT, output as JPEG.
    /// Returns (resized_bytes, ext, width, height) on success,
    /// or the original (bytes, detected_ext, 0, 0) on failure.
    pub fn resize_image(bytes: &[u8]) -> (Vec<u8>, &'static str, u32, u32) {
        // Try to decode the image
        let img = match image::load_from_memory(bytes) {
            Ok(img) => img,
            Err(_) => {
                // Can't decode — return original bytes with detected extension
                return (bytes.to_vec(), Self::detect_extension(bytes), 0, 0);
            }
        };

        let (w, h) = (img.width(), img.height());

        // Only resize if larger than our max dimensions
        let resized = if w > MAX_WIDTH || h > MAX_HEIGHT {
            img.resize(MAX_WIDTH, MAX_HEIGHT, FilterType::Lanczos3)
        } else {
            img
        };

        let (rw, rh) = (resized.width(), resized.height());

        // Encode as JPEG (widely supported by Yomitan and all browsers)
        let mut buf = Cursor::new(Vec::new());
        match resized.write_to(&mut buf, ImageFormat::Jpeg) {
            Ok(_) => (buf.into_inner(), "jpg", rw, rh),
            Err(_) => (bytes.to_vec(), Self::detect_extension(bytes), w, h),
        }
    }

    /// Build the filename for a character image in the ZIP.
    pub fn make_filename(char_id: &str, ext: &str) -> String {
        format!("c{}.{}", char_id, ext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === detect_extension tests ===

    #[test]
    fn test_detect_extension_jpeg() {
        assert_eq!(
            ImageHandler::detect_extension(&[0xFF, 0xD8, 0xFF, 0xE0]),
            "jpg"
        );
    }

    #[test]
    fn test_detect_extension_png() {
        assert_eq!(
            ImageHandler::detect_extension(&[0x89, 0x50, 0x4E, 0x47]),
            "png"
        );
    }

    #[test]
    fn test_detect_extension_gif() {
        assert_eq!(
            ImageHandler::detect_extension(&[0x47, 0x49, 0x46, 0x38]),
            "gif"
        );
    }

    #[test]
    fn test_detect_extension_webp() {
        let webp_header = [
            0x52, 0x49, 0x46, 0x46, 0x00, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50,
        ];
        assert_eq!(ImageHandler::detect_extension(&webp_header), "webp");
    }

    #[test]
    fn test_detect_extension_unknown() {
        assert_eq!(
            ImageHandler::detect_extension(&[0x00, 0x01, 0x02, 0x03]),
            "jpg"
        );
    }

    #[test]
    fn test_detect_extension_too_short() {
        assert_eq!(ImageHandler::detect_extension(&[0xFF, 0xD8]), "jpg");
    }

    // === resize_image tests ===

    #[test]
    fn test_resize_small_image_stays_small() {
        // Create a tiny 2×2 JPEG-like image using the image crate
        let img = image::RgbImage::from_pixel(2, 2, image::Rgb([255, 0, 0]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Jpeg).unwrap();
        let jpeg_bytes = buf.into_inner();

        let (resized, ext, w, h) = ImageHandler::resize_image(&jpeg_bytes);
        assert_eq!(ext, "jpg");
        // Should still be valid image data
        assert!(!resized.is_empty());
        // Verify it's actually JPEG by checking magic bytes
        assert_eq!(&resized[0..3], &[0xFF, 0xD8, 0xFF]);
        // Dimensions should match the original (no resize needed)
        assert_eq!(w, 2);
        assert_eq!(h, 2);
    }

    #[test]
    fn test_resize_large_image_shrinks() {
        // Create a 400×500 image (larger than MAX_WIDTH × MAX_HEIGHT)
        let img = image::RgbImage::from_pixel(400, 500, image::Rgb([0, 128, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        let png_bytes = buf.into_inner();

        let (resized, ext, w, h) = ImageHandler::resize_image(&png_bytes);
        assert_eq!(ext, "jpg");

        // Verify the resized image dimensions are within bounds
        let resized_img = image::load_from_memory(&resized).unwrap();
        assert!(
            resized_img.width() <= 160,
            "width {} > 160",
            resized_img.width()
        );
        assert!(
            resized_img.height() <= 200,
            "height {} > 200",
            resized_img.height()
        );
        // Returned dimensions should match the actual resized image
        assert_eq!(w, resized_img.width());
        assert_eq!(h, resized_img.height());
    }

    #[test]
    fn test_resize_preserves_aspect_ratio() {
        // 300×600 image — tall portrait, should scale to 100×200
        let img = image::RgbImage::from_pixel(300, 600, image::Rgb([0, 0, 0]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Jpeg).unwrap();
        let jpeg_bytes = buf.into_inner();

        let (resized, _, rw, rh) = ImageHandler::resize_image(&jpeg_bytes);
        let resized_img = image::load_from_memory(&resized).unwrap();
        assert!(resized_img.height() <= 200);
        assert!(resized_img.width() <= 160);
        // Aspect ratio should be roughly 1:2
        let ratio = resized_img.width() as f64 / resized_img.height() as f64;
        assert!(
            (ratio - 0.5).abs() < 0.05,
            "aspect ratio {} not ~0.5",
            ratio
        );
        // Returned dimensions should match
        assert_eq!(rw, resized_img.width());
        assert_eq!(rh, resized_img.height());
    }

    #[test]
    fn test_resize_invalid_bytes_returns_original() {
        let garbage = vec![0x00, 0x01, 0x02, 0x03, 0x04];
        let (result, ext, w, h) = ImageHandler::resize_image(&garbage);
        assert_eq!(result, garbage);
        assert_eq!(ext, "jpg"); // fallback
        assert_eq!(w, 0);
        assert_eq!(h, 0);
    }

    // === make_filename tests ===

    #[test]
    fn test_make_filename() {
        assert_eq!(ImageHandler::make_filename("42", "webp"), "c42.webp");
        assert_eq!(ImageHandler::make_filename("c100", "jpg"), "cc100.jpg");
    }

    // === Edge case: detect_extension boundary sizes ===

    #[test]
    fn test_detect_extension_exactly_3_bytes_jpeg() {
        // 3 bytes: JPEG magic is FF D8 FF, but len < 4 so check fails
        assert_eq!(ImageHandler::detect_extension(&[0xFF, 0xD8, 0xFF]), "jpg");
    }

    #[test]
    fn test_detect_extension_empty() {
        assert_eq!(ImageHandler::detect_extension(&[]), "jpg");
    }

    #[test]
    fn test_detect_extension_single_byte() {
        assert_eq!(ImageHandler::detect_extension(&[0xFF]), "jpg");
    }

    #[test]
    fn test_detect_extension_webp_incomplete_header() {
        // RIFF header but only 8 bytes (needs 12 for WebP check)
        let partial = [0x52, 0x49, 0x46, 0x46, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(ImageHandler::detect_extension(&partial), "jpg");
    }

    // === Edge case: resize with empty bytes ===

    #[test]
    fn test_resize_empty_bytes() {
        let (result, ext, w, h) = ImageHandler::resize_image(&[]);
        assert!(result.is_empty());
        assert_eq!(ext, "jpg"); // fallback
        assert_eq!(w, 0);
        assert_eq!(h, 0);
    }

    // === Edge case: resize 1x1 image ===

    #[test]
    fn test_resize_1x1_image() {
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([128, 128, 128]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        let png_bytes = buf.into_inner();

        let (resized, ext, w, h) = ImageHandler::resize_image(&png_bytes);
        assert_eq!(ext, "jpg");
        assert!(!resized.is_empty());
        assert_eq!(w, 1);
        assert_eq!(h, 1);
    }

    // === Edge case: make_filename with special characters ===

    #[test]
    fn test_make_filename_with_slash() {
        // Documents that path traversal chars are NOT sanitized
        assert_eq!(ImageHandler::make_filename("../etc", "jpg"), "c../etc.jpg");
    }

    #[test]
    fn test_make_filename_empty_id() {
        assert_eq!(ImageHandler::make_filename("", "jpg"), "c.jpg");
    }

    #[test]
    fn test_make_filename_empty_ext() {
        assert_eq!(ImageHandler::make_filename("42", ""), "c42.");
    }

    // ===== Additional comprehensive tests =====

    // --- detect_extension: more magic byte patterns ---

    #[test]
    fn test_detect_extension_jpeg_with_exif() {
        // JPEG with EXIF marker (FF D8 FF E1)
        assert_eq!(
            ImageHandler::detect_extension(&[0xFF, 0xD8, 0xFF, 0xE1]),
            "jpg"
        );
    }

    #[test]
    fn test_detect_extension_jpeg_with_jfif() {
        // JPEG with JFIF marker (FF D8 FF E0)
        assert_eq!(
            ImageHandler::detect_extension(&[0xFF, 0xD8, 0xFF, 0xE0]),
            "jpg"
        );
    }

    #[test]
    fn test_detect_extension_png_full_header() {
        // Full PNG header: 89 50 4E 47 0D 0A 1A 0A
        assert_eq!(
            ImageHandler::detect_extension(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]),
            "png"
        );
    }

    #[test]
    fn test_detect_extension_gif87a() {
        assert_eq!(
            ImageHandler::detect_extension(&[0x47, 0x49, 0x46, 0x38, 0x37, 0x61]),
            "gif"
        );
    }

    #[test]
    fn test_detect_extension_gif89a() {
        assert_eq!(
            ImageHandler::detect_extension(&[0x47, 0x49, 0x46, 0x38, 0x39, 0x61]),
            "gif"
        );
    }

    #[test]
    fn test_detect_extension_webp_with_size() {
        let mut webp = vec![0x52, 0x49, 0x46, 0x46];
        webp.extend_from_slice(&[0x24, 0x08, 0x00, 0x00]); // file size
        webp.extend_from_slice(&[0x57, 0x45, 0x42, 0x50]); // WEBP
        assert_eq!(ImageHandler::detect_extension(&webp), "webp");
    }

    // --- resize_image: boundary dimensions ---

    #[test]
    fn test_resize_exact_max_dimensions() {
        // Image exactly at MAX_WIDTH × MAX_HEIGHT (160×200) — should NOT resize
        let img = image::RgbImage::from_pixel(160, 200, image::Rgb([100, 100, 100]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Jpeg).unwrap();
        let jpeg_bytes = buf.into_inner();

        let (_, _, w, h) = ImageHandler::resize_image(&jpeg_bytes);
        assert_eq!(w, 160);
        assert_eq!(h, 200);
    }

    #[test]
    fn test_resize_one_pixel_over_width() {
        // 161×200 — just over width limit, should resize
        let img = image::RgbImage::from_pixel(161, 200, image::Rgb([100, 100, 100]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Jpeg).unwrap();
        let jpeg_bytes = buf.into_inner();

        let (_, _, w, h) = ImageHandler::resize_image(&jpeg_bytes);
        assert!(w <= 160);
        assert!(h <= 200);
    }

    #[test]
    fn test_resize_one_pixel_over_height() {
        // 160×201 — just over height limit, should resize
        let img = image::RgbImage::from_pixel(160, 201, image::Rgb([100, 100, 100]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Jpeg).unwrap();
        let jpeg_bytes = buf.into_inner();

        let (_, _, w, h) = ImageHandler::resize_image(&jpeg_bytes);
        assert!(w <= 160);
        assert!(h <= 200);
    }

    #[test]
    fn test_resize_very_wide_image() {
        // 1000×100 — very wide, should scale down to fit width
        let img = image::RgbImage::from_pixel(1000, 100, image::Rgb([0, 0, 0]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        let png_bytes = buf.into_inner();

        let (resized, ext, w, h) = ImageHandler::resize_image(&png_bytes);
        assert_eq!(ext, "jpg");
        assert!(w <= 160);
        assert!(h <= 200);
        assert!(!resized.is_empty());
    }

    #[test]
    fn test_resize_very_tall_image() {
        // 50×2000 — very tall, should scale down to fit height
        let img = image::RgbImage::from_pixel(50, 2000, image::Rgb([0, 0, 0]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        let png_bytes = buf.into_inner();

        let (_, _, w, h) = ImageHandler::resize_image(&png_bytes);
        assert!(w <= 160);
        assert!(h <= 200);
    }

    #[test]
    fn test_resize_output_is_always_jpeg() {
        // Input is PNG, output should be JPEG
        let img = image::RgbImage::from_pixel(10, 10, image::Rgb([255, 0, 0]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        let png_bytes = buf.into_inner();

        let (resized, ext, _, _) = ImageHandler::resize_image(&png_bytes);
        assert_eq!(ext, "jpg");
        // Verify output starts with JPEG magic bytes
        assert_eq!(&resized[0..3], &[0xFF, 0xD8, 0xFF]);
    }

    // --- make_filename: various IDs ---

    #[test]
    fn test_make_filename_numeric_id() {
        assert_eq!(ImageHandler::make_filename("12345", "jpg"), "c12345.jpg");
    }

    #[test]
    fn test_make_filename_with_prefix() {
        // Some IDs might already have a prefix
        assert_eq!(ImageHandler::make_filename("v17", "png"), "cv17.png");
    }

    #[test]
    fn test_make_filename_unicode_id() {
        assert_eq!(ImageHandler::make_filename("テスト", "jpg"), "cテスト.jpg");
    }
}
