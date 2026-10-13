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

/// The shortest wait before a screenshot after an input.
const SETTLE_MIN: std::time::Duration = std::time::Duration::from_millis(300);
/// The time between two frames that must match for a settled screen.
const SETTLE_POLL: std::time::Duration = std::time::Duration::from_millis(200);
/// The limit for a screen that keeps changing.
const SETTLE_MAX: std::time::Duration = std::time::Duration::from_secs(3);

/// One frame after the screen stops changing, and whether it settles.
/// A page reacts to an input over several frames, and a screenshot in
/// the middle shows a state that is gone. Anthropic's reference loop
/// waits a fixed two seconds after each action; this waits at least
/// 300 ms, then until two frames 200 ms apart match ([`frames_match`]).
/// It stops polling at the 3 s limit; a poll that starts before the
/// limit can finish after it. A loading page keeps moving: the tab
/// spinner turns until the load ends. A capture error ends the loop.
pub async fn settled_frame<Capture, Captured, Error>(
    mut capture: Capture,
) -> Result<(Vec<u8>, bool), Error>
where
    Capture: FnMut() -> Captured,
    Captured: std::future::Future<Output = Result<Vec<u8>, Error>>,
{
    let deadline = pagis_rt::Instant::now() + SETTLE_MAX;
    pagis_rt::sleep(SETTLE_MIN).await;
    let mut frame = capture().await?;
    let mut settled = false;
    while pagis_rt::Instant::now() < deadline {
        pagis_rt::sleep(SETTLE_POLL).await;
        let next = capture().await?;
        let matched = frames_match(&frame, &next);
        frame = next;
        if matched {
            settled = true;
            break;
        }
    }
    Ok((frame, settled))
}
