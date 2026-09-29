//! The preflight price of one authorized suite run.
//!
//! The manifest declares caps; it authorizes no spend. Before the first
//! model call the caller prices every route the responsible Agent's
//! alias may use, and refuses a run that is unpriced or unaffordable.
//! The refusal names the missing capability, so an unscored report says
//! why it called no model.

use crate::SuiteLimits;

/// The configured price of one route, in USD per million tokens. The
/// caller reads it from the model registry the daemon itself uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RouteRate {
    pub input_usd_per_mtok: f64,
    pub output_usd_per_mtok: f64,
}

impl RouteRate {
    /// The USD cost of one metered amount of tokens at this rate.
    pub fn cost(&self, input_tokens: u64, output_tokens: u64) -> f64 {
        (input_tokens as f64 * self.input_usd_per_mtok
            + output_tokens as f64 * self.output_usd_per_mtok)
            / 1e6
    }
}

/// One route of the alias with the price the run reserves against.
#[derive(Clone, Debug, PartialEq)]
pub struct PricedRoute {
    pub route: String,
    pub rate: RouteRate,
}

/// What an affordable suite may spend, worst case, under the manifest
/// caps and the priciest route of the alias.
#[derive(Clone, Debug, PartialEq)]
pub struct Priced {
    pub alias: String,
    pub routes: Vec<PricedRoute>,
    /// The priciest rate of the alias. A run settles its metered tokens
    /// at this rate, because the router may fall back to any route of
    /// the alias and the settled cost must never read as too low.
    pub settle_rate: RouteRate,
    /// The reserve of one chronology repeat: every allowed token of the
    /// repeat, at the priciest route.
    pub usd_per_chronology_repeat: f64,
    /// The worst case of the complete suite.
    pub usd_for_suite: f64,
}

/// The number of chronology repeats the suite runs.
fn repeats(suite: &SuiteLimits) -> u64 {
    (suite.development_chronologies + suite.held_out_chronologies) as u64
        * suite.repeats_per_chronology as u64
}

/// Price the alias, or name the capability the run lacks. `rate` reads
/// the configured price of one `provider/model` route and returns
/// `None` when that route has no price.
pub fn preflight(
    alias: &str,
    routes: &[String],
    suite: &SuiteLimits,
    rate: impl Fn(&str) -> Option<RouteRate>,
) -> Result<Priced, String> {
    if routes.is_empty() {
        return Err(format!("a resolved model route for alias `{alias}`"));
    }
    let mut priced = Vec::new();
    for route in routes {
        // An unpriced route stops the run before any call: a fallback
        // whose price is unknown cannot be reserved against.
        let rate =
            rate(route).ok_or_else(|| format!("a priced model route: {route} is unpriced"))?;
        priced.push(PricedRoute {
            route: route.clone(),
            rate,
        });
    }
    let costliest = priced
        .iter()
        .max_by(|left, right| {
            let cost = |priced: &PricedRoute| {
                priced.rate.cost(
                    suite.max_input_tokens_per_chronology_repeat,
                    suite.max_output_tokens_per_chronology_repeat,
                )
            };
            cost(left).total_cmp(&cost(right))
        })
        .expect("a priced alias has a route")
        .rate;
    let per_repeat = costliest.cost(
        suite.max_input_tokens_per_chronology_repeat,
        suite.max_output_tokens_per_chronology_repeat,
    );
    if per_repeat > suite.max_usd_per_chronology_repeat {
        return Err(format!(
            "an affordable route: one repeat costs at most ${per_repeat:.2} and the manifest \
             allows ${:.2}",
            suite.max_usd_per_chronology_repeat
        ));
    }
    let suite_usd = per_repeat * repeats(suite) as f64;
    if suite_usd > suite.max_total_usd {
        return Err(format!(
            "an affordable suite: the run costs at most ${suite_usd:.2} and the manifest allows \
             ${:.2}",
            suite.max_total_usd
        ));
    }
    Ok(Priced {
        alias: alias.to_owned(),
        routes: priced,
        settle_rate: costliest,
        usd_per_chronology_repeat: per_repeat,
        usd_for_suite: suite_usd,
    })
}
