//! The release evaluation of continuous learning: it loads and checks
//! the suite, runs each chronology through a driver, and reports the
//! result against the manifest gates.

pub mod grading;
pub mod pricing;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: String,
        source: serde_json::Error,
    },
    #[error("invalid evaluation suite: {0}")]
    Invalid(String),
}

#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub profile: String,
    pub corpus: CorpusFile,
    pub model_policy: ModelPolicy,
    pub scoring: Scoring,
    pub grading: Grading,
    pub required_metrics: RequiredMetrics,
    pub suite: SuiteLimits,
    pub release_thresholds: ReleaseThresholds,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CorpusFile {
    pub file: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Scoring {
    pub fraction_gate: String,
    pub zero_tolerance_gates: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Grading {
    pub owner_grades: String,
    pub fixed_probe_sample_per_pooled_arm: usize,
    pub pre_grader_route: String,
    pub pre_grader_fills: String,
    pub ungraded_run: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RequiredMetrics {
    pub intervention_precision: RequiredMetric,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RequiredMetric {
    pub denominator: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModelPolicy {
    pub route: String,
    pub spend_authorized_by_manifest: bool,
    pub unpriced_route: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SuiteLimits {
    pub development_chronologies: usize,
    pub held_out_chronologies: usize,
    pub repeats_per_chronology: u8,
    pub max_evidence_updates_per_chronology: usize,
    pub minimum_input_forms_per_chronology: usize,
    pub max_model_calls_per_chronology_repeat: u64,
    pub max_input_tokens_per_chronology_repeat: u64,
    pub max_output_tokens_per_chronology_repeat: u64,
    pub max_wall_seconds_per_chronology_repeat: u64,
    pub max_usd_per_chronology_repeat: f64,
    pub max_total_usd: f64,
    pub max_total_wall_seconds: u64,
}

/// The release thresholds that the owner applies to the pooled held-out
/// grades.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReleaseThresholds {
    pub deterministic_invariant_pass_fraction: f64,
    pub material_output_grounded_fraction: f64,
    pub unsupported_persisted_or_delivered_associations: u64,
    pub final_supported_connection_recall_min: f64,
    pub useful_help_selection_fraction_min: f64,
    pub delivered_intervention_precision_min: f64,
    pub restraint_probe_success_fraction: f64,
    pub revision_probe_success_fraction: f64,
    pub duplicate_visible_interventions: u64,
    pub future_evidence_leaks: u64,
    pub unauthorized_effects_or_disclosures: u64,
    pub unscored_required_runs_for_release: u64,
    pub resource_bound_compliance_fraction: f64,
}

/// One fraction gate over the pooled held-out observations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FractionGate {
    pub point_estimate: f64,
    pub lower_bound: f64,
    pub threshold: f64,
    pub passed: bool,
}

/// Return the Wilson score lower bound for a binomial fraction.
///
/// `successes` must not exceed `total`, and `total` must be nonzero.
pub fn wilson_lower_bound(successes: u64, total: u64, z: f64) -> Option<f64> {
    if total == 0 || successes > total || !z.is_finite() || z < 0.0 {
        return None;
    }
    let total = total as f64;
    let point = successes as f64 / total;
    let z_squared = z * z;
    let centre = point + z_squared / (2.0 * total);
    let margin = z * (point * (1.0 - point) / total + z_squared / (4.0 * total * total)).sqrt();
    Some((centre - margin) / (1.0 + z_squared / total))
}

/// Apply a release fraction threshold to pooled held-out observations
/// with the lower bound of a two-sided 90% Wilson interval.
/// A threshold of one stays an exact zero-failure gate.
pub fn fraction_gate(successes: u64, total: u64, threshold: f64) -> Option<FractionGate> {
    if !(0.0..=1.0).contains(&threshold) {
        return None;
    }
    let point_estimate = successes as f64 / total as f64;
    let lower_bound = wilson_lower_bound(successes, total, 1.645)?;
    let passed = if threshold == 1.0 {
        successes == total
    } else {
        lower_bound >= threshold
    };
    Some(FractionGate {
        point_estimate,
        lower_bound,
        threshold,
        passed,
    })
}

#[derive(Clone, Debug, Deserialize)]
pub struct Corpus {
    pub schema_version: u32,
    pub version: String,
    pub cases: Vec<Chronology>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Chronology {
    pub id: String,
    pub partition: Partition,
    pub zone: String,
    pub evidence: Vec<Evidence>,
    pub probes: Vec<Probe>,
    pub expected_interventions: Vec<ExpectedIntervention>,
}

/// One intervention that the frozen corpus expects the Agent to
/// deliver during a chronology.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExpectedIntervention {
    pub id: String,
    pub after_evidence: usize,
    pub not_before: String,
    pub not_after: String,
    pub purpose: String,
    pub required_support: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Partition {
    Development,
    HeldOut,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Evidence {
    pub source_id: String,
    pub source_version: String,
    pub source_ref: String,
    pub input_form: String,
    pub occurred_at: String,
    pub valid_at: String,
    pub acquired_at: String,
    pub scope: String,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Probe {
    pub id: String,
    pub role: ProbeRole,
    pub after_evidence: usize,
    pub now: String,
    pub prompt: String,
    pub permissible_conclusions: Vec<String>,
    pub required_support: Vec<String>,
    pub counterevidence: Vec<String>,
    pub expected_connections: Vec<String>,
    pub allowed_alternatives: Vec<String>,
    pub prohibited: Vec<String>,
    pub uncertainty: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ProbeRole {
    Help,
    Revision,
    Restraint,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Usage {
    pub model_calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub source_reads: u64,
    pub elapsed_millis: u64,
    pub usd: f64,
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, other: Self) {
        self.model_calls += other.model_calls;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.source_reads += other.source_reads;
        self.elapsed_millis += other.elapsed_millis;
        self.usd += other.usd;
    }
}

#[derive(Clone, Debug)]
pub struct Authorization {
    pub max_usd: f64,
    pub priced_routes: BTreeSet<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Complete,
    Failed,
    Unscored,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProbeObservation {
    pub probe_id: String,
    pub role: ProbeRole,
    pub observed_output: Option<String>,
    pub observed_effects: Vec<String>,
    pub failure: Option<String>,
    pub proposed_grade: Option<String>,
    pub proposed_reason: Option<String>,
}

/// One visible agent message in the owner's conversation that answered
/// no probe: help the daemon delivered on its own.
#[derive(Clone, Debug, Serialize)]
pub struct DeliveredIntervention {
    /// How many evidence items were visible when the owner could read
    /// the intervention.
    pub after_evidence: usize,
    /// When the owner could read it, in Unix milliseconds.
    pub at: i64,
    pub text: String,
}

/// How the delivered interventions of one chronology compare with its
/// frozen expected intervention windows.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct InterventionAccounting {
    pub expected: usize,
    pub delivered: usize,
    pub matched: usize,
    pub early: usize,
    pub late: usize,
    pub unexpected: usize,
}

/// The Schedule pipeline stages observed during one chronology repeat.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct InterventionPipeline {
    pub schedules_created: usize,
    pub wake_ups_fired: usize,
    pub schedules: Vec<ScheduleObservation>,
    pub decisions: InterventionDecisions,
}

/// One Schedule made during a chronology repeat and whether its Wake-up ran.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ScheduleObservation {
    pub schedule_id: String,
    pub next_due_at: Option<i64>,
    pub fired: bool,
}

/// One expected intervention window copied into each repeat report.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ExpectedInterventionWindow {
    pub id: String,
    pub after_evidence: usize,
    pub not_before: String,
    pub not_after: String,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct InterventionDecisions {
    pub sent: usize,
    pub rescheduled: usize,
    pub silent: usize,
}

impl InterventionAccounting {
    /// Delivered interventions plus expected interventions that were
    /// not delivered in their window.
    pub fn denominator(&self) -> usize {
        self.delivered + self.expected - self.matched
    }
}

/// What durable memory the runs of one driver can observe.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StoreObservability {
    /// The daemon reads Subject Pages through the memory brief.
    SubjectPages,
}

/// The metric a pooled held-out suite with no delivered or expected
/// intervention cannot score. This is counted once at the suite, never
/// per run.
pub const UNSCORED_INTERVENTION_PRECISION: &str =
    "intervention precision: no delivered or expected intervention to score";

/// The brittleness count of one run: a quarantine, a hang, an
/// aborted turn, or a required metric the run cannot score is a system
/// failure. A wrong answer is not. Every evaluation run must report zero.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct SystemFailures {
    /// Work the store gave up on, such as a quarantined import item.
    pub quarantines: u64,
    /// Deliveries and probes that reached no settled reply in time.
    pub hangs: u64,
    /// Turns that ended without an answer: a message the daemon
    /// refused, an import that gave up, or a reply run that failed.
    /// Each line names the turn, by the delivery position or the probe
    /// id, and the failure text.
    pub aborted_turns: Vec<String>,
    /// The required metrics this run cannot score, each named. A metric
    /// the driver never observes is not applicable and is not one of
    /// them.
    pub unscored_metrics: Vec<String>,
}

impl SystemFailures {
    /// How many system failures this run holds. Zero is the gate.
    pub fn total(&self) -> u64 {
        self.quarantines
            + self.hangs
            + self.aborted_turns.len() as u64
            + self.unscored_metrics.len() as u64
    }

    /// Add the counts of one run to a suite total.
    pub fn add(&mut self, other: &Self) {
        self.quarantines += other.quarantines;
        self.hangs += other.hangs;
        self.aborted_turns
            .extend(other.aborted_turns.iter().cloned());
        self.unscored_metrics
            .extend(other.unscored_metrics.iter().cloned());
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RunReport {
    pub case_id: String,
    pub partition: Partition,
    pub repeat: u8,
    pub status: RunStatus,
    /// Each resource cap this repeat exceeded, with the observed value
    /// and limit.
    pub over: Vec<String>,
    pub missing_capabilities: Vec<String>,
    pub observations: Vec<ProbeObservation>,
    pub delivered_interventions: Vec<DeliveredIntervention>,
    pub expected_intervention_windows: Vec<ExpectedInterventionWindow>,
    pub intervention_accounting: InterventionAccounting,
    pub intervention_pipeline: InterventionPipeline,
    /// The brittleness count of this run. Zero is the gate.
    pub system_failures: SystemFailures,
    pub usage: Usage,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub status: RunStatus,
    pub manifest_version: u32,
    pub profile: String,
    pub release_thresholds: ReleaseThresholds,
    pub pre_grader_route: String,
    pub corpus_version: String,
    pub corpus_sha256: String,
    /// The driver that replayed the corpus.
    pub driver: String,
    /// The durable memory that the driver's runs can observe.
    pub store: StoreObservability,
    pub model_route: String,
    pub clock_version: String,
    pub zone_rule_version: String,
    /// The owner settings the driver applied before every chronology,
    /// which a default installation does not hold.
    pub run_context: Vec<String>,
    pub repeat_order: Vec<String>,
    /// Probe observations for which the pre-grader made no proposal.
    pub probes_without_proposal: usize,
    pub missing_capabilities: Vec<String>,
    /// The brittleness count of the whole suite, the sum of the runs.
    pub system_failures: SystemFailures,
    pub usage: Usage,
    pub runs: Vec<RunReport>,
}

#[derive(Clone, Debug)]
pub struct DriverResult {
    pub status: RunStatus,
    pub missing_capabilities: Vec<String>,
    pub observations: Vec<ProbeObservation>,
    pub delivered_interventions: Vec<DeliveredIntervention>,
    pub intervention_pipeline: InterventionPipeline,
    /// What the driver counts against the brittleness gate of this run.
    pub system_failures: SystemFailures,
    pub usage: Usage,
}

#[async_trait]
pub trait ChronologyDriver: Send {
    /// Start one case from isolated clean state and replay evidence in acquisition order.
    async fn run(&mut self, chronology: &Chronology, repeat: u8) -> DriverResult;
    /// How the report names this driver.
    fn name(&self) -> &str;
    /// What durable memory this driver's runs can observe.
    fn store(&self) -> StoreObservability;
    fn model_route(&self) -> &str;
    /// The concrete routes under test. The pre-grader must differ from
    /// each one.
    fn routes_under_test(&self) -> Vec<String> {
        vec![self.model_route().to_string()]
    }
    fn clock_version(&self) -> &str;
    fn zone_rule_version(&self) -> &str;
    /// Set the different route that proposes grades for this suite.
    /// Drivers that do not call a model can ignore it.
    fn set_pre_grader_route(&mut self, _route: &str) {}
    /// The owner settings this driver turns on before a chronology. A
    /// driver that changes no setting has none.
    fn run_context(&self) -> Vec<String> {
        Vec::new()
    }
}

pub fn load_manifest(path: &Path) -> Result<Manifest, Error> {
    read_json(path)
}

/// Load one manifest and the corpus file that it names.
pub fn load_suite(path: &Path) -> Result<(Manifest, Corpus, String), Error> {
    let manifest = load_manifest(path)?;
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let (corpus, hash) = load_corpus(&directory.join(&manifest.corpus.file))?;
    Ok((manifest, corpus, hash))
}

pub fn load_corpus(path: &Path) -> Result<(Corpus, String), Error> {
    let bytes = std::fs::read(path).map_err(|source| Error::Read {
        path: path.display().to_string(),
        source,
    })?;
    let hash = hex::encode(Sha256::digest(&bytes));
    let corpus = serde_json::from_slice(&bytes).map_err(|source| Error::Json {
        path: path.display().to_string(),
        source,
    })?;
    Ok((corpus, hash))
}

fn read_json<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<T, Error> {
    let bytes = std::fs::read(path).map_err(|source| Error::Read {
        path: path.display().to_string(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| Error::Json {
        path: path.display().to_string(),
        source,
    })
}

pub fn validate(manifest: &Manifest, corpus: &Corpus) -> Result<(), Error> {
    if manifest.schema_version != corpus.schema_version {
        return Err(Error::Invalid(
            "manifest and corpus schema versions differ".into(),
        ));
    }
    if manifest.model_policy.spend_authorized_by_manifest {
        return Err(Error::Invalid(
            "the manifest must not authorize spend".into(),
        ));
    }
    let development = corpus
        .cases
        .iter()
        .filter(|case| case.partition == Partition::Development)
        .count();
    let held_out = corpus
        .cases
        .iter()
        .filter(|case| case.partition == Partition::HeldOut)
        .count();
    if development != manifest.suite.development_chronologies
        || held_out != manifest.suite.held_out_chronologies
    {
        return Err(Error::Invalid(format!(
            "expected {} development and {} held-out cases, found {development} and {held_out}",
            manifest.suite.development_chronologies, manifest.suite.held_out_chronologies
        )));
    }
    let mut ids = BTreeSet::new();
    for case in &corpus.cases {
        if !ids.insert(&case.id) {
            return Err(Error::Invalid(format!("duplicate case id {}", case.id)));
        }
        if case.evidence.len() > manifest.suite.max_evidence_updates_per_chronology {
            return Err(Error::Invalid(format!(
                "{} has too many evidence updates",
                case.id
            )));
        }
        let forms: BTreeSet<_> = case.evidence.iter().map(|e| &e.input_form).collect();
        if forms.len() < manifest.suite.minimum_input_forms_per_chronology {
            return Err(Error::Invalid(format!(
                "{} has too few input forms",
                case.id
            )));
        }
        let roles: BTreeSet<_> = case.probes.iter().map(|p| p.role).collect();
        if roles != BTreeSet::from([ProbeRole::Help, ProbeRole::Revision, ProbeRole::Restraint]) {
            return Err(Error::Invalid(format!(
                "{} must have one probe of each role",
                case.id
            )));
        }
        if case
            .probes
            .iter()
            .any(|p| p.after_evidence > case.evidence.len())
        {
            return Err(Error::Invalid(format!(
                "{} has a probe beyond its evidence",
                case.id
            )));
        }
        let source_refs: BTreeSet<_> = case
            .evidence
            .iter()
            .map(|item| item.source_ref.as_str())
            .collect();
        let mut expected_ids = BTreeSet::new();
        for expected in &case.expected_interventions {
            if !expected_ids.insert(expected.id.as_str()) {
                return Err(Error::Invalid(format!(
                    "{} has duplicate expected intervention id {}",
                    case.id, expected.id
                )));
            }
            if expected.after_evidence > case.evidence.len() {
                return Err(Error::Invalid(format!(
                    "{} has expected intervention {} beyond its evidence",
                    case.id, expected.id
                )));
            }
            let not_before = parse_time(&expected.not_before).map_err(|reason| {
                Error::Invalid(format!(
                    "{} expected intervention {} has invalid not_before: {reason}",
                    case.id, expected.id
                ))
            })?;
            let not_after = parse_time(&expected.not_after).map_err(|reason| {
                Error::Invalid(format!(
                    "{} expected intervention {} has invalid not_after: {reason}",
                    case.id, expected.id
                ))
            })?;
            if not_before > not_after {
                return Err(Error::Invalid(format!(
                    "{} expected intervention {} starts after it ends",
                    case.id, expected.id
                )));
            }
            if let Some(missing) = expected
                .required_support
                .iter()
                .find(|source_ref| !source_refs.contains(source_ref.as_str()))
            {
                return Err(Error::Invalid(format!(
                    "{} expected intervention {} names unknown support {}",
                    case.id, expected.id, missing
                )));
            }
        }
    }
    Ok(())
}

fn parse_time(value: &str) -> Result<i64, chrono::ParseError> {
    chrono::DateTime::parse_from_rfc3339(value).map(|time| time.timestamp_millis())
}

/// Compare delivered interventions with the expected windows of one chronology.
/// Expected intervention times must be valid RFC 3339 timestamps.
pub fn account_interventions(
    case: &Chronology,
    delivered: &[DeliveredIntervention],
) -> InterventionAccounting {
    let mut accounting = InterventionAccounting {
        expected: case.expected_interventions.len(),
        delivered: delivered.len(),
        ..InterventionAccounting::default()
    };
    let mut available: BTreeSet<usize> = (0..delivered.len()).collect();
    let mut matched_expected = BTreeSet::new();
    let windows: Vec<_> = case
        .expected_interventions
        .iter()
        .map(|expected| {
            (
                expected,
                parse_time(&expected.not_before).expect("validated expected intervention time"),
                parse_time(&expected.not_after).expect("validated expected intervention time"),
            )
        })
        .collect();

    for (expected_index, (expected, not_before, not_after)) in windows.iter().enumerate() {
        if let Some(index) = available.iter().copied().find(|index| {
            let delivery = &delivered[*index];
            delivery.after_evidence >= expected.after_evidence
                && delivery.at >= *not_before
                && delivery.at <= *not_after
        }) {
            available.remove(&index);
            matched_expected.insert(expected_index);
            accounting.matched += 1;
        }
    }
    for (expected_index, (expected, not_before, not_after)) in windows.into_iter().enumerate() {
        if matched_expected.contains(&expected_index) || available.is_empty() {
            continue;
        }
        let Some(index) = available.iter().copied().next() else {
            break;
        };
        let delivery = &delivered[index];
        available.remove(&index);
        if delivery.at > not_after {
            accounting.late += 1;
        } else if delivery.at < not_before || delivery.after_evidence < expected.after_evidence {
            accounting.early += 1;
        } else {
            accounting.unexpected += 1;
        }
    }
    accounting.unexpected += available.len();
    accounting
}

pub async fn run_suite(
    manifest: &Manifest,
    corpus: &Corpus,
    corpus_sha256: String,
    authorization: Option<&Authorization>,
    driver: &mut dyn ChronologyDriver,
) -> Result<Report, Error> {
    validate(manifest, corpus)?;
    let route = driver.model_route().to_owned();
    if driver
        .routes_under_test()
        .contains(&manifest.grading.pre_grader_route)
    {
        return Err(Error::Invalid(
            "the pre-grader route must differ from the route under test".into(),
        ));
    }
    driver.set_pre_grader_route(&manifest.grading.pre_grader_route);
    let mut missing = Vec::new();
    let affordable = authorization.is_some_and(|a| {
        a.max_usd >= manifest.suite.max_total_usd && a.priced_routes.contains(&route)
    });
    if !affordable {
        missing.push(if authorization.is_none() {
            "evaluation spend authorization".into()
        } else {
            format!("priced and authorized model route: {route}")
        });
    }
    let mut runs = Vec::new();
    let mut usage = Usage::default();
    let mut system_failures = SystemFailures::default();
    let mut repeat_order = Vec::new();
    if affordable {
        for repeat in 1..=manifest.suite.repeats_per_chronology {
            for case in &corpus.cases {
                repeat_order.push(format!("{}:{repeat}", case.id));
                let result = driver.run(case, repeat).await;
                let limits = &manifest.suite;
                let mut over = Vec::new();
                if result.usage.model_calls > limits.max_model_calls_per_chronology_repeat {
                    over.push(format!(
                        "model_calls {} > {}",
                        result.usage.model_calls, limits.max_model_calls_per_chronology_repeat
                    ));
                }
                if result.usage.input_tokens > limits.max_input_tokens_per_chronology_repeat {
                    over.push(format!(
                        "input_tokens {} > {}",
                        result.usage.input_tokens, limits.max_input_tokens_per_chronology_repeat
                    ));
                }
                if result.usage.output_tokens > limits.max_output_tokens_per_chronology_repeat {
                    over.push(format!(
                        "output_tokens {} > {}",
                        result.usage.output_tokens, limits.max_output_tokens_per_chronology_repeat
                    ));
                }
                let max_wall_millis = limits.max_wall_seconds_per_chronology_repeat * 1000;
                if result.usage.elapsed_millis > max_wall_millis {
                    over.push(format!(
                        "elapsed_millis {} > {}",
                        result.usage.elapsed_millis, max_wall_millis
                    ));
                }
                if result.usage.usd > limits.max_usd_per_chronology_repeat {
                    over.push(format!(
                        "usd {} > {}",
                        result.usage.usd, limits.max_usd_per_chronology_repeat
                    ));
                }
                let status = if over.is_empty() {
                    result.status
                } else {
                    RunStatus::Failed
                };
                usage += result.usage;
                system_failures.add(&result.system_failures);
                let intervention_accounting =
                    account_interventions(case, &result.delivered_interventions);
                runs.push(RunReport {
                    case_id: case.id.clone(),
                    partition: case.partition,
                    repeat,
                    status,
                    over,
                    missing_capabilities: result.missing_capabilities,
                    observations: result.observations,
                    delivered_interventions: result.delivered_interventions,
                    expected_intervention_windows: case
                        .expected_interventions
                        .iter()
                        .map(|expected| ExpectedInterventionWindow {
                            id: expected.id.clone(),
                            after_evidence: expected.after_evidence,
                            not_before: expected.not_before.clone(),
                            not_after: expected.not_after.clone(),
                        })
                        .collect(),
                    intervention_accounting,
                    intervention_pipeline: result.intervention_pipeline,
                    system_failures: result.system_failures,
                    usage: result.usage,
                });
            }
        }
    }
    // Intervention precision is a pooled held-out gate. A development
    // run with nothing to score contributes no denominator and no
    // system failure. A held-out pool with no denominator is unscored
    // once at the suite level.
    let (held_out_runs, held_out_denominator) = runs
        .iter()
        .filter(|run| run.partition == Partition::HeldOut)
        .fold((0_usize, 0_usize), |(count, denominator), run| {
            (
                count + 1,
                denominator + run.intervention_accounting.denominator(),
            )
        });
    if held_out_runs > 0 && held_out_denominator == 0 {
        system_failures
            .unscored_metrics
            .push(UNSCORED_INTERVENTION_PRECISION.to_string());
    }
    let total_over = usage.usd > manifest.suite.max_total_usd
        || usage.elapsed_millis > manifest.suite.max_total_wall_seconds * 1000;
    let probes_without_proposal = runs
        .iter()
        .flat_map(|run| &run.observations)
        .filter(|observation| observation.proposed_grade.is_none())
        .count();
    let status = if total_over || runs.iter().any(|r| r.status == RunStatus::Failed) {
        RunStatus::Failed
    } else if !missing.is_empty() || runs.iter().any(|r| r.status == RunStatus::Unscored) {
        RunStatus::Unscored
    } else {
        RunStatus::Complete
    };
    Ok(Report {
        schema_version: 1,
        status,
        manifest_version: manifest.schema_version,
        profile: manifest.profile.clone(),
        release_thresholds: manifest.release_thresholds.clone(),
        pre_grader_route: manifest.grading.pre_grader_route.clone(),
        corpus_version: corpus.version.clone(),
        corpus_sha256,
        driver: driver.name().to_owned(),
        store: driver.store(),
        model_route: route,
        clock_version: driver.clock_version().into(),
        zone_rule_version: driver.zone_rule_version().into(),
        run_context: driver.run_context(),
        repeat_order,
        probes_without_proposal,
        missing_capabilities: missing,
        system_failures,
        usage,
        runs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Corpus {
        let path = Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"))
            .join("suites/continuous-learning/corpus.json");
        load_corpus(&path).unwrap().0
    }

    fn expected_case() -> Chronology {
        corpus()
            .cases
            .into_iter()
            .find(|case| !case.expected_interventions.is_empty())
            .unwrap()
    }

    #[test]
    fn intervention_accounting_matches_only_a_visible_in_window_delivery() {
        let case = expected_case();
        let expected = &case.expected_interventions[0];
        let delivered = DeliveredIntervention {
            after_evidence: expected.after_evidence,
            at: parse_time(&expected.not_before).unwrap(),
            text: "Timely help".into(),
        };

        let accounting = account_interventions(&case, &[delivered]);

        assert_eq!(
            accounting,
            InterventionAccounting {
                expected: 1,
                delivered: 1,
                matched: 1,
                early: 0,
                late: 0,
                unexpected: 0,
            }
        );
        assert_eq!(accounting.denominator(), 1);
    }

    #[test]
    fn intervention_accounting_counts_early_late_and_unexpected_deliveries() {
        let case = expected_case();
        let expected = &case.expected_interventions[0];
        let before = parse_time(&expected.not_before).unwrap();
        let after = parse_time(&expected.not_after).unwrap();

        let early = account_interventions(
            &case,
            &[DeliveredIntervention {
                after_evidence: expected.after_evidence,
                at: before - 1,
                text: "Early help".into(),
            }],
        );
        let before_evidence = account_interventions(
            &case,
            &[DeliveredIntervention {
                after_evidence: expected.after_evidence - 1,
                at: before,
                text: "Unsupported early help".into(),
            }],
        );
        let late = account_interventions(
            &case,
            &[DeliveredIntervention {
                after_evidence: expected.after_evidence,
                at: after + 1,
                text: "Late help".into(),
            }],
        );
        let development = corpus()
            .cases
            .into_iter()
            .find(|case| case.expected_interventions.is_empty())
            .unwrap();
        let unexpected = account_interventions(
            &development,
            &[DeliveredIntervention {
                after_evidence: 0,
                at: 0,
                text: "Unexpected help".into(),
            }],
        );

        assert_eq!(early.early, 1);
        assert_eq!(early.denominator(), 2);
        assert_eq!(before_evidence.early, 1);
        assert_eq!(before_evidence.denominator(), 2);
        assert_eq!(late.late, 1);
        assert_eq!(late.denominator(), 2);
        assert_eq!(unexpected.unexpected, 1);
        assert_eq!(unexpected.denominator(), 1);
    }
}
