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


    /// Resize raw image bytes to fit within MAX_WIDTH × MAX_HEIGHT, output as WebP.
    /// Returns (resized_bytes, "webp") on success, or the original (bytes, detected_ext) on failure.
    pub fn resize_image(bytes: &[u8]) -> (Vec<u8>, &'static str) {
        // Try to decode the image
        let img = match image::load_from_memory(bytes) {
            Ok(img) => img,
            Err(_) => {
                // Can't decode — return original bytes with detected extension
                return (bytes.to_vec(), Self::detect_extension(bytes));
            }
        };

        let (w, h) = (img.width(), img.height());

        // Only resize if larger than our max dimensions
        let resized = if w > MAX_WIDTH || h > MAX_HEIGHT {
            img.resize(MAX_WIDTH, MAX_HEIGHT, FilterType::Lanczos3)
        } else {
            img
        };

        // Encode as WebP
        let mut buf = Cursor::new(Vec::new());
        match resized.write_to(&mut buf, ImageFormat::WebP) {
            Ok(_) => (buf.into_inner(), "webp"),
            Err(_) => {
                // WebP encoding failed — try JPEG as fallback
                let mut buf = Cursor::new(Vec::new());
                match resized.write_to(&mut buf, ImageFormat::Jpeg) {
                    Ok(_) => (buf.into_inner(), "jpg"),
                    Err(_) => (bytes.to_vec(), Self::detect_extension(bytes)),
                }
            }
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
        assert_eq!(ImageHandler::detect_extension(&[0xFF, 0xD8, 0xFF, 0xE0]), "jpg");
    }

    #[test]
    fn test_detect_extension_png() {
        assert_eq!(ImageHandler::detect_extension(&[0x89, 0x50, 0x4E, 0x47]), "png");
    }

    #[test]
    fn test_detect_extension_gif() {
        assert_eq!(ImageHandler::detect_extension(&[0x47, 0x49, 0x46, 0x38]), "gif");
    }

    #[test]
    fn test_detect_extension_webp() {
        let webp_header = [0x52, 0x49, 0x46, 0x46, 0x00, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50];
        assert_eq!(ImageHandler::detect_extension(&webp_header), "webp");
    }

    #[test]
    fn test_detect_extension_unknown() {
        assert_eq!(ImageHandler::detect_extension(&[0x00, 0x01, 0x02, 0x03]), "jpg");
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

        let (resized, ext) = ImageHandler::resize_image(&jpeg_bytes);
        assert_eq!(ext, "webp");
        // Should still be valid image data
        assert!(!resized.is_empty());
        // Verify it's actually WebP by checking RIFF header
        assert_eq!(&resized[0..4], b"RIFF");
    }

    #[test]
    fn test_resize_large_image_shrinks() {
        // Create a 400×500 image (larger than MAX_WIDTH × MAX_HEIGHT)
        let img = image::RgbImage::from_pixel(400, 500, image::Rgb([0, 128, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        let png_bytes = buf.into_inner();

        let (resized, ext) = ImageHandler::resize_image(&png_bytes);
        assert_eq!(ext, "webp");

        // Verify the resized image dimensions are within bounds
        let resized_img = image::load_from_memory(&resized).unwrap();
        assert!(resized_img.width() <= 160, "width {} > 160", resized_img.width());
        assert!(resized_img.height() <= 200, "height {} > 200", resized_img.height());
    }

    #[test]
    fn test_resize_preserves_aspect_ratio() {
        // 300×600 image — tall portrait, should scale to 100×200
        let img = image::RgbImage::from_pixel(300, 600, image::Rgb([0, 0, 0]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Jpeg).unwrap();
        let jpeg_bytes = buf.into_inner();

        let (resized, _) = ImageHandler::resize_image(&jpeg_bytes);
        let resized_img = image::load_from_memory(&resized).unwrap();
        assert!(resized_img.height() <= 200);
        assert!(resized_img.width() <= 160);
        // Aspect ratio should be roughly 1:2
        let ratio = resized_img.width() as f64 / resized_img.height() as f64;
        assert!((ratio - 0.5).abs() < 0.05, "aspect ratio {} not ~0.5", ratio);
    }

    #[test]
    fn test_resize_invalid_bytes_returns_original() {
        let garbage = vec![0x00, 0x01, 0x02, 0x03, 0x04];
        let (result, ext) = ImageHandler::resize_image(&garbage);
        assert_eq!(result, garbage);
        assert_eq!(ext, "jpg"); // fallback
    }

    // === make_filename tests ===

    #[test]
    fn test_make_filename() {
        assert_eq!(ImageHandler::make_filename("42", "webp"), "c42.webp");
        assert_eq!(ImageHandler::make_filename("c100", "jpg"), "cc100.jpg");
    }
}
