//! The release-evaluation entry points: the complete
//! frozen suite through a real daemon on the responsible Agent's
//! configured model alias.
//!
//! Every entry point is `#[ignore]`, because each boots 74 daemons and
//! the authorized ones spend the owner's money. An authorized run is
//! refused without an explicit ceiling in `PAGIS_EVALUATION_MAX_USD`,
//! which names the evaluation ledger and never the product's own
//! background quota.
//!
//! ```text
//! ANTHROPIC_API_KEY=... PAGIS_EVALUATION_MAX_USD=25 \
//!   cargo test -p pagis --test main -- --ignored --nocapture \
//!   release_evaluation::the_configured_model_runs_the_release_suite
//! ```
//!
//! The alias routes are the product's seeded default. The owner names
//! other `provider/model` routes, comma-separated, in
//! `PAGIS_EVALUATION_ROUTES`; the run prices them the same way, writes
//! them over the seeded alias of every daemon it boots, and records
//! them in the report.
//! The manifest's different pre-grader route uses the same provider
//! path, meter and evaluation ledger.
//!
//! The run writes `report.json` and `GRADING.md` under
//! `crates/pagis-evaluation/suites/continuous-learning/runs/<timestamp>/` when
//! `PAGIS_EVALUATION_RECORD=1`, else under the target directory. Grading is
//! the owner's work: the report carries model proposals and visible
//! output only, never a hidden model trajectory.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use pagis_agent::{Brain, RouterBrain};
use pagis_core::{MemorySecretStore, ProviderKeys, secrets::Provider};
use pagis_evaluation::grading::{RunContext, write_run};
use pagis_evaluation::pricing::{Priced, RouteRate, preflight};
use pagis_evaluation::{
    Authorization, ChronologyDriver, Corpus, DriverResult, Manifest, Report, RunStatus,
    StoreObservability, load_suite, run_suite,
};
use pagis_testkit::evaluation::{DaemonDriver, EvaluationSpend, ScriptedModel};

/// The environment variable that authorizes spend. The manifest never
/// does, and the product's background quota is a different ledger.
const AUTHORIZATION: &str = "PAGIS_EVALUATION_MAX_USD";
/// Set to `1` to record a run under
/// `crates/pagis-evaluation/suites/continuous-learning/runs/`.
const RECORD: &str = "PAGIS_EVALUATION_RECORD";
/// Comma-separated `provider/model` routes that replace the seeded
/// default of the alias for this run.
const ROUTES: &str = "PAGIS_EVALUATION_ROUTES";
/// The free local route of the smoke run.
const SCRIPTED_ROUTE: &str = "scripted-smoke";

/// The directory of this crate, read at run time.
fn manifest_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
}

fn base() -> PathBuf {
    manifest_dir().join("../pagis-evaluation/suites/continuous-learning")
}

/// The release manifest and its named corpus.
fn suite() -> (Manifest, Corpus, String) {
    load_suite(&base().join("release.json")).expect("the frozen suite")
}

