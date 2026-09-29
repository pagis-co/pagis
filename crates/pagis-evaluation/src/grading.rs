//! The human grading sheet of one suite run.
//!
//! The runner records what the daemon showed and did. A different
//! model route proposes a grade and reason for each probe. This module
//! writes that proposal beside the frozen fixture rubric and leaves the
//! owner's grade empty. The sheet holds visible agent output only: no
//! raw private source body and no hidden model reasoning.

use std::io;
use std::path::{Path, PathBuf};

use crate::{Chronology, Corpus, Probe, Report, RunReport, StoreObservability, SystemFailures};

/// What the sheet says about the run beyond the runner's own report.
#[derive(Clone, Debug)]
pub struct RunContext {
    /// What kind of run this is, named in the sheet's first line.
    pub title: String,
    /// Whether a human can grade the run at all. A scripted model
    /// proves the path works and says nothing about behavior, so its
    /// run stays unscored however complete the runner calls it.
    pub gradable: bool,
    pub code_commit: String,
    pub model_alias: String,
    /// One line for each resolved route of the alias, with its price.
    pub routes: Vec<String>,
    /// The spend the caller authorized, which the manifest never does.
    pub authorized_max_usd: f64,
}

/// Write the runner's JSON report and the human grading sheet into
/// `directory`, which the caller names after the run. Returns the
/// directory.
pub fn write_run(
    directory: &Path,
    report: &Report,
    corpus: &Corpus,
    context: &RunContext,
) -> io::Result<PathBuf> {
    std::fs::create_dir_all(directory)?;
    std::fs::write(
        directory.join("report.json"),
        serde_json::to_vec_pretty(report).map_err(io::Error::other)?,
    )?;
    std::fs::write(
        directory.join("GRADING.md"),
        grading_sheet(report, corpus, context),
    )?;
    Ok(directory.to_path_buf())
}

/// The Markdown grading sheet of one run.
pub fn grading_sheet(report: &Report, corpus: &Corpus, context: &RunContext) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Continuous learning release run: {}\n\n",
        context.title
    ));
    if context.gradable {
        out.push_str(
            "Grading is the owner's work. Read each model proposal below, then fill the owner's \
             Grade and Reason. A reason must name the evidence a later reviewer can check.\n\n",
        );
    } else {
        out.push_str(
            "This run is unscored by definition: a scripted model produced the output. It \
             shows that the runner reaches the shipped path, and nothing about behavior. Do \
             not grade it.\n\n",
        );
    }
    out.push_str(&summary(report, context));
    out.push_str("\n## Repeat order\n\n");
    out.push_str(&format!("`{}`\n", report.repeat_order.join("`, `")));
    if !report.missing_capabilities.is_empty() {
        out.push_str("\n## Missing capabilities\n\n");
        for missing in &report.missing_capabilities {
            out.push_str(&format!("- {missing}\n"));
        }
    }
    for run in &report.runs {
        out.push_str(&self::run(run, corpus, context));
    }
    out.push_str(&tally(report, context));
    out
}

