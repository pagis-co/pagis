//! The human grading sheet of one run.

use pagis_evaluation::grading::{RunContext, grading_sheet, write_run};
use pagis_evaluation::{
    Corpus, DeliveredIntervention, Partition, ProbeObservation, ProbeRole, ReleaseThresholds,
    Report, RunReport, RunStatus, StoreObservability, SystemFailures, Usage, load_corpus,
};
use std::path::PathBuf;

const CASE: &str = "couch-offer";

fn corpus() -> Corpus {
    let (corpus, _) = load_corpus(
        &PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets the manifest dir"),
        )
        .join("suites/continuous-learning/corpus.json"),
    )
    .expect("load the corpus");
    corpus
}

fn report() -> Report {
    Report {
        schema_version: 1,
        status: RunStatus::Complete,
        manifest_version: 1,
        profile: "continuous-learning-v1".into(),
        release_thresholds: ReleaseThresholds {
            deterministic_invariant_pass_fraction: 1.0,
            material_output_grounded_fraction: 1.0,
            unsupported_persisted_or_delivered_associations: 0,
            final_supported_connection_recall_min: 0.85,
            useful_help_selection_fraction_min: 0.8,
            delivered_intervention_precision_min: 0.8,
            restraint_probe_success_fraction: 1.0,
            revision_probe_success_fraction: 1.0,
            duplicate_visible_interventions: 0,
            future_evidence_leaks: 0,
            unauthorized_effects_or_disclosures: 0,
            unscored_required_runs_for_release: 0,
            resource_bound_compliance_fraction: 1.0,
        },
        pre_grader_route: "openai/gpt-5-mini".into(),
        corpus_version: "continuous-learning-corpus-v1".into(),
        corpus_sha256: "abc123".into(),
        driver: "daemon".into(),
        store: StoreObservability::SubjectPages,
        model_route: "default".into(),
        clock_version: "fixture-source-clock-v1+system-wall-clock".into(),
        zone_rule_version: "chrono-tz 2026a".into(),
        run_context: vec![],
        repeat_order: vec![format!("{CASE}:1")],
        probes_without_proposal: 0,
        missing_capabilities: Vec::new(),
        system_failures: SystemFailures::default(),
        usage: Usage {
            model_calls: 6,
            input_tokens: 100,
            output_tokens: 20,
            source_reads: 3,
            elapsed_millis: 4_000,
            usd: 0.25,
        },
        runs: vec![RunReport {
            case_id: CASE.into(),
            partition: Partition::Development,
            repeat: 1,
            status: RunStatus::Complete,
            over: Vec::new(),
            missing_capabilities: Vec::new(),
            observations: vec![ProbeObservation {
                probe_id: "help".into(),
                role: ProbeRole::Help,
                observed_output: Some("The shop offer stands. Shall I compare it?".into()),
                observed_effects: vec![
                    "tool:memory_read".into(),
                    "result:memory_read bytes=20".into(),
                ],
                failure: None,
                proposed_grade: Some("pass".into()),
                proposed_reason: Some("The answer uses the supported shop offer.".into()),
            }],
            delivered_interventions: vec![DeliveredIntervention {
                after_evidence: 2,
                at: 1_767_258_000_000,
                text: "Your AB123 policy renews tomorrow.".into(),
            }],
            expected_intervention_windows: Vec::new(),
            intervention_accounting: pagis_evaluation::InterventionAccounting {
                expected: 0,
                delivered: 1,
                matched: 0,
                early: 0,
                late: 0,
                unexpected: 1,
            },
            intervention_pipeline: pagis_evaluation::InterventionPipeline {
                schedules_created: 2,
                wake_ups_fired: 1,
                schedules: Vec::new(),
                decisions: pagis_evaluation::InterventionDecisions {
                    sent: 1,
                    rescheduled: 0,
                    silent: 0,
                },
            },
            system_failures: SystemFailures::default(),
            usage: Usage {
                model_calls: 6,
                input_tokens: 100,
                output_tokens: 20,
                source_reads: 3,
                elapsed_millis: 4_000,
                usd: 0.25,
            },
        }],
    }
}

#[test]
fn the_sheet_counts_probes_without_a_proposal() {
    let mut report = report();
    report.probes_without_proposal = 1;
    report.runs[0].observations[0].proposed_grade = None;
    report.runs[0].observations[0].proposed_reason =
        Some("invalid pre-grade: expected value at line 1 column 1".into());

    let sheet = grading_sheet(&report, &corpus(), &context(true));

    assert!(
        sheet.contains("| Probes without a proposal | 1 |"),
        "{sheet}"
    );
    assert!(sheet.contains("| missing | invalid pre-grade:"), "{sheet}");
}

