//! Compare captured PNG frames to detect a settled screen.

/// The share of pixels two frames may differ by and still show one
/// settled screen: 0.5%, about a 68px square. A blinking caret or a
/// small animation stays below it, and a menu, a dialog or a new page
/// goes above it.
const SETTLED_CHANGE: f64 = 0.005;

/// Whether two PNG frames show the same settled screen. Frames that do
/// not decode, or differ in size, match only when their bytes are equal.
pub fn frames_match(a: &[u8], b: &[u8]) -> bool {
    if a == b {
        return true;
    }
    let decode = |png: &[u8]| {
        image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .ok()
            .map(|image| image.to_rgb8())
    };
    let (Some(a), Some(b)) = (decode(a), decode(b)) else {
        return false;
    };
    if a.dimensions() != b.dimensions() {
        return false;
    }
    let changed = a
        .pixels()
        .zip(b.pixels())
        .filter(|(left, right)| left != right)
        .count();
    (changed as f64) <= SETTLED_CHANGE * f64::from(a.width() * a.height())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_with_box(width: u32, height: u32) -> Vec<u8> {
        let mut image = image::RgbImage::new(1280, 720);
        for x in 0..width {
            for y in 0..height {
                image.put_pixel(x, y, image::Rgb([255, 255, 255]));
            }
        }
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    /// A blinking caret is not a screen that still changes.
    #[test]
    fn frames_that_differ_by_a_caret_match() {
        assert!(frames_match(&png_with_box(0, 0), &png_with_box(0, 0)));
        assert!(frames_match(&png_with_box(0, 0), &png_with_box(2, 20)));
    }

    /// A menu that opens changes a large area.
    #[test]
    fn frames_that_differ_by_a_menu_do_not_match() {
        assert!(!frames_match(&png_with_box(0, 0), &png_with_box(300, 200)));
    }

    #[test]
    fn frames_that_are_not_images_match_only_when_equal() {
        assert!(frames_match(b"frame", b"frame"));
        assert!(!frames_match(b"frame", b"other"));
    }
}