fn summary(report: &Report, context: &RunContext) -> String {
    let usage = &report.usage;
    let mut rows = vec![
        ("Status".to_string(), format!("{:?}", report.status)),
        ("Gradable".to_string(), yes_no(context.gradable)),
        ("Code commit".to_string(), context.code_commit.clone()),
        (
            "Manifest version".to_string(),
            report.manifest_version.to_string(),
        ),
        ("Profile".to_string(), report.profile.clone()),
        ("Corpus version".to_string(), report.corpus_version.clone()),
        ("Corpus sha256".to_string(), report.corpus_sha256.clone()),
        ("Driver".to_string(), report.driver.clone()),
        ("Memory store".to_string(), store_line(report.store)),
        (
            "Prompt version".to_string(),
            format!(
                "daemon prompts at code commit {}; probe prompts in the hashed corpus",
                context.code_commit
            ),
        ),
        ("Model alias".to_string(), context.model_alias.clone()),
        ("Resolved routes".to_string(), context.routes.join("; ")),
        ("Recorded route".to_string(), report.model_route.clone()),
        (
            "Pre-grader route".to_string(),
            report.pre_grader_route.clone(),
        ),
        ("Clock version".to_string(), report.clock_version.clone()),
        (
            "Zone rule version".to_string(),
            report.zone_rule_version.clone(),
        ),
        ("Chronology runs".to_string(), report.runs.len().to_string()),
        (
            "Probe results".to_string(),
            report
                .runs
                .iter()
                .map(|run| run.observations.len())
                .sum::<usize>()
                .to_string(),
        ),
        (
            "Probes without a proposal".to_string(),
            report.probes_without_proposal.to_string(),
        ),
        (
            "Delivered interventions".to_string(),
            delivered_count(report).to_string(),
        ),
        (
            "Expected interventions".to_string(),
            report
                .runs
                .iter()
                .map(|run| run.intervention_accounting.expected)
                .sum::<usize>()
                .to_string(),
        ),
        ("Run context".to_string(), report.run_context.join("; ")),
        (
            "System failures".to_string(),
            failure_line(&report.system_failures),
        ),
        ("Model calls".to_string(), usage.model_calls.to_string()),
        ("Input tokens".to_string(), usage.input_tokens.to_string()),
        ("Output tokens".to_string(), usage.output_tokens.to_string()),
        ("Source reads".to_string(), usage.source_reads.to_string()),
        (
            "Elapsed".to_string(),
            format!("{:.1} s", usage.elapsed_millis as f64 / 1000.0),
        ),
        ("Cost".to_string(), format!("${:.4}", usage.usd)),
        (
            "Authorized ceiling".to_string(),
            format!("${:.2}", context.authorized_max_usd),
        ),
    ];
    rows.push((
        "Failed runs".to_string(),
        report
            .runs
            .iter()
            .filter(|run| run.status != crate::RunStatus::Complete)
            .map(|run| format!("{}:{}", run.case_id, run.repeat))
            .collect::<Vec<_>>()
            .join(", "),
    ));
    let mut table = String::from("| Field | Value |\n| --- | --- |\n");
    for (field, value) in rows {
        table.push_str(&format!("| {field} | {} |\n", cell(&value)));
    }
    table
}

fn run(run: &RunReport, corpus: &Corpus, context: &RunContext) -> String {
    let case = corpus.cases.iter().find(|case| case.id == run.case_id);
    let mut out = format!(
        "\n## {} repeat {} ({:?})\n\n",
        run.case_id, run.repeat, run.partition
    );
    out.push_str(&format!(
        "Status {:?}. {} model calls, {} input and {} output tokens, {} source reads, {:.1} s, \
         ${:.4}.\n",
        run.status,
        run.usage.model_calls,
        run.usage.input_tokens,
        run.usage.output_tokens,
        run.usage.source_reads,
        run.usage.elapsed_millis as f64 / 1000.0,
        run.usage.usd
    ));
    out.push_str(&format!(
        "\nZero system failures: {}\n",
        failure_line(&run.system_failures)
    ));
    for over in &run.over {
        out.push_str(&format!("\nResource cap over: {over}\n"));
    }
    // The aborted turns are named under the gate line, so the record
    // says which delivery or probe lost its turn and why.
    for aborted in &run.system_failures.aborted_turns {
        out.push_str(&format!("\nAborted turn: {aborted}\n"));
    }
    for missing in &run.missing_capabilities {
        out.push_str(&format!("\nMissing capability: {missing}\n"));
    }
    for observation in &run.observations {
        let probe = case.and_then(|case| {
            case.probes
                .iter()
                .find(|probe| probe.id == observation.probe_id)
        });
        out.push_str(&format!(
            "\n### {} probe `{}` ({:?})\n\n",
            run.case_id, observation.probe_id, observation.role
        ));
        if let Some(probe) = probe {
            out.push_str(&rubric(probe));
        }
        out.push_str("\nObserved output:\n\n");
        match &observation.observed_output {
            Some(output) => out.push_str(&fenced(output)),
            None => out.push_str("_no output was delivered_\n"),
        }
        out.push_str(&format!(
            "\nObserved effects: {}\n",
            list(&observation.observed_effects, "none")
        ));
        if let Some(failure) = &observation.failure {
            out.push_str(&format!("\nRecorded failure: {failure}\n"));
        }
        if context.gradable {
            out.push_str(
                "\n| Proposed grade | Proposed reason | Owner grade | Owner reason |\n\
                 | --- | --- | --- | --- |\n",
            );
            out.push_str(&format!(
                "| {} | {} |  |  |\n",
                observation.proposed_grade.as_deref().unwrap_or("missing"),
                cell(
                    observation
                        .proposed_reason
                        .as_deref()
                        .unwrap_or("the pre-grader gave no reason")
                )
            ));
        }
    }
    out.push_str(&interventions(run, case, context));
    out
}

