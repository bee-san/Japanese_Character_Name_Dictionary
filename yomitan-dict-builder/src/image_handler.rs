use base64::engine::general_purpose::STANDARD;
use base64::Engine;

pub struct ImageHandler;

impl ImageHandler {
    /// Decode a base64-encoded image string.
    /// Input may have data URI prefix: "data:image/jpeg;base64,..."
    /// Returns (filename, raw_image_bytes).
    pub fn decode_image(base64_data: &str, char_id: &str) -> (String, Vec<u8>) {
        let (ext, data_part) = if let Some(comma_pos) = base64_data.find(',') {
            let header = &base64_data[..comma_pos];
            let data = &base64_data[comma_pos + 1..];

            let ext = if header.contains("png") {
                "png"
            } else if header.contains("gif") {
                "gif"
            } else if header.contains("webp") {
                "webp"
            } else {
                "jpg"
            };

            (ext, data)
        } else {
            ("jpg", base64_data) // No prefix — assume JPEG
        };

        let image_bytes = STANDARD.decode(data_part).unwrap_or_default();
        let filename = format!("c{}.{}", char_id, ext);

        (filename, image_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    #[test]
    fn test_decode_image_jpeg_with_prefix() {
        let raw = vec![0xFF, 0xD8, 0xFF]; // JPEG magic bytes
        let b64 = STANDARD.encode(&raw);
        let data_uri = format!("data:image/jpeg;base64,{}", b64);

        let (filename, bytes) = ImageHandler::decode_image(&data_uri, "123");
        assert_eq!(filename, "c123.jpg");
        assert_eq!(bytes, raw);
    }

    #[test]
    fn test_decode_image_png_with_prefix() {
        let raw = vec![0x89, 0x50, 0x4E, 0x47]; // PNG magic bytes
        let b64 = STANDARD.encode(&raw);
        let data_uri = format!("data:image/png;base64,{}", b64);

        let (filename, bytes) = ImageHandler::decode_image(&data_uri, "456");
        assert_eq!(filename, "c456.png");
        assert_eq!(bytes, raw);
    }

    #[test]
    fn test_decode_image_webp_with_prefix() {
        let raw = vec![0x52, 0x49, 0x46, 0x46];
        let b64 = STANDARD.encode(&raw);
        let data_uri = format!("data:image/webp;base64,{}", b64);

        let (filename, bytes) = ImageHandler::decode_image(&data_uri, "789");
        assert_eq!(filename, "c789.webp");
        assert_eq!(bytes, raw);
    }

    #[test]
    fn test_decode_image_no_prefix() {
        let raw = vec![0x01, 0x02, 0x03];
        let b64 = STANDARD.encode(&raw);

        let (filename, bytes) = ImageHandler::decode_image(&b64, "100");
        assert_eq!(filename, "c100.jpg"); // Default to jpg
        assert_eq!(bytes, raw);
    }

    #[test]
    fn test_decode_image_empty_data() {
        let (filename, bytes) = ImageHandler::decode_image("data:image/jpeg;base64,", "0");
        assert_eq!(filename, "c0.jpg");
        assert!(bytes.is_empty());
    }
}
