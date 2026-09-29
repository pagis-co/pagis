//! The contract check of the Google adapter with the pinned `gog`.
//!
//! The contract tests of `pagis-google` check the argv that the adapter
//! builds for each tool call. Its `pinned_gog` tests give that argv to
//! the `gog` release that each release bundles (`GOG_VERSION`). They show
//! that this `gog` reads each value from the Agent as a value, and that
//! each call keeps the account and the client of its Connection. They
//! need no Google account and send no request to Google.
//!
//! The step downloads the pinned `gog` archive of the host, checks it
//! against its pinned SHA-256 (see [`tools::command`]), and runs those
//! tests with `PAGIS_GOG` set to the binary. The full gate runs the step.
//! The development check runs it when `pagis-google` or a package that it
//! depends on changed. A host that has no pin skips the step.

use std::path::Path;

use crate::{Action, Cmd, Step, tools};

/// Run the ignored `pinned_gog` tests of `pagis-google` with the pinned
/// `gog`. The test step of the gate leaves them out.
const CONTRACT_TESTS: &str = "PAGIS_GOG=\"$work/gog\" cargo nextest run -p pagis-google \
     --run-ignored only -E 'test(/^pinned_gog::/)'\n";

/// A command that runs `body` in `sh` at `root`, with the pinned `gog` of
/// this host at `$work/gog` (see [`tools::command`]), or why this host
/// has no pin.
pub fn command(root: &Path, target_dir: &Path, body: &str) -> Result<Cmd, String> {
    let pin = tools::gog()?;
    Ok(tools::command(root, target_dir, Ok(pin), body, &[]))
}

/// The contract check of the workspace at `root`.
pub fn contract_step(root: &Path, target_dir: &Path) -> Step {
    let action = match command(root, target_dir, CONTRACT_TESTS) {
        Ok(cmd) => Action::Run(vec![cmd]),
        Err(reason) => Action::Skip(reason),
    };
    Step {
        name: "gog-contract",
        action,
    }
}
