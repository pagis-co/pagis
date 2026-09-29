use async_trait::async_trait;
use pagis_evaluation::*;
use std::{collections::BTreeSet, path::PathBuf};

fn crate_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
}
fn manifest_path() -> PathBuf {
    crate_dir().join("suites/continuous-learning/release.json")
}
fn suite() -> (Manifest, Corpus, String) {
    load_suite(&manifest_path()).unwrap()
}

#[test]
fn the_corpus_obeys_the_release_manifest() {
    let (manifest, corpus, hash) = load_suite(&manifest_path()).expect("release suite");

    assert_eq!(manifest.profile, "continuous-learning");
    assert_eq!(manifest.corpus.file, "corpus.json");
    assert_eq!(manifest.suite.development_chronologies, 13);
    assert_eq!(manifest.suite.held_out_chronologies, 24);
    assert_eq!(manifest.suite.repeats_per_chronology, 2);
    assert_eq!(manifest.suite.max_model_calls_per_chronology_repeat, 96);
    assert_eq!(
        manifest.suite.max_input_tokens_per_chronology_repeat,
        512_000
    );
    assert_eq!(
        manifest.suite.max_output_tokens_per_chronology_repeat,
        48_000
    );
    assert_eq!(manifest.suite.max_wall_seconds_per_chronology_repeat, 600);
    assert_eq!(manifest.scoring.fraction_gate, "wilson-90-lower-bound");
    assert!(
        manifest
            .scoring
            .zero_tolerance_gates
            .iter()
            .any(|gate| gate == "zero-system-failures")
    );
    assert_eq!(manifest.grading.fixed_probe_sample_per_pooled_arm, 12);
    assert_eq!(
        manifest.required_metrics.intervention_precision.denominator,
        "delivered-interventions-plus-corpus-expected-interventions"
    );

    validate(&manifest, &corpus).expect("the corpus satisfies the manifest");
    assert_eq!(corpus.cases.len(), 37);
    assert_eq!(
        corpus
            .cases
            .iter()
            .filter(|case| case.partition == Partition::Development)
            .count(),
        13
    );
    assert_eq!(
        corpus
            .cases
            .iter()
            .filter(|case| case.partition == Partition::HeldOut)
            .count(),
        24
    );
    assert!(corpus.cases.iter().all(|case| case.probes.len() == 3));
    assert_eq!(hash.len(), 64);
    assert!(hash.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(
        hash,
        "2e10565d39bc3ed954e5f6ce9636b6e969b396ad74544d4fcc1dbfb8bd5aa4bc"
    );
}

#[test]
fn validation_rejects_an_expected_intervention_beyond_the_evidence() {
    let (manifest, mut corpus, _) = suite();
    let case = corpus
        .cases
        .iter_mut()
        .find(|case| !case.expected_interventions.is_empty())
        .unwrap();
    case.expected_interventions[0].after_evidence = case.evidence.len() + 1;

    let error = validate(&manifest, &corpus).expect_err("invalid evidence position");

    assert!(error.to_string().contains("beyond its evidence"), "{error}");
}

#[test]
fn validation_rejects_duplicate_expected_intervention_ids() {
    let (manifest, mut corpus, _) = suite();
    let case = corpus
        .cases
        .iter_mut()
        .find(|case| !case.expected_interventions.is_empty())
        .unwrap();
    let duplicate = case.expected_interventions[0].clone();
    case.expected_interventions.push(duplicate);

    let error = validate(&manifest, &corpus).expect_err("duplicate expected id");

    assert!(
        error
            .to_string()
            .contains("duplicate expected intervention id"),
        "{error}"
    );
}

#[test]
fn expected_interventions_are_a_required_corpus_field() {
    let path = crate_dir().join("tests/fixtures/corpus-missing-expected-interventions.json");

    let error = load_corpus(&path).expect_err("the corpus has no expected interventions");

    assert!(
        error.to_string().contains("expected_interventions"),
        "{error}"
    );
}

#[test]
fn validation_rejects_an_inverted_expected_intervention_window() {
    let (manifest, mut corpus, _) = suite();
    let expected = &mut corpus
        .cases
        .iter_mut()
        .find(|case| !case.expected_interventions.is_empty())
        .unwrap()
        .expected_interventions[0];
    std::mem::swap(&mut expected.not_before, &mut expected.not_after);

    let error = validate(&manifest, &corpus).expect_err("inverted intervention window");

    assert!(
        error.to_string().contains("starts after it ends"),
        "{error}"
    );
}

#[test]
fn validation_rejects_an_invalid_expected_intervention_time() {
    let (manifest, mut corpus, _) = suite();
    let expected = &mut corpus
        .cases
        .iter_mut()
        .find(|case| !case.expected_interventions.is_empty())
        .unwrap()
        .expected_interventions[0];
    expected.not_before = "not-a-time".into();

    let error = validate(&manifest, &corpus).expect_err("invalid intervention time");

    assert!(error.to_string().contains("invalid not_before"), "{error}");
}

#[test]
fn validation_rejects_unknown_expected_intervention_support() {
    let (manifest, mut corpus, _) = suite();
    let expected = &mut corpus
        .cases
        .iter_mut()
        .find(|case| !case.expected_interventions.is_empty())
        .unwrap()
        .expected_interventions[0];
    expected.required_support.push("fixture://unknown/1".into());

    let error = validate(&manifest, &corpus).expect_err("unknown required support");

    assert!(
        error.to_string().contains("names unknown support"),
        "{error}"
    );
}

struct Driver {
    calls: usize,
    usage: Usage,
    status: RunStatus,
}
#[async_trait]
impl ChronologyDriver for Driver {
    fn name(&self) -> &str {
        "daemon"
    }
    fn store(&self) -> StoreObservability {
        StoreObservability::SubjectPages
    }
    async fn run(&mut self, case: &Chronology, _: u8) -> DriverResult {
        self.calls += 1;
        DriverResult {
            status: self.status.clone(),
            missing_capabilities: if self.status == RunStatus::Unscored {
                vec!["contextual revision".into()]
            } else {
                vec![]
            },
            observations: case
                .probes
                .iter()
                .map(|probe| ProbeObservation {
                    probe_id: probe.id.clone(),
                    role: probe.role,
                    observed_output: None,
                    observed_effects: vec![],
                    failure: if self.status == RunStatus::Complete {
                        None
                    } else {
                        Some("capability unavailable".into())
                    },
                    proposed_grade: None,
                    proposed_reason: None,
                })
                .collect(),
            delivered_interventions: vec![],
            intervention_pipeline: pagis_evaluation::InterventionPipeline {
                schedules_created: 1,
                wake_ups_fired: 1,
                schedules: vec![pagis_evaluation::ScheduleObservation {
                    schedule_id: "schedule-1".into(),
                    next_due_at: Some(1_767_261_600_000),
                    fired: true,
                }],
                ..Default::default()
            },
            system_failures: SystemFailures::default(),
            usage: self.usage,
        }
    }
    fn model_route(&self) -> &str {
        "sage-test"
    }
    fn clock_version(&self) -> &str {
        "fixed-2026-01-01"
    }
    fn zone_rule_version(&self) -> &str {
        "chrono-tz-test"
    }
}

#[tokio::test]
async fn preflight_stops_before_an_unauthorized_call() {
    let (manifest, corpus, hash) = suite();
    let mut driver = Driver {
        calls: 0,
        usage: Usage::default(),
        status: RunStatus::Complete,
    };
    let report = run_suite(&manifest, &corpus, hash, None, &mut driver)
        .await
        .unwrap();
    assert_eq!(report.status, RunStatus::Unscored);
    assert_eq!(driver.calls, 0);
    assert!(report.runs.is_empty());
    assert_eq!(
        report.missing_capabilities,
        ["evaluation spend authorization"]
    );
}

#[tokio::test]
async fn the_route_under_test_cannot_grade_itself() {
    let (mut manifest, corpus, hash) = suite();
    manifest.grading.pre_grader_route = "sage-test".into();
    let mut driver = Driver {
        calls: 0,
        usage: Usage::default(),
        status: RunStatus::Complete,
    };

    let error = run_suite(&manifest, &corpus, hash, None, &mut driver)
        .await
        .expect_err("one route cannot test and grade itself");

    assert!(error.to_string().contains("pre-grader route"), "{error}");
    assert_eq!(driver.calls, 0);
}

#[tokio::test]
async fn reports_every_repeat_and_does_not_turn_unbuilt_work_into_a_pass() {
    let (manifest, corpus, hash) = suite();
    let authorization = Authorization {
        max_usd: manifest.suite.max_total_usd,
        priced_routes: BTreeSet::from(["sage-test".into()]),
    };
    let mut driver = Driver {
        calls: 0,
        usage: Usage::default(),
        status: RunStatus::Unscored,
    };
    let report = run_suite(&manifest, &corpus, hash, Some(&authorization), &mut driver)
        .await
        .unwrap();
    assert_eq!(driver.calls, 74);
    assert_eq!(report.runs.len(), 74);
    assert_eq!(
        report
            .runs
            .iter()
            .map(|run| run.observations.len())
            .sum::<usize>(),
        222
    );
    assert_eq!(report.probes_without_proposal, 222);
    let json = serde_json::to_value(&report).expect("serialize the report");
    assert_eq!(
        json["runs"][0]["intervention_pipeline"]["schedules_created"],
        1
    );
    assert_eq!(
        json["runs"][0]["intervention_pipeline"]["wake_ups_fired"],
        1
    );
    assert_eq!(
        json["runs"][0]["intervention_pipeline"]["decisions"],
        serde_json::json!({"sent": 0, "rescheduled": 0, "silent": 0})
    );
    assert_eq!(report.status, RunStatus::Unscored);
}

#[tokio::test]
async fn per_repeat_report_lists_schedule_due_times_against_expected_windows() {
    let (manifest, corpus, hash) = suite();
    let authorization = Authorization {
        max_usd: manifest.suite.max_total_usd,
        priced_routes: BTreeSet::from(["sage-test".into()]),
    };
    let mut driver = Driver {
        calls: 0,
        usage: Usage::default(),
        status: RunStatus::Complete,
    };
    let report = run_suite(&manifest, &corpus, hash, Some(&authorization), &mut driver)
        .await
        .unwrap();
    let json = serde_json::to_value(&report).expect("serialize the report");
    let run_with_window = json["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| {
            run["expected_intervention_windows"]
                .as_array()
                .is_some_and(|windows| !windows.is_empty())
        })
        .expect("repeat with an expected intervention window");
    assert_eq!(
        run_with_window["intervention_pipeline"]["schedules"][0],
        serde_json::json!({
            "schedule_id": "schedule-1",
            "next_due_at": 1_767_261_600_000_i64,
            "fired": true
        })
    );
    assert!(
        run_with_window["expected_intervention_windows"][0]["not_before"]
            .as_str()
            .is_some()
    );
    assert!(
        run_with_window["expected_intervention_windows"][0]["not_after"]
            .as_str()
            .is_some()
    );
    assert!(
        run_with_window["expected_intervention_windows"][0]["after_evidence"]
            .as_u64()
            .is_some()
    );
}

