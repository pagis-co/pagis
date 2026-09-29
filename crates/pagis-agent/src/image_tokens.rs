//! The documented token cost of one image, from its pixel size.
//!
//! OpenAI: https://developers.openai.com/api/docs/guides/images-vision
//! Claude: https://platform.claude.com/docs/en/build-with-claude/vision

/// The token cost of the `data:` URI image on `model`, the vendor's own
/// model id. `None` when Pagis cannot read the pixel size.
pub(crate) fn image_tokens(model: &str, data_uri: &str) -> Option<u64> {
    let (width, height) = dimensions(data_uri)?;
    let (width, height) = (f64::from(width), f64::from(height));
    let tokens = if model.starts_with("claude-") {
        // Width times height over 750. Auto-resizing only lowers it, and
        // the high-resolution tier stops at 4,784.
        (width * height / 750.0).ceil().min(4_784.0)
    } else if model.starts_with("gpt-6-astra") || model.starts_with("gpt-5.6") {
        // Pagis omits detail, so `auto` uses `original`.
        patches(width, height, 30_000.0, 1.2)
    } else if model.starts_with("gpt-4o-mini") {
        tiles(width, height, 2_833.0, 5_667.0)
    } else {
        // A model with no documented formula takes the largest cost of
        // the current formulas: gpt-4o tiles, and the patches of the
        // nano models with the largest multiplier.
        tiles(width, height, 85.0, 170.0)
            .max(patches(width, height, 1_536.0, 2.46))
            .max(patches(width, height, 30_000.0, 1.2))
    };
    // The formulas give small positive values, so the cast is exact.
    Some(tokens as u64)
}

/// 32px patches, at most `cap` of them, times the model's multiplier.
fn patches(width: f64, height: f64, cap: f64, multiplier: f64) -> f64 {
    let count = ((width / 32.0).ceil() * (height / 32.0).ceil()).min(cap);
    (count * multiplier).ceil()
}

/// `high` detail tiles: the image fits 2048x2048, its short side
/// shrinks to 768px, and each 512px tile costs `per_tile` over `base`.
fn tiles(width: f64, height: f64, base: f64, per_tile: f64) -> f64 {
    let fit = (2_048.0 / width.max(height)).min(1.0);
    let (width, height) = (width * fit, height * fit);
    let short = (768.0 / width.min(height)).min(1.0);
    let (width, height) = (width * short, height * short);
    base + per_tile * (width / 512.0).ceil() * (height / 512.0).ceil()
}

/// The pixel size from the image header of a base64 `data:` URI.
fn dimensions(data_uri: &str) -> Option<(u32, u32)> {
    use base64::Engine as _;
    let (_, encoded) = data_uri.strip_prefix("data:")?.split_once(";base64,")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_large_image_fits_the_tile_grid_before_the_count() {
        // 4096x2048 fits 2048x1024, then 1536x768: 3 by 2 tiles.
        assert_eq!(tiles(4_096.0, 2_048.0, 85.0, 170.0), 85.0 + 170.0 * 6.0);
    }

    #[test]
    fn the_patch_count_stops_at_the_cap() {
        assert_eq!(patches(4_096.0, 4_096.0, 1_536.0, 1.0), 1_536.0);
    }

    #[test]
    fn an_image_without_a_readable_header_has_no_size() {
        assert_eq!(
            image_tokens("gpt-unlisted", "data:image/png;base64,AAAA"),
            None
        );
        assert_eq!(
            image_tokens("gpt-unlisted", "https://example.com/a.png"),
            None
        );
    }
}
