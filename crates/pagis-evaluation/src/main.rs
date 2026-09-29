use async_trait::async_trait;
use pagis_evaluation::{
    Chronology, ChronologyDriver, DriverResult, RunStatus, StoreObservability, load_suite,
    run_suite,
};
use std::path::PathBuf;

struct Unavailable;

#[async_trait]
impl ChronologyDriver for Unavailable {
    async fn run(&mut self, _: &Chronology, _: u8) -> DriverResult {
        unreachable!("preflight stops before an unauthorized model call")
    }
    fn name(&self) -> &str {
        "daemon"
    }
    fn store(&self) -> StoreObservability {
        StoreObservability::SubjectPages
    }
    fn model_route(&self) -> &str {
        "existing-responsible-agent-model-alias"
    }
    fn clock_version(&self) -> &str {
        "system-clock-unavailable"
    }
    fn zone_rule_version(&self) -> &str {
        "system-zone-rules-unavailable"
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    )
    .join("suites/continuous-learning/release.json");
    let (manifest, corpus, hash) = load_suite(&manifest)?;
    let report = run_suite(&manifest, &corpus, hash, None, &mut Unavailable).await?;
    assert_eq!(report.status, RunStatus::Unscored);
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
