//! Blur/sharpness detection via Laplacian variance — a classic no-reference blur metric (apply
//! the discrete Laplacian operator, then take the variance of the result: a sharp image has many
//! strong edges, so the Laplacian response varies a lot pixel-to-pixel; a blurry image's edges are
//! soft, so the Laplacian response stays close to zero everywhere and its variance is low).
//!
//! Deliberately not `resolution` or file size — issue #77's own real-world motivation: a
//! low-quality scan can report a large pixel resolution while the actual line art is thick/smeared
//! and the page is genuinely hard to read, which resolution/file-size metrics can't distinguish
//! from a real high-quality scan at the same resolution.

use image::DynamicImage;

/// The 3x3 discrete Laplacian kernel (4-connected, the standard choice for this metric — matches
/// OpenCV's own default `cv2.Laplacian` kernel used in the reference "variance of Laplacian" blur
/// detector this technique is named after).
const LAPLACIAN_KERNEL: [i32; 9] = [0, 1, 0, 1, -4, 1, 0, 1, 0];

/// Computes the Laplacian-variance sharpness score for an image — higher means sharper. Not
/// normalized against any fixed scale (the metric is only meaningful as a relative comparison
/// between two images processed the same way, per this module's own docs), so callers compare two
/// scores against each other, not against an absolute threshold.
///
/// Converts to grayscale first (color channels would triple the work for no accuracy gain — edge
/// strength is a luminance property) and processes at the image's native resolution (no
/// downscaling) since resolution differences between the two compared versions are themselves
/// part of what's being compared.
pub fn laplacian_variance(image: &DynamicImage) -> f64 {
    let gray = image.to_luma8();
    let (width, height) = gray.dimensions();
    if width < 3 || height < 3 {
        // Too small for a 3x3 kernel to produce any interior pixels at all.
        return 0.0;
    }

    let mut responses: Vec<f64> = Vec::with_capacity(((width - 2) * (height - 2)) as usize);
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let mut sum = 0i32;
            for (ki, (dx, dy)) in [
                (-1, -1),
                (0, -1),
                (1, -1),
                (-1, 0),
                (0, 0),
                (1, 0),
                (-1, 1),
                (0, 1),
                (1, 1),
            ]
            .into_iter()
            .enumerate()
            {
                let px = gray
                    .get_pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32)
                    .0[0] as i32;
                sum += px * LAPLACIAN_KERNEL[ki];
            }
            responses.push(sum as f64);
        }
    }

    let mean = responses.iter().sum::<f64>() / responses.len() as f64;
    responses.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / responses.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Luma};

    #[test]
    fn laplacian_variance_is_zero_for_a_flat_image() {
        // A perfectly uniform image has zero edges anywhere — the Laplacian response is 0 at
        // every interior pixel, so the variance must be exactly 0.0, not just "low".
        let flat: ImageBuffer<Luma<u8>, Vec<u8>> = ImageBuffer::from_pixel(10, 10, Luma([128]));
        let img = DynamicImage::ImageLuma8(flat);
        assert_eq!(laplacian_variance(&img), 0.0);
    }

    #[test]
    fn laplacian_variance_is_higher_for_a_sharp_checkerboard_than_a_blurred_one() {
        let sharp: ImageBuffer<Luma<u8>, Vec<u8>> = ImageBuffer::from_fn(20, 20, |x, y| {
            if (x / 2 + y / 2) % 2 == 0 {
                Luma([255])
            } else {
                Luma([0])
            }
        });
        // A "blurred" version: every pixel replaced by the local 3x3 average of the sharp
        // checkerboard — same overall structure, but edges are softened, which is exactly what a
        // low-quality/blurry scan looks like relative to a sharp one.
        let blurred: ImageBuffer<Luma<u8>, Vec<u8>> = ImageBuffer::from_fn(20, 20, |x, y| {
            let mut sum = 0u32;
            let mut count = 0u32;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx >= 0 && ny >= 0 && (nx as u32) < 20 && (ny as u32) < 20 {
                        let v = if ((nx / 2) + (ny / 2)) % 2 == 0 {
                            255
                        } else {
                            0
                        };
                        sum += v as u32;
                        count += 1;
                    }
                }
            }
            Luma([(sum / count) as u8])
        });

        let sharp_score = laplacian_variance(&DynamicImage::ImageLuma8(sharp));
        let blurred_score = laplacian_variance(&DynamicImage::ImageLuma8(blurred));
        assert!(
            sharp_score > blurred_score,
            "sharp checkerboard ({sharp_score}) must score higher than its blurred version ({blurred_score})"
        );
    }

    #[test]
    fn laplacian_variance_handles_tiny_images_without_panicking() {
        let tiny: ImageBuffer<Luma<u8>, Vec<u8>> = ImageBuffer::from_pixel(2, 2, Luma([100]));
        assert_eq!(laplacian_variance(&DynamicImage::ImageLuma8(tiny)), 0.0);
    }
}