#[tokio::test]
async fn a_repeat_over_one_cap_reports_its_name_and_values() {
    let (mut manifest, corpus, hash) = suite();
    manifest.suite.max_output_tokens_per_chronology_repeat = 16_000;
    let authorization = Authorization {
        max_usd: manifest.suite.max_total_usd,
        priced_routes: BTreeSet::from(["sage-test".into()]),
    };
    let mut driver = Driver {
        calls: 0,
        usage: Usage {
            output_tokens: 18_684,
            ..Usage::default()
        },
        // An overrun is Failed even when the underlying result is
        // otherwise unscored.
        status: RunStatus::Unscored,
    };
    let report = run_suite(&manifest, &corpus, hash, Some(&authorization), &mut driver)
        .await
        .unwrap();
    assert_eq!(report.status, RunStatus::Failed);
    assert!(
        report
            .runs
            .iter()
            .all(|run| run.status == RunStatus::Failed)
    );
    assert!(
        report
            .runs
            .iter()
            .all(|run| run.over == ["output_tokens 18684 > 16000".to_string()])
    );
}

/// A development-only suite needs no intervention denominator.
#[tokio::test]
async fn a_development_only_suite_has_no_required_intervention_denominator() {
    let (mut manifest, mut corpus, hash) = suite();
    corpus.cases.retain(|case| {
        case.partition == Partition::Development && case.expected_interventions.is_empty()
    });
    corpus.cases.truncate(1);
    manifest.suite.development_chronologies = 1;
    manifest.suite.held_out_chronologies = 0;
    let authorization = Authorization {
        max_usd: manifest.suite.max_total_usd,
        priced_routes: BTreeSet::from(["sage-test".to_string()]),
    };
    let mut driver = Driver {
        calls: 0,
        usage: Usage::default(),
        status: RunStatus::Complete,
    };

    let report = run_suite(&manifest, &corpus, hash, Some(&authorization), &mut driver)
        .await
        .unwrap();

    assert_eq!(report.status, RunStatus::Complete);
    assert!(report.system_failures.unscored_metrics.is_empty());
}

#[tokio::test]
async fn a_held_out_suite_with_no_intervention_denominator_is_unscored() {
    let (mut manifest, mut corpus, hash) = suite();
    corpus
        .cases
        .retain(|case| case.partition == Partition::HeldOut);
    corpus.cases.truncate(1);
    corpus.cases[0].expected_interventions.clear();
    manifest.suite.development_chronologies = 0;
    manifest.suite.held_out_chronologies = 1;
    let authorization = Authorization {
        max_usd: manifest.suite.max_total_usd,
        priced_routes: BTreeSet::from(["sage-test".to_string()]),
    };
    let mut driver = Driver {
        calls: 0,
        usage: Usage::default(),
        status: RunStatus::Complete,
    };

    let report = run_suite(&manifest, &corpus, hash, Some(&authorization), &mut driver)
        .await
        .unwrap();

    assert_eq!(
        report.system_failures.unscored_metrics,
        [UNSCORED_INTERVENTION_PRECISION]
    );
}