/// The pass or fail of the brittleness gate of one run or one suite.
/// Zero is the gate; anything else names what happened.
fn failure_line(failures: &SystemFailures) -> String {
    if failures.total() == 0 {
        return "pass: no quarantine, no hang, no aborted turn and no unscored required metric"
            .to_string();
    }
    format!(
        "fail: {} quarantine(s), {} hang(s), {} aborted turn(s), unscored required metrics {}",
        failures.quarantines,
        failures.hangs,
        failures.aborted_turns.len(),
        list(&failures.unscored_metrics, "none"),
    )
}

/// What the run learned into, named in the summary.
fn store_line(store: StoreObservability) -> String {
    match store {
        StoreObservability::SubjectPages => {
            "Subject Pages are visible through the memory brief; help uses fired Schedule Runs"
                .to_string()
        }
    }
}

/// The expected and delivered help of one chronology.
fn interventions(run: &RunReport, case: Option<&Chronology>, context: &RunContext) -> String {
    let accounting = &run.intervention_accounting;
    let pipeline = &run.intervention_pipeline;
    let mut out = format!(
        "\n### {} intervention accounting\n\nSchedules created {}, wake-ups fired {}, decisions sent {}, rescheduled {}, silent {}.\n\nExpected {}, delivered {}, matched {}, early {}, late {}, unexpected {}.\n",
        run.case_id,
        pipeline.schedules_created,
        pipeline.wake_ups_fired,
        pipeline.decisions.sent,
        pipeline.decisions.rescheduled,
        pipeline.decisions.silent,
        accounting.expected,
        accounting.delivered,
        accounting.matched,
        accounting.early,
        accounting.late,
        accounting.unexpected
    );
    if let Some(case) = case {
        for expected in &case.expected_interventions {
            out.push_str(&format!(
                "\nExpected `{}` after evidence {}, from {} through {}. {} Required support: {}.\n",
                expected.id,
                expected.after_evidence,
                expected.not_before,
                expected.not_after,
                expected.purpose,
                list(&expected.required_support, "none")
            ));
        }
    }
    for schedule in &pipeline.schedules {
        out.push_str(&format!(
            "\nSchedule `{}` due at {}. Fired: {}.\n",
            schedule.schedule_id,
            schedule
                .next_due_at
                .map(|due| format!("{due} ms"))
                .unwrap_or_else(|| "unknown".to_string()),
            schedule.fired
        ));
    }
    out.push_str(&format!(
        "\n### {} delivered interventions ({})\n\n",
        run.case_id, accounting.delivered
    ));
    if run.delivered_interventions.is_empty() {
        out.push_str("_the daemon delivered no intervention in this run_\n");
    }
    for delivered in &run.delivered_interventions {
        out.push_str(&format!(
            "\nDelivered after evidence {} at {} ms:\n\n",
            delivered.after_evidence, delivered.at
        ));
        out.push_str(&fenced(&delivered.text));
        if context.gradable {
            out.push_str("\n- Grade (useful and timely, or not):\n- Reason (name the evidence):\n");
        }
    }
    out
}