/// The exact code the run exercised. A dirty tree is named as such, so
/// no report claims an unreachable commit.
fn code_commit() -> String {
    let git = |arguments: &[&str]| {
        std::process::Command::new("git")
            .current_dir(manifest_dir())
            .args(arguments)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let head = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    match git(&["status", "--porcelain"]) {
        Some(changes) if !changes.is_empty() => format!("{head} (uncommitted changes)"),
        _ => head,
    }
}

/// The configured price of one `provider/model` route, read from the
/// same registry the daemon's router charges against.
fn rate(route: &str) -> Option<RouteRate> {
    let (_, model) = route.split_once('/')?;
    llm_router::model_info(model).map(|info| RouteRate {
        input_usd_per_mtok: info.prices.input_cost,
        output_usd_per_mtok: info.prices.output_cost,
    })
}

/// The routes the alias resolves to for this run: the owner's
/// `PAGIS_EVALUATION_ROUTES`, else the seeded default of every Agent.
fn configured_routes() -> Vec<String> {
    match std::env::var(ROUTES) {
        Ok(named) => named
            .split(',')
            .map(str::trim)
            .filter(|route| !route.is_empty())
            .map(str::to_string)
            .collect(),
        Err(_) => vec![pagis_server::provisioning::seed_default_model().to_string()],
    }
}

/// The routes whose worst-case price the shared evaluation ledger must
/// reserve. The pre-grader uses the same meter and budget.
fn budget_routes(manifest: &Manifest, routes: &[String]) -> Vec<String> {
    let mut budgeted = routes.to_vec();
    if !budgeted.contains(&manifest.grading.pre_grader_route) {
        budgeted.push(manifest.grading.pre_grader_route.clone());
    }
    budgeted
}

/// Keys from the environment only. The run does not read the owner's
/// keychain: the owner exports the key for the run they authorized.
fn provider_keys() -> Arc<ProviderKeys> {
    Arc::new(ProviderKeys::new(
        HashMap::new(),
        Arc::new(MemorySecretStore::default()),
    ))
}

/// A driver that proves a refused run reaches no model. It carries the
/// route, the name and the store of the arm it stands for, so the
/// refused record says which arm was refused.
struct Refused(String, String, StoreObservability);

#[async_trait]
impl ChronologyDriver for Refused {
    async fn run(&mut self, _: &pagis_evaluation::Chronology, _: u8) -> DriverResult {
        unreachable!("a refused run calls no model")
    }
    fn name(&self) -> &str {
        &self.1
    }
    fn store(&self) -> StoreObservability {
        self.2
    }
    fn model_route(&self) -> &str {
        &self.0
    }
    fn clock_version(&self) -> &str {
        "not started"
    }
    fn zone_rule_version(&self) -> &str {
        "not started"
    }
}

/// Why the authorized run cannot start, if it cannot. The order is the
/// order the manifest reads: a priced route, then an authorized
/// ceiling, then a provider that can serve the route.
fn refusal(manifest: &Manifest, priced: &Result<Priced, String>) -> Option<String> {
    let priced = match priced {
        Ok(priced) => priced,
        Err(reason) => return Some(reason.clone()),
    };
    let Some(authorized) = std::env::var(AUTHORIZATION)
        .ok()
        .map(|value| value.parse::<f64>())
    else {
        return Some(format!(
            "explicit spend authorization: set {AUTHORIZATION} to the USD ceiling of the \
             evaluation ledger, at least ${:.2}",
            manifest.suite.max_total_usd
        ));
    };
    let Ok(authorized) = authorized else {
        return Some(format!("a numeric {AUTHORIZATION} in USD"));
    };
    let needed = manifest.suite.max_total_usd.max(priced.usd_for_suite);
    if authorized < needed {
        return Some(format!(
            "sufficient spend authorization: {AUTHORIZATION} is ${authorized:.2} and the suite \
             reserves up to ${needed:.2}"
        ));
    }
    let keys = provider_keys();
    for route in &priced.routes {
        let Some(provider) = route
            .route
            .split_once('/')
            .and_then(|(provider, _)| Provider::from_id(provider))
        else {
            return Some(format!("a known provider for route {}", route.route));
        };
        if !matches!(keys.resolve(provider), Ok(Some(_))) {
            return Some(format!(
                "a configured model provider key: export {} for {}",
                provider.env_var(),
                route.route
            ));
        }
    }
    None
}

/// The authorized ceiling, or zero when the environment names none.
fn authorized_usd() -> f64 {
    std::env::var(AUTHORIZATION)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_default()
}

/// Where one run's artifacts go: a directory named after its start.
/// The docs tree keeps a run only when the owner asks for it with
/// `PAGIS_EVALUATION_RECORD=1`; every other run goes under the target
/// directory and leaves the checkout clean.
fn run_directory(suffix: &str) -> PathBuf {
    let started = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let root = if std::env::var(RECORD).is_ok_and(|value| value == "1") {
        base().join("runs")
    } else {
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("evaluation-runs")
    };
    root.join(format!("{started}{suffix}"))
}

/// Every recorded failure of a report, so a mismatch names the reason.
fn diagnosis(report: &Report) -> String {
    let mut lines = vec![format!(
        "status {:?}, missing {:?}",
        report.status, report.missing_capabilities
    )];
    for run in &report.runs {
        if run.status == RunStatus::Complete {
            continue;
        }
        lines.push(format!(
            "{}:{} is {:?}, missing {:?}",
            run.case_id, run.repeat, run.status, run.missing_capabilities
        ));
        for probe in run.observations.iter().filter(|p| p.failure.is_some()) {
            lines.push(format!("  probe {}: {:?}", probe.probe_id, probe.failure));
        }
    }
    lines.join("\n")
}

/// The capabilities the driver reported missing, in one sorted list.
fn missing_capabilities(report: &Report) -> Vec<String> {
    let mut missing: BTreeSet<String> = report.missing_capabilities.iter().cloned().collect();
    for run in &report.runs {
        missing.extend(run.missing_capabilities.iter().cloned());
    }
    missing.into_iter().collect()
}

/// The structural shape every complete suite has: one run for each
/// chronology repeat and one observation for each probe of it.
fn assert_complete(report: &Report, manifest: &Manifest, corpus: &Corpus) {
    let expected_runs = corpus.cases.len() * manifest.suite.repeats_per_chronology as usize;
    assert_eq!(report.runs.len(), expected_runs, "{}", diagnosis(report));
    let probes: usize = report.runs.iter().map(|run| run.observations.len()).sum();
    assert_eq!(
        probes,
        corpus
            .cases
            .iter()
            .map(|case| case.probes.len())
            .sum::<usize>()
            * manifest.suite.repeats_per_chronology as usize,
        "{}",
        diagnosis(report)
    );
    assert_eq!(report.status, RunStatus::Complete, "{}", diagnosis(report));
}

/// The full suite on the responsible Agent's configured model. It
/// refuses an unpriced route and an unauthorized or unaffordable run,
/// and calls no model in either case.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "boots 74 daemons and spends the authorization in PAGIS_EVALUATION_MAX_USD"]
async fn the_configured_model_runs_the_release_suite() {
    let (manifest, corpus, hash) = suite();
    let alias = pagis_core::DEFAULT_MODEL_ALIAS;
    let routes = configured_routes();
    let budgeted = budget_routes(&manifest, &routes);
    let priced = preflight(alias, &budgeted, &manifest.suite, rate);
    let route = format!("{alias} via {}", routes.join(", "));
    let described: Vec<String> = match &priced {
        Ok(priced) => priced
            .routes
            .iter()
            .map(|priced| {
                format!(
                    "{} at ${:.2}/${:.2} per Mtok",
                    priced.route, priced.rate.input_usd_per_mtok, priced.rate.output_usd_per_mtok
                )
            })
            .collect(),
        Err(_) => routes.clone(),
    };
    let context = RunContext {
        title: format!("the configured model on alias `{alias}`"),
        gradable: true,
        code_commit: code_commit(),
        model_alias: alias.to_string(),
        routes: described,
        authorized_max_usd: authorized_usd(),
    };

    if let Some(reason) = refusal(&manifest, &priced) {
        // The refused run is evidence too: it records the capability it
        // lacked, and it called no model to learn it.
        let mut report = run_suite(
            &manifest,
            &corpus,
            hash,
            None,
            &mut Refused(route, "daemon".into(), StoreObservability::SubjectPages),
        )
        .await
        .expect("validate the suite");
        report.missing_capabilities.push(reason.clone());
        assert_eq!(report.status, RunStatus::Unscored);
        assert!(report.runs.is_empty());
        let written = write_run(&run_directory("-refused"), &report, &corpus, &context)
            .expect("write the refused run");
        // A run with no authorization is refused. A refusal is the correct
        // outcome there, so it is reported and not failed.
        eprintln!(
            "the run is unscored and needs {reason}\nrecorded in {}",
            written.display()
        );
        return;
    }

    let priced = priced.expect("a refusal names an unpriced alias");
    let mut driver = DaemonDriver::new(
        {
            let keys = provider_keys();
            let models = Arc::new(pagis_agent::ModelCatalog::new(Arc::clone(&keys)));
            Arc::new(RouterBrain::new(keys, models)) as Arc<dyn Brain>
        },
        route.clone(),
        routes.clone(),
        EvaluationSpend {
            authorization: Authorization {
                max_usd: authorized_usd(),
                priced_routes: BTreeSet::from([route.clone()]),
            },
            reserve_per_run_usd: priced.usd_per_chronology_repeat,
            rate: priced.settle_rate,
        },
    );
    let authorization = Authorization {
        max_usd: authorized_usd(),
        priced_routes: BTreeSet::from([route]),
    };
    let report = run_suite(&manifest, &corpus, hash, Some(&authorization), &mut driver)
        .await
        .expect("validate the suite");

    let written = write_run(&run_directory(""), &report, &corpus, &context)
        .expect("write the authorized run");
    println!(
        "the run is recorded in {} and is unscored until the owner grades it; \
         missing capabilities: {:?}; spent ${:.4} of the ${:.2} reserved",
        written.display(),
        missing_capabilities(&report),
        report.usage.usd,
        priced.usd_for_suite
    );
    assert_complete(&report, &manifest, &corpus);
}

/// The same entry point on a scripted local model. It costs nothing,
/// uses scripted grade proposals and proves the runner reaches the
/// shipped path for every chronology of the frozen corpus.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "boots 74 daemons; the smoke run is unscored by definition"]
async fn a_scripted_model_smoke_runs_the_release_suite() {
    let (manifest, corpus, hash) = suite();
    let authorization = Authorization {
        max_usd: manifest.suite.max_total_usd,
        priced_routes: BTreeSet::from([SCRIPTED_ROUTE.to_string()]),
    };
    let mut driver = DaemonDriver::new(
        Arc::new(ScriptedModel::default()) as Arc<dyn Brain>,
        SCRIPTED_ROUTE,
        vec!["anthropic/claude-haiku-4-5".to_string()],
        EvaluationSpend {
            authorization: authorization.clone(),
            reserve_per_run_usd: manifest.suite.max_usd_per_chronology_repeat,
            // A local route is explicitly zero-cost; its token and time
            // caps still apply.
            rate: RouteRate {
                input_usd_per_mtok: 0.0,
                output_usd_per_mtok: 0.0,
            },
        },
    );
    let report = run_suite(&manifest, &corpus, hash, Some(&authorization), &mut driver)
        .await
        .expect("validate the suite");

    let context = RunContext {
        title: "a scripted local model (smoke run)".into(),
        gradable: false,
        code_commit: code_commit(),
        model_alias: SCRIPTED_ROUTE.into(),
        routes: vec![format!("{SCRIPTED_ROUTE} at $0.00/$0.00 per Mtok")],
        authorized_max_usd: 0.0,
    };
    let written = write_run(
        &run_directory("-scripted-smoke"),
        &report,
        &corpus,
        &context,
    )
    .expect("write the smoke run");
    println!(
        "the smoke run is recorded in {} and is unscored by definition; \
         missing capabilities: {:?}",
        written.display(),
        missing_capabilities(&report)
    );
    assert_eq!(report.usage.usd, 0.0);
    assert_complete(&report, &manifest, &corpus);
}
