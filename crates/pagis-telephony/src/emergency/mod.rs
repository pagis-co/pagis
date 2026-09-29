//! Pagis never calls an emergency number (ADR-0018). There is no
//! setting, no override and no approval that permits one.
//!
//! One predicate decides, [`is_emergency`], and the daemon applies it
//! two times: in the capability broker before the call tool acts, and
//! again in the dial path, so no caller goes around the broker. Every
//! path that starts or redirects a Call must use the same predicate.
//!
//! The data is libphonenumber's `ShortNumberMetadata.xml` (Apache-2.0),
//! compiled into [`table::TERRITORIES`] by `cargo xtask emergency-numbers`
//! and stamped with its upstream version. It is a constant of the
//! program and never enters the user's database.

mod table;

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

pub use table::SOURCE_VERSION;

/// One row of the generated table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Territory {
    /// The ISO 3166-1 alpha-2 region, e.g. `US`.
    pub region: &'static str,
    /// The country calling code, e.g. `1`.
    pub calling_code: u16,
    /// True for the territory a calling code maps to when several
    /// territories share it: `US` for `1`, `GB` for `44`.
    pub main_for_code: bool,
    /// The emergency pattern, in libphonenumber's regex dialect, or
    /// `None` for the one territory without one.
    pub emergency: Option<&'static str>,
}

/// The regions where the source data demands an exact match, as
/// libphonenumber's `ShortNumberInfo` hard-codes them. Everywhere else
/// a number that starts with an emergency number connects to it.
const EXACT_MATCH_REGIONS: [&str; 3] = ["BR", "CL", "NI"];

/// The refusal (ADR-0018). It reaches the model as a tool result under
/// [`EmergencyRefused::CODE`], and the message tells the model what to
/// do instead of trying another form of the number.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "Pagis never calls an emergency number, and {to} is one in {region}. The person must dial it now, from a telephone that the person holds."
)]
pub struct EmergencyRefused {
    /// What the Agent asked to dial, as it gave it.
    pub to: String,
    /// The region under which the number is an emergency number.
    pub region: &'static str,
}

impl EmergencyRefused {
    /// A code of its own, and not `invalid_request`: that one reads as
    /// "your arguments were malformed" and invites a retry with
    /// different formatting, which is the exact hazard here.
    pub const CODE: &str = "emergency_number_refused";
}

/// The region of a held number: the territory of its calling code, and
/// the main territory when several share the code. A Canadian line
/// therefore reads as `US`, whose emergency numbers are the same. The
/// number record holds no region, because the E.164 number already
/// says where the line is.
pub fn region_of(e164: &str) -> Option<&'static str> {
    let digits = e164.trim().strip_prefix('+')?;
    table::TERRITORIES
        .iter()
        .filter(|territory| territory.main_for_code)
        .find(|territory| {
            digits
                .strip_prefix(territory.calling_code.to_string().as_str())
                .is_some()
        })
        .map(|territory| territory.region)
}

/// True when `number` reaches an emergency service in `region`.
///
/// A number without a `+` is read as dialed on a keypad, with
/// libphonenumber's `connectsToEmergencyNumber` rule: a prefix match,
/// except in the regions that demand an exact one. So `911` and
/// `9116666666` both refuse in `US`.
///
/// A number with a `+` is one step stricter than libphonenumber, which
/// reports false for every such number. Pagis tests the digits and, when
/// they start with the region's calling code, the national form under
/// the region. Both tests are exact: the carrier dials an international
/// number as one number, so extra digits do not connect to a short
/// code, and a prefix match would refuse every Paris number from a
/// French line. So `+1911` refuses in `US` and `+33155123456` passes in
/// `FR`.
///
/// A region the table does not know, or one with no emergency pattern,
/// refuses nothing.
pub fn is_emergency(number: &str, region: &str) -> bool {
    let number = number.trim();
    let digits: String = number.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return false;
    }
    let Some(matcher) = MATCHERS.get(region) else {
        return false;
    };
    if number.starts_with('+') {
        let national = digits.strip_prefix(matcher.calling_code.as_str());
        return matcher.exact.is_match(&digits)
            || national.is_some_and(|national| matcher.exact.is_match(national));
    }
    matcher.as_dialed.is_match(&digits)
}

