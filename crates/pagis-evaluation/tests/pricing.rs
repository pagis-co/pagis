//! The preflight price of an authorized run.

use pagis_evaluation::pricing::{RouteRate, preflight};
use pagis_evaluation::{SuiteLimits, load_manifest};
use std::path::PathBuf;

const SONNET: &str = "anthropic/claude-sonnet-4-6";
const GPT: &str = "openai/gpt-5";

fn suite() -> SuiteLimits {
    let manifest = load_manifest(
        &PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets the manifest dir"),
        )
        .join("suites/continuous-learning/release.json"),
    )
    .expect("load the manifest");
    manifest.suite
}

/// The prices of the two routes the seeded `default` alias resolves to.
fn shipped_rate(route: &str) -> Option<RouteRate> {
    match route {
        SONNET => Some(RouteRate {
            input_usd_per_mtok: 3.0,
            output_usd_per_mtok: 15.0,
        }),
        GPT => Some(RouteRate {
            input_usd_per_mtok: 1.25,
            output_usd_per_mtok: 10.0,
        }),
        _ => None,
    }
}

#[test]
fn the_reserve_is_the_repeat_cap_at_the_most_expensive_route() {
    // The manifest token caps price a Sonnet repeat above the $1.10 dollar
    // cap, so this fixture lifts the dollar cap to price the reserve.
    let suite = SuiteLimits {
        max_usd_per_chronology_repeat: 3.0,
        max_total_usd: 250.0,
        ..suite()
    };
    let routes = [SONNET.to_string(), GPT.to_string()];

    let priced = preflight("default", &routes, &suite, shipped_rate).expect("the alias is priced");

    // 512000 input at $3 and 48000 output at $15 for each million tokens.
    assert!((priced.usd_per_chronology_repeat - 2.256).abs() < 1e-9);
    assert_eq!(priced.settle_rate, shipped_rate(SONNET).unwrap());
    assert!((priced.usd_for_suite - 2.256 * 74.0).abs() < 1e-9);
    assert_eq!(priced.routes.len(), 2);
    assert_eq!(priced.alias, "default");
}

#[test]
fn an_unpriced_route_stops_the_run_before_a_call() {
    let routes = [SONNET.to_string(), "local/unlisted".to_string()];

    let refusal = preflight("default", &routes, &suite(), shipped_rate).expect_err("unpriced");

    assert_eq!(refusal, "a priced model route: local/unlisted is unpriced");
}

#[test]
fn an_alias_without_a_route_is_refused() {
    let refusal = preflight("default", &[], &suite(), shipped_rate).expect_err("no route");

    assert_eq!(refusal, "a resolved model route for alias `default`");
}

#[test]
fn a_route_above_the_repeat_cap_is_refused() {
    let costly = |_: &str| {
        Some(RouteRate {
            input_usd_per_mtok: 30.0,
            output_usd_per_mtok: 150.0,
        })
    };

    let refusal =
        preflight("default", &[SONNET.to_string()], &suite(), costly).expect_err("unaffordable");

    assert!(
        refusal.starts_with("an affordable route: one repeat costs at most $22.56"),
        "{refusal}"
    );
}

#[test]
fn a_suite_above_the_total_cap_is_refused() {
    // The metered Agent and pre-grader calls share the repeat cap.
    // Each repeat fits that cap, but 74 repeats do not fit this suite.
    let suite = SuiteLimits {
        max_usd_per_chronology_repeat: 2.3,
        max_total_usd: 10.0,
        ..suite()
    };

    let refusal =
        preflight("default", &[SONNET.to_string()], &suite, shipped_rate).expect_err("over suite");

    assert!(
        refusal.starts_with("an affordable suite: the run costs at most $166.94"),
        "{refusal}"
    );
}
