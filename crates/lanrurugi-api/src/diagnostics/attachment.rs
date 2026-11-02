//! Feedback screenshots: every upload becomes a WebP at its original pixel dimensions.
//!
//! The tracker stores attachments as sent and renders only PNG/JPEG/GIF/WebP inline, so one
//! output format keeps both its storage bill and the rendering path predictable. Animation is
//! preserved where a frame decoder exists (`lanrurugi_scanner::webp_convert`); a WebP upload is
//! passed through rather than re-encoded.

use crate::diagnostics::forward::FeedbackAttachment;

/// One note may carry this many screenshots.
pub const MAX_IMAGES: usize = 3;
/// Pre-conversion cap, per file. A raw phone screenshot is usually well under this.
pub const MAX_UPLOAD_BYTES: usize = 12 * 1024 * 1024;
/// The tracker drops an attachment above 10 MB (and keeps the event), so stay under that.
pub const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;

/// Visually near-transparent to lossy artefacts on UI text, and still a fraction of PNG.
const QUALITY: u8 = 85;
/// One retry at this quality when the first pass lands over the tracker's ceiling. Dimensions are
/// never reduced: the point of a bug screenshot is the pixel that went wrong.
const FALLBACK_QUALITY: u8 = 60;

pub async fn convert(original_name: &str, bytes: Vec<u8>) -> Result<FeedbackAttachment, String> {
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(format!(
            "the image is larger than {} MB.",
            MAX_UPLOAD_BYTES / 1024 / 1024
        ));
    }
    let mut webp = lanrurugi_scanner::webp_convert::convert_any_to_webp(bytes.clone(), QUALITY)
        .await
        .map_err(|error| format!("the image could not be converted: {error}"))?;
    if webp.len() > MAX_ATTACHMENT_BYTES {
        webp = lanrurugi_scanner::webp_convert::convert_any_to_webp(bytes, FALLBACK_QUALITY)
            .await
            .map_err(|error| format!("the image could not be converted: {error}"))?;
        if webp.len() > MAX_ATTACHMENT_BYTES {
            return Err(format!(
                "the image is still larger than {} MB after conversion.",
                MAX_ATTACHMENT_BYTES / 1024 / 1024
            ));
        }
    }
    Ok(FeedbackAttachment {
        filename: webp_filename(original_name),
        bytes: webp,
    })
}

/// The tracker keeps the original stem for display; only characters that are safe in a header and
/// in a filename survive. A name that is empty after that (all-CJK, emoji) falls back to a plain
/// one rather than a file called `.webp`.
fn webp_filename(original: &str) -> String {
    let stem = std::path::Path::new(original)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let clean: String = stem
        .chars()
        .filter_map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                Some(c)
            } else if c.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .take(60)
        .collect();
    let clean = clean.trim_matches('-').to_string();
    if clean.is_empty() {
        "feedback.webp".to_string()
    } else {
        format!("{clean}.webp")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbImage::from_pixel(width, height, image::Rgb([10, 20, 30]));
        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    #[tokio::test]
    async fn a_png_becomes_a_webp_with_the_same_dimensions() {
        let attachment = convert("Screenshot 2026-10-29.png", png(40, 30))
            .await
            .unwrap();
        assert_eq!(attachment.filename, "Screenshot-2026-10-29.webp");
        assert_eq!(&attachment.bytes[0..4], b"RIFF");
        assert_eq!(&attachment.bytes[8..12], b"WEBP");
        let decoded = image::load_from_memory(&attachment.bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (40, 30));
    }

    #[tokio::test]
    async fn an_upload_over_the_cap_is_refused_before_conversion() {
        let error = convert("huge.png", vec![0u8; MAX_UPLOAD_BYTES + 1])
            .await
            .unwrap_err();
        assert!(error.contains("larger than"), "{error}");
    }

    #[tokio::test]
    async fn something_that_is_not_an_image_is_refused() {
        let error = convert("notes.txt", b"hello".to_vec()).await.unwrap_err();
        assert!(error.contains("could not be converted"), "{error}");
    }

    #[test]
    fn a_name_with_nothing_usable_left_falls_back() {
        assert_eq!(webp_filename("截图.png"), "feedback.webp");
        assert_eq!(webp_filename("截图 2026.png"), "2026.webp");
        assert_eq!(webp_filename("reader bug.PNG"), "reader-bug.webp");
        assert_eq!(webp_filename(""), "feedback.webp");
    }
}
