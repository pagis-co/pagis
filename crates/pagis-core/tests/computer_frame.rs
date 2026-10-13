//! The settled-frame rule on native targets and under Node.

use pagis_core::computer::{frames_match, settled_frame};
use pagis_rt::Instant;
use std::future::ready;
use std::time::Duration;

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
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn frames_that_differ_by_a_caret_match() {
    assert!(frames_match(&png_with_box(0, 0), &png_with_box(0, 0)));
    assert!(frames_match(&png_with_box(0, 0), &png_with_box(2, 20)));
}

/// A menu that opens changes a large area.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn frames_that_differ_by_a_menu_do_not_match() {
    assert!(!frames_match(&png_with_box(0, 0), &png_with_box(300, 200)));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn frames_that_are_not_images_match_only_when_equal() {
    assert!(frames_match(b"frame", b"frame"));
    assert!(!frames_match(b"frame", b"other"));
}

fn png(width: u32, height: u32, changed: u32) -> Vec<u8> {
    let mut image = image::RgbImage::new(width, height);
    for (index, pixel) in image.pixels_mut().enumerate() {
        if index < changed as usize {
            *pixel = image::Rgb([255, 0, 0]);
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn the_threshold_counts_pixels_and_includes_half_a_percent() {
    let unchanged = png(100, 100, 0);
    assert!(frames_match(&unchanged, &unchanged));
    assert!(frames_match(&unchanged, &png(100, 100, 49)));
    assert!(frames_match(&unchanged, &png(100, 100, 50)));
    assert!(!frames_match(&unchanged, &png(100, 100, 51)));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn frames_of_different_sizes_do_not_match() {
    assert!(!frames_match(&png(100, 100, 0), &png(100, 101, 0)));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn a_frame_that_does_not_decode_does_not_match_a_png() {
    let valid = png(100, 100, 0);
    assert!(!frames_match(&valid, b"not a PNG"));
    assert!(!frames_match(&valid[..valid.len() / 2], &valid));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test(start_paused = true))]
async fn a_settled_frame_waits_for_the_minimum_and_two_matching_frames() {
    let started = Instant::now();
    let mut captures = Vec::new();
    let expected = png(100, 100, 0);
    let (frame, settled) = settled_frame(|| {
        captures.push(started.elapsed());
        ready(Ok::<_, &str>(expected.clone()))
    })
    .await
    .unwrap();

    assert_eq!(frame, expected);
    assert!(settled);
    assert_eq!(captures.len(), 2);
    assert!(captures[0] >= Duration::from_millis(300));
    assert!(captures[1] - captures[0] >= Duration::from_millis(200));
    #[cfg(not(target_arch = "wasm32"))]
    assert_eq!(
        captures,
        [Duration::from_millis(300), Duration::from_millis(500)]
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test(start_paused = true)]
async fn a_settled_frame_returns_the_last_of_two_matching_frames() {
    let frames = [png(100, 100, 0), png(100, 100, 100), png(100, 100, 149)];
    let mut next = frames.iter();
    let started = Instant::now();
    let (frame, settled) = settled_frame(|| ready(Ok::<_, &str>(next.next().unwrap().clone())))
        .await
        .unwrap();

    assert!(settled);
    assert_eq!(frame, frames[2]);
    assert_eq!(started.elapsed(), Duration::from_millis(700));
    assert!(next.next().is_none());
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test(start_paused = true)]
async fn a_moving_screen_stops_at_the_limit_and_returns_the_last_frame() {
    let started = Instant::now();
    let mut captures = Vec::new();
    let (frame, settled) = settled_frame(|| {
        captures.push(started.elapsed());
        ready(Ok::<_, &str>(
            format!("frame-{}", captures.len()).into_bytes(),
        ))
    })
    .await
    .unwrap();

    assert!(!settled);
    assert_eq!(frame, b"frame-15");
    // The final poll starts before the limit and completes after it.
    assert_eq!(started.elapsed(), Duration::from_millis(3100));
    assert_eq!(captures.len(), 15);
    assert!(
        captures
            .windows(2)
            .all(|pair| pair[1] - pair[0] == Duration::from_millis(200))
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test(start_paused = true))]
async fn a_capture_error_ends_the_loop_with_the_same_error() {
    for failing_capture in [1, 2] {
        let started = Instant::now();
        let mut captures = 0;
        let result = settled_frame(|| {
            captures += 1;
            ready(if captures == failing_capture {
                Err("capture failed")
            } else {
                Ok(b"frame".to_vec())
            })
        })
        .await;

        assert_eq!(result, Err("capture failed"));
        assert_eq!(captures, failing_capture);
        #[cfg(not(target_arch = "wasm32"))]
        assert_eq!(
            started.elapsed(),
            Duration::from_millis(100 + 200 * failing_capture)
        );
        #[cfg(target_arch = "wasm32")]
        assert!(started.elapsed() >= Duration::from_millis(100 + 200 * failing_capture));
    }
}