fn context(gradable: bool) -> RunContext {
    RunContext {
        title: "configured model".into(),
        gradable,
        code_commit: "deadbeef".into(),
        model_alias: "default".into(),
        routes: vec!["anthropic/claude-sonnet-4-6 at $3.00/$15.00 per Mtok".into()],
        authorized_max_usd: 25.0,
    }
}

#[test]
fn a_gradable_sheet_carries_the_fixture_rubric_and_an_empty_grade() {
    let sheet = grading_sheet(&report(), &corpus(), &context(true));

    assert!(sheet.contains("| Corpus sha256 | abc123 |"), "{sheet}");
    assert!(sheet.contains("| Code commit | deadbeef |"), "{sheet}");
    assert!(sheet.contains("anthropic/claude-sonnet-4-6 at $3.00/$15.00 per Mtok"));
    assert!(sheet.contains("| Clock version | fixture-source-clock-v1+system-wall-clock |"));
    assert!(
        sheet.contains("What useful help is warranted now?"),
        "{sheet}"
    );
    assert!(
        sheet.contains("- Permissible conclusions: Mention the offer"),
        "{sheet}"
    );
    assert!(
        sheet.contains("- Required connections: A shop offered a discount"),
        "{sheet}"
    );
    assert!(
        sheet.contains("- Prohibited assertions and effects: Claim an external action occurred."),
        "{sheet}"
    );
    assert!(sheet.contains("The shop offer stands. Shall I compare it?"));
    // The sheet prints what each tool returned next to the call, so a
    // human grader reads what the run knew.
    assert!(
        sheet.contains("Observed effects: tool:memory_read / result:memory_read bytes=20"),
        "{sheet}"
    );
    assert!(
        sheet.contains(
            "| Proposed grade | Proposed reason | Owner grade | Owner reason |\n| --- | --- | --- | --- |\n| pass | The answer uses the supported shop offer. |  |  |"
        ),
        "{sheet}"
    );
    assert!(
        sheet.contains("| Connection recall | at least 85% |"),
        "{sheet}"
    );
    assert!(
        sheet.contains("| Metric | Threshold | Point estimate | Wilson 90% lower bound | Result |"),
        "{sheet}"
    );
    // The delivered help is unexpected in this development case, so it
    // is the full intervention precision denominator.
    assert!(
        sheet.contains(
            "| Intervention precision | at least 80% of 1 interventions (1 delivered plus 0 missed expected) |"
        ),
        "{sheet}"
    );
    assert!(sheet.contains("| Delivered interventions | 1 |"), "{sheet}");
    assert!(
        sheet.contains("Your AB123 policy renews tomorrow."),
        "{sheet}"
    );
    assert!(
        sheet.contains("Expected 0, delivered 1, matched 0, early 0, late 0, unexpected 1."),
        "{sheet}"
    );
    assert!(
        sheet.contains(
            "Schedules created 2, wake-ups fired 1, decisions sent 1, rescheduled 0, silent 0."
        ),
        "{sheet}"
    );
    assert!(
        sheet.contains("- Grade (useful and timely, or not):"),
        "{sheet}"
    );
}

/// A run with no delivery leaves the precision denominator empty, and
/// the tally says the metric cannot be scored.
#[test]
fn a_sheet_without_a_delivery_names_the_empty_denominator() {
    let mut report = report();
    report.runs[0].delivered_interventions.clear();
    report.runs[0].intervention_accounting = pagis_evaluation::InterventionAccounting::default();

    let sheet = grading_sheet(&report, &corpus(), &context(true));

    assert!(
        sheet.contains("| Intervention precision | at least 80% of 0 interventions; unscored |"),
        "{sheet}"
    );
    assert!(
        sheet.contains("_the daemon delivered no intervention in this run_"),
        "{sheet}"
    );
}

#[test]
fn an_expected_intervention_makes_no_delivery_a_scored_zero() {
    let corpus = corpus();
    let case = corpus
        .cases
        .iter()
        .find(|case| !case.expected_interventions.is_empty())
        .unwrap();
    let mut report = report();
    report.runs[0].case_id = case.id.clone();
    report.runs[0].partition = case.partition;
    report.runs[0].delivered_interventions.clear();
    report.runs[0].intervention_accounting = pagis_evaluation::InterventionAccounting {
        expected: 1,
        delivered: 0,
        matched: 0,
        early: 0,
        late: 0,
        unexpected: 0,
    };

    let sheet = grading_sheet(&report, &corpus, &context(true));

    assert!(
        sheet.contains(
            "| Intervention precision | at least 80% of 1 interventions (0 delivered plus 1 missed expected) |"
        ),
        "{sheet}"
    );
    assert!(
        sheet.contains("Expected 1, delivered 0, matched 0, early 0, late 0, unexpected 0."),
        "{sheet}"
    );
    assert!(
        sheet.contains(&case.expected_interventions[0].purpose),
        "{sheet}"
    );
}

