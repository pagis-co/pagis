//! What a Person spends: the tokens and the cost of every model
//! call, attributed to the Workspace and the Run that asked for it.
//!
//! The router already computes the cost of one call from the model's own
//! price table. Nothing kept it, so an installation whose Administrator
//! supplies the keys for everybody could not say who spent the budget.
//! This is the record that answers it.
//!
//! One row is one model call, because that is the grain the router
//! reports. A read adds the rows up: per Run for the person's own list,
//! per Workspace for the Administrator's roster, and over a period for
//! the Spend Cap.

use async_trait::async_trait;

use crate::id::{RunId, UsageId, WorkspaceId};
use crate::store::StoreError;
use crate::time::UnixMillis;

/// What one model call spent. The token counts are the provider's own;
/// `cost_usd` is what the serving model's price makes of them, or `None`
/// when no layer of the model metadata prices the model: an unknown
/// cost, never zero.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRecord {
    pub id: UsageId,
    pub workspace_id: WorkspaceId,
    pub run_id: RunId,
    /// The provider that served the call, when the router named one.
    pub provider: Option<String>,
    /// The concrete model, not the alias.
    pub model: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd: Option<f64>,
    pub created_at: UnixMillis,
}

/// The sum of a set of Usage Records.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageTotal {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// The sum of the known costs.
    pub cost_usd: f64,
    /// How many model calls the total holds.
    pub calls: i64,
    /// How many of those calls have an unknown cost, which `cost_usd`
    /// does not count.
    pub unpriced_calls: i64,
}

/// One Workspace's total over a period, for the Administrator's read.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceUsage {
    pub workspace_id: WorkspaceId,
    pub total: UsageTotal,
}

/// One Run's total over a period, for the person's own read.
#[derive(Debug, Clone, PartialEq)]
pub struct RunUsage {
    pub run_id: RunId,
    pub total: UsageTotal,
    /// When the last call of the Run was recorded.
    pub last_at: UnixMillis,
}

/// A half-open period, `[from, to)`, in Unix milliseconds. A caller
/// names both ends, so a month, a day and "everything so far" are the
/// same read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsagePeriod {
    pub from: UnixMillis,
    pub to: UnixMillis,
}

#[async_trait]
pub trait UsageStore: Send + Sync {
    /// Keep what one model call spent.
    async fn record(&self, usage: &UsageRecord) -> Result<(), StoreError>;
    /// One Workspace's total over the period. The Spend Cap reads this,
    /// and so does the person's own Usage page.
    async fn total_for_workspace(
        &self,
        workspace_id: &WorkspaceId,
        period: UsagePeriod,
    ) -> Result<UsageTotal, StoreError>;
    /// Every Workspace's total over the period, highest cost first.
    ///
    /// This is the one read of this trait that crosses Workspaces, and
    /// it is the Administrator's roster read: spend per person for a
    /// period, which no per-Workspace read can answer. The route behind
    /// it takes the `Administrator` extractor, so a Member never reaches
    /// it. Every other signature takes the Workspace.
    async fn totals_by_workspace(
        &self,
        period: UsagePeriod,
    ) -> Result<Vec<WorkspaceUsage>, StoreError>;
    /// One Workspace's Runs over the period, newest first.
    async fn runs_for_workspace(
        &self,
        workspace_id: &WorkspaceId,
        period: UsagePeriod,
        limit: u32,
    ) -> Result<Vec<RunUsage>, StoreError>;
}

impl UsagePeriod {
    /// The calendar month one instant falls in, read in a named
    /// timezone. The Spend Cap is a month's allowance, and the month is
    /// the person's own: an installation in Sydney must not reset its
    /// caps in the middle of a working day.
    ///
    /// A timezone name the database does not know falls back to UTC,
    /// which is what the Workspace timezone defaults to anyway.
    pub fn calendar_month(at: UnixMillis, timezone: &str) -> Self {
        use chrono::{Datelike, TimeZone};

        let zone: chrono_tz::Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
        let local = zone.timestamp_millis_opt(at).single().unwrap_or_else(|| {
            zone.from_utc_datetime(
                &chrono::DateTime::from_timestamp_millis(at)
                    .unwrap_or_default()
                    .naive_utc(),
            )
        });
        let start = month_start(&zone, local.year(), local.month());
        let (next_year, next_month) = match local.month() {
            12 => (local.year() + 1, 1),
            month => (local.year(), month + 1),
        };
        Self {
            from: start,
            to: month_start(&zone, next_year, next_month),
        }
    }
}

/// Midnight on the first of one month in one zone, in Unix
/// milliseconds. A zone that skips that instant (a DST jump on the
/// first of the month) answers the first instant that exists after it.
fn month_start(zone: &chrono_tz::Tz, year: i32, month: u32) -> UnixMillis {
    use chrono::{NaiveDate, TimeZone};

    let date = NaiveDate::from_ymd_opt(year, month, 1).expect("month 1..=12 has a first day");
    let naive = date.and_hms_opt(0, 0, 0).expect("midnight exists");
    zone.from_local_datetime(&naive)
        .earliest()
        .map(|at| at.timestamp_millis())
        .unwrap_or_else(|| {
            // A gap at local midnight: take the UTC reading of it, which
            // is at most one hour early and never lands in the month
            // before.
            chrono::Utc.from_utc_datetime(&naive).timestamp_millis()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One instant inside a month answers that whole month, and the
    /// period is half-open: the first of the next month is not in it.
    #[test]
    fn a_calendar_month_runs_from_its_first_to_the_next_first() {
        // 2026-09-22T12:00:00Z
        let period = UsagePeriod::calendar_month(1_790_078_400_000, "UTC");

        assert_eq!(period.from, 1_788_220_800_000); // 2026-09-01T00:00:00Z
        assert_eq!(period.to, 1_790_812_800_000); // 2026-10-01T00:00:00Z
        assert!(period.from <= 1_790_078_400_000 && 1_790_078_400_000 < period.to);
    }

    /// December rolls into January of the next year.
    #[test]
    fn december_rolls_into_the_next_year() {
        // 2026-12-31T23:00:00Z
        let period = UsagePeriod::calendar_month(1_798_758_000_000, "UTC");

        assert_eq!(period.to, 1_798_761_600_000); // 2027-01-01T00:00:00Z
        assert!(period.from < period.to);
    }

    /// The month belongs to the Workspace timezone, so a person east of
    /// UTC starts their month before UTC does.
    #[test]
    fn the_month_follows_the_named_timezone() {
        let utc = UsagePeriod::calendar_month(1_790_078_400_000, "UTC");
        let sydney = UsagePeriod::calendar_month(1_790_078_400_000, "Australia/Sydney");

        assert!(sydney.from < utc.from, "{sydney:?} {utc:?}");
    }

    /// A name no zone database holds reads as UTC rather than failing.
    #[test]
    fn an_unknown_timezone_reads_as_utc() {
        assert_eq!(
            UsagePeriod::calendar_month(1_790_078_400_000, "Mars/Olympus"),
            UsagePeriod::calendar_month(1_790_078_400_000, "UTC")
        );
    }
}