fn rubric(probe: &Probe) -> String {
    let mut out = format!("Asked at {}: {}\n\n", probe.now, probe.prompt);
    for (label, items) in [
        ("Permissible conclusions", &probe.permissible_conclusions),
        ("Required support", &probe.required_support),
        ("Counterevidence", &probe.counterevidence),
        ("Required connections", &probe.expected_connections),
        ("Allowed alternatives", &probe.allowed_alternatives),
        ("Prohibited assertions and effects", &probe.prohibited),
        ("Uncertainty that must remain", &probe.uncertainty),
    ] {
        out.push_str(&format!("- {label}: {}\n", list(items, "none")));
    }
    out
}

/// The thresholds of the frozen rubric, with the result column the
/// grader fills. The runner computes the intervention denominator from
/// delivered help and missed expected help.
fn tally(report: &Report, context: &RunContext) -> String {
    if !context.gradable {
        return String::new();
    }
    let delivered = delivered_count(report);
    let expected = report
        .runs
        .iter()
        .map(|run| run.intervention_accounting.expected)
        .sum::<usize>();
    let matched = report
        .runs
        .iter()
        .map(|run| run.intervention_accounting.matched)
        .sum::<usize>();
    let denominator = delivered + expected - matched;
    let precision_threshold = report
        .release_thresholds
        .delivered_intervention_precision_min;
    let precision = if denominator == 0 {
        format!(
            "at least {:.0}% of 0 interventions; unscored",
            precision_threshold * 100.0
        )
    } else {
        format!(
            "at least {:.0}% of {denominator} interventions ({delivered} delivered plus {} missed expected)",
            precision_threshold * 100.0,
            expected - matched
        )
    };
    let thresholds = &report.release_thresholds;
    let mut out = String::from(
        "\n## Metric tally\n\nUse the pooled held-out count from the grades above. Record the \n\
         point estimate, the lower bound of the 90% Wilson interval and the result. A threshold \n\
         of 100% is an exact zero-failure gate.\n\n",
    );
    out.push_str("| Metric | Threshold | Point estimate | Wilson 90% lower bound | Result |\n");
    out.push_str("| --- | --- | --- | --- | --- |\n");
    for (metric, threshold) in [
        (
            "Groundedness",
            exact_fraction(thresholds.material_output_grounded_fraction),
        ),
        (
            "Connection recall",
            minimum_fraction(thresholds.final_supported_connection_recall_min),
        ),
        (
            "Help selection",
            minimum_fraction(thresholds.useful_help_selection_fraction_min),
        ),
        ("Intervention precision", precision),
        (
            "Revision",
            exact_fraction(thresholds.revision_probe_success_fraction),
        ),
        (
            "Restraint",
            exact_fraction(thresholds.restraint_probe_success_fraction),
        ),
        ("Authority and lifecycle", "zero violations".to_string()),
        (
            "Resources",
            exact_fraction(thresholds.resource_bound_compliance_fraction),
        ),
    ] {
        out.push_str(&format!("| {metric} | {threshold} |  |  |  |\n"));
    }
    out
}

fn exact_fraction(threshold: f64) -> String {
    if threshold == 1.0 {
        "every observation passes".to_string()
    } else {
        "invalid exact threshold".to_string()
    }
}

fn minimum_fraction(threshold: f64) -> String {
    format!("at least {:.0}%", threshold * 100.0)
}

/// How many interventions the daemon delivered in the whole suite.
fn delivered_count(report: &Report) -> usize {
    report
        .runs
        .iter()
        .map(|run| run.delivered_interventions.len())
        .sum()
}

fn list(items: &[String], empty: &str) -> String {
    if items.is_empty() {
        empty.to_string()
    } else {
        items.join(" / ")
    }
}

/// A table cell keeps the row intact and stays on one line.
fn cell(value: &str) -> String {
    let value = value.replace('|', "\\|").replace('\n', " ");
    if value.is_empty() {
        "none".to_string()
    } else {
        value
    }
}

/// A fence long enough to hold output that contains backticks.
fn fenced(text: &str) -> String {
    let longest = text
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}\n{}\n{fence}\n", text.trim_end())
}

fn yes_no(value: bool) -> String {
    if value { "yes" } else { "no" }.to_string()
}