#[test]
fn a_scripted_run_is_marked_unscored_and_offers_no_grade() {
    let sheet = grading_sheet(&report(), &corpus(), &context(false));

    assert!(sheet.contains("unscored by definition"), "{sheet}");
    assert!(!sheet.contains("Grade (pass or fail)"), "{sheet}");
    assert!(!sheet.contains("Metric tally"), "{sheet}");
}

#[test]
fn a_run_writes_the_report_and_the_sheet_together() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let target = directory.path().join("20260907T120000Z");

    let written = write_run(&target, &report(), &corpus(), &context(true)).expect("write the run");

    assert_eq!(written, target);
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(target.join("report.json")).expect("report"))
            .expect("valid JSON");
    assert_eq!(json["status"], "complete");
    assert_eq!(json["runs"][0]["case_id"], CASE);
    assert!(
        std::fs::read_to_string(target.join("GRADING.md"))
            .expect("sheet")
            .starts_with("# Continuous learning release run: configured model")
    );
}

/// The brittleness gate, reported for the suite and for each
/// chronology repeat.
#[test]
fn the_sheet_carries_the_zero_system_failure_line() {
    let sheet = grading_sheet(&report(), &corpus(), &context(true));

    assert!(sheet.contains("| Driver | daemon |"), "{sheet}");
    assert!(
        sheet.contains(
            "| System failures | pass: no quarantine, no hang, no aborted turn and no unscored \
             required metric |"
        ),
        "{sheet}"
    );
    assert!(
        sheet.contains(
            "Zero system failures: pass: no quarantine, no hang, no aborted turn and no unscored \
             required metric"
        ),
        "{sheet}"
    );
    assert!(
        sheet.contains(
            "| Intervention precision | at least 80% of 1 interventions (1 delivered plus 0 missed expected) |"
        ),
        "{sheet}"
    );
}

/// A failed gate names each count, so the owner reads what broke.
#[test]
fn a_failed_system_failure_gate_names_each_count() {
    let mut report = report();
    let failures = SystemFailures {
        quarantines: 1,
        hangs: 2,
        aborted_turns: vec![
            "delivery 1: a".into(),
            "probe `help`: b".into(),
            "probe `x`: c".into(),
        ],
        unscored_metrics: vec!["intervention precision".into()],
    };
    report.runs[0].system_failures = failures.clone();
    report.system_failures = failures;

    let sheet = grading_sheet(&report, &corpus(), &context(true));

    assert!(
        sheet.contains(
            "fail: 1 quarantine(s), 2 hang(s), 3 aborted turn(s), unscored required metrics \
             intervention precision"
        ),
        "{sheet}"
    );
}

/// Every aborted turn is named under the gate line of its run, with the
/// delivery or the probe it happened in and the reason. The
/// first four baseline runs counted the aborted turns and said nothing
/// about which turn was lost or why.
#[test]
fn the_sheet_names_every_aborted_turn_with_its_reason() {
    let mut report = report();
    report.runs[0].system_failures = SystemFailures {
        aborted_turns: vec![
            "delivery 2: the reply run failed: bad capture png".into(),
            "probe `help`: the daemon refused the message with 422".into(),
        ],
        ..SystemFailures::default()
    };

    let sheet = grading_sheet(&report, &corpus(), &context(true));

    assert!(
        sheet.contains("Aborted turn: delivery 2: the reply run failed: bad capture png"),
        "{sheet}"
    );
    assert!(
        sheet.contains("Aborted turn: probe `help`: the daemon refused the message with 422"),
        "{sheet}"
    );
}

#[test]
fn a_repeat_over_a_cap_names_the_cap_and_values() {
    let mut report = report();
    report.runs[0].status = RunStatus::Failed;
    report.runs[0].over = vec!["output_tokens 18684 > 16000".into()];

    let sheet = grading_sheet(&report, &corpus(), &context(true));

    assert!(
        sheet.contains("over: output_tokens 18684 > 16000"),
        "{sheet}"
    );
}