/// The one check every path that starts a Call makes: `to` is what the
/// Agent asked to reach, and `held_e164` is the line it calls from.
pub fn check_dial(to: &str, held_e164: &str) -> Result<(), EmergencyRefused> {
    let Some(region) = region_of(held_e164) else {
        return Ok(());
    };
    if is_emergency(to, region) {
        return Err(EmergencyRefused {
            to: to.trim().to_string(),
            region,
        });
    }
    Ok(())
}

struct Matcher {
    calling_code: String,
    /// The whole number is an emergency number.
    exact: Regex,
    /// The number, as dialed on a keypad, connects to an emergency
    /// number: a prefix match, or an exact one where the region demands
    /// it.
    as_dialed: Regex,
}

static MATCHERS: LazyLock<HashMap<&'static str, Matcher>> = LazyLock::new(|| {
    table::TERRITORIES
        .iter()
        .filter_map(|territory| {
            let pattern = territory.emergency?;
            let exact = Regex::new(&format!("^(?:{pattern})$")).unwrap_or_else(|error| {
                panic!("emergency pattern of {}: {error}", territory.region)
            });
            let as_dialed = if EXACT_MATCH_REGIONS.contains(&territory.region) {
                exact.clone()
            } else {
                Regex::new(&format!("^(?:{pattern})")).unwrap_or_else(|error| {
                    panic!("emergency pattern of {}: {error}", territory.region)
                })
            };
            Some((
                territory.region,
                Matcher {
                    calling_code: territory.calling_code.to_string(),
                    exact,
                    as_dialed,
                },
            ))
        })
        .collect()
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_in_the_table_compiles() {
        let with_pattern = table::TERRITORIES
            .iter()
            .filter(|territory| territory.emergency.is_some())
            .count();

        assert_eq!(MATCHERS.len(), with_pattern);
        assert!(with_pattern >= 240, "{with_pattern} patterns");
    }

    /// The known limit (ADR-0018): where the source data has no
    /// pattern, Pagis refuses nothing. Guinea is in the short number
    /// data without one; the other four are not in it at all.
    #[test]
    fn the_territories_without_a_pattern_are_the_known_few() {
        let without: Vec<&str> = table::TERRITORIES
            .iter()
            .filter(|territory| territory.emergency.is_none())
            .map(|territory| territory.region)
            .collect();

        assert_eq!(without, ["GN", "GQ", "IO", "TA", "TK"]);
    }

    #[test]
    fn the_table_is_stamped_with_its_upstream_version() {
        assert!(SOURCE_VERSION.starts_with('v'), "{SOURCE_VERSION}");
    }

    #[test]
    fn the_keypad_form_refuses_as_libphonenumber_does() {
        assert!(is_emergency("911", "US"));
        assert!(is_emergency("112", "US"));
        assert!(is_emergency("9116666666", "US"));
        assert!(is_emergency("911", "CA"));
        assert!(is_emergency("999", "GB"));
        assert!(is_emergency("112", "GB"));
        assert!(is_emergency("110", "DE"));
        assert!(is_emergency("112", "DE"));
        assert!(is_emergency("15", "FR"));
        assert!(is_emergency("17", "FR"));
        assert!(is_emergency("18", "FR"));
        assert!(is_emergency("112", "FR"));
        assert!(is_emergency("000", "AU"));
        assert!(is_emergency("112", "AU"));
        assert!(is_emergency("190", "BR"));
        assert!(is_emergency("192", "BR"));
        assert!(is_emergency("131", "CL"));
        assert!(is_emergency("118", "NI"));
    }

    #[test]
    fn brazil_chile_and_nicaragua_demand_an_exact_match() {
        assert!(!is_emergency("1900", "BR"));
        assert!(!is_emergency("9111", "BR"));
        assert!(!is_emergency("1311", "CL"));
        assert!(!is_emergency("1180", "NI"));
    }

    #[test]
    fn the_e164_form_refuses_where_libphonenumber_reports_false() {
        assert!(is_emergency("+1911", "US"));
        assert!(is_emergency("+1112", "US"));
        assert!(is_emergency("+1 911", "US"));
        assert!(is_emergency("+911", "US"));
        assert!(is_emergency("+1911", "CA"));
        assert!(is_emergency("+44999", "GB"));
        assert!(is_emergency("+49110", "DE"));
        assert!(is_emergency("+3315", "FR"));
        assert!(is_emergency("+33112", "FR"));
        assert!(is_emergency("+61000", "AU"));
        assert!(is_emergency("+55190", "BR"));
    }

    #[test]
    fn the_e164_form_is_one_number_so_extra_digits_do_not_connect() {
        // A Paris landline starts with 1 5, which is also the SAMU.
        assert!(!is_emergency("+33155123456", "FR"));
        // An Oakland number from a French line is not the SAMU either.
        assert!(!is_emergency("+15105550123", "FR"));
        assert!(!is_emergency("+19116666666", "US"));
        assert!(!is_emergency("+551900", "BR"));
    }

    #[test]
    fn ordinary_numbers_pass() {
        assert!(!is_emergency("+14155550123", "US"));
        assert!(!is_emergency("4155550123", "US"));
        assert!(!is_emergency("+442079460000", "GB"));
        assert!(!is_emergency("+493012345678", "DE"));
        assert!(!is_emergency("+61212345678", "AU"));
        assert!(!is_emergency("+5511912345678", "BR"));
        assert!(!is_emergency("+16135550123", "CA"));
    }

    #[test]
    fn a_number_in_another_country_is_read_under_the_held_region_only() {
        assert!(!is_emergency("+44999", "US"));
        assert!(!is_emergency("+1911", "GB"));
    }

    #[test]
    fn no_digits_and_no_region_refuse_nothing() {
        assert!(!is_emergency("", "US"));
        assert!(!is_emergency("+", "US"));
        assert!(!is_emergency("abc", "US"));
        assert!(!is_emergency("911", "ZZ"));
        assert!(!is_emergency("911", ""));
        // Guinea has no emergency pattern in the source data.
        assert!(!is_emergency("112", "GN"));
    }

    #[test]
    fn the_region_of_a_line_is_the_main_territory_of_its_calling_code() {
        assert_eq!(region_of("+14155550123"), Some("US"));
        assert_eq!(region_of("+16135550123"), Some("US"));
        assert_eq!(region_of("+442079460000"), Some("GB"));
        assert_eq!(region_of("+493012345678"), Some("DE"));
        assert_eq!(region_of("+33155123456"), Some("FR"));
        assert_eq!(region_of("+61212345678"), Some("AU"));
        assert_eq!(region_of("+5511912345678"), Some("BR"));
        assert_eq!(region_of("+224621234567"), Some("GN"));
        assert_eq!(region_of("14155550123"), None);
        assert_eq!(region_of("+"), None);
        assert_eq!(region_of("+0"), None);
    }

    #[test]
    fn the_dial_check_names_the_number_and_the_region() {
        let refused = check_dial(" +1911 ", "+14155550123").unwrap_err();

        assert_eq!(refused.to, "+1911");
        assert_eq!(refused.region, "US");
        assert_eq!(EmergencyRefused::CODE, "emergency_number_refused");
        assert!(refused.to_string().contains("The person must dial it now"));
        assert_eq!(check_dial("+14155550124", "+14155550123"), Ok(()));
        assert_eq!(check_dial("+1911", "+0"), Ok(()));
    }
}
