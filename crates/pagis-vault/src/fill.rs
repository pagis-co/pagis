//! The verified fill (ADR-0013).
//!
//! A fill writes a secret only into a page that the daemon verified, and
//! only through the browser channel that the daemon owns. The Agent
//! chooses which Credential to fill. It never chooses the page, the
//! field or the window.
//!
//! `vault__fill` takes the input switch and opens the record's
//! `login_url` in the daemon's own tab of the Computer's browser. It
//! waits for the load event of the page, not for a fixed time, and then
//! reads the top-level address that the tab shows after every redirect.
//! The fill continues only when that address is `https` and its
//! registrable domain is the record's domain. screend then writes the
//! username into the text or email field that has the focus in the top
//! frame, and the secret into the password field after it in the same
//! form. When the page gives the focus to no input, screend finds the
//! login fields itself: the first password field on the screen and the
//! last text or email field before it in the same form. A focused field
//! of another kind or outside the login form stops the fill. screend
//! gives each field the focus and checks the origin and the focused
//! field again in the same step as each write, so the focus cannot move
//! to another element between the check and the write.
//!
//! `vault__totp_fill` does not navigate. It checks the address that the
//! daemon's tab shows now with the same rules, and screend writes the
//! code into the text, email or one-time-code field that has the focus,
//! or, when no input has the focus, into the one one-time-code field or
//! the one text field on the screen.
//!
//! The text goes through the DevTools pipe of the browser. It never goes
//! as keystrokes to the window that has the focus, so a terminal or any
//! other window that holds the focus gets nothing. A single sign-on host
//! on another registrable domain needs a Credential of its own.
//!
//! A fill that fails a check writes nothing and reports `failed` with
//! the reason.

use std::sync::Arc;

use pagis_computer::{ComputerError, ComputerManager, DaemonHold, FillField};
use pagis_core::AgentId;

use crate::{VaultError, domain};

/// Open `login_url` in the daemon's tab and write the username and the
/// secret into the verified fields of the page of `domain`. The switch
/// is held for the whole fill, and the hold releases also when a step
/// fails.
pub async fn open_and_fill(
    computer: &Arc<ComputerManager>,
    agent_id: &AgentId,
    domain: &str,
    login_url: &str,
    username: &str,
    secret: &str,
) -> Result<(), VaultError> {
    let hold = hold(computer, agent_id).await?;
    let filled = async {
        let page = hold.open(login_url).await.map_err(failed)?;
        let origin = domain::verified_origin(domain, &page)?;
        hold.fill(
            &origin,
            &[FillField::text(username), FillField::password(secret)],
        )
        .await
        .map_err(failed)
    }
    .await;
    hold.release().await;
    filled
}

/// Write one one-time code into the verified field of the page the
/// daemon's tab shows now. A code follows a password the site already
/// took, so this does not navigate: to open `login_url` again would
/// leave the step that asks for the code.
pub async fn fill_code(
    computer: &Arc<ComputerManager>,
    agent_id: &AgentId,
    domain: &str,
    code: &str,
) -> Result<(), VaultError> {
    let hold = hold(computer, agent_id).await?;
    let filled = async {
        let page = hold.page().await.map_err(failed)?;
        let origin = domain::verified_origin(domain, &page)?;
        hold.fill(&origin, &[FillField::text(code)])
            .await
            .map_err(failed)
    }
    .await;
    hold.release().await;
    filled
}

/// Wake the Computer and take its input switch for the daemon.
async fn hold(
    computer: &Arc<ComputerManager>,
    agent_id: &AgentId,
) -> Result<DaemonHold, VaultError> {
    computer
        .ensure_awake(agent_id)
        .await
        .map_err(|error| VaultError::Computer(error.to_string()))?;
    computer
        .daemon_hold(agent_id)
        .await
        .map_err(|error| VaultError::Computer(error.to_string()))
}

/// A step of the browser channel that did not complete wrote nothing,
/// and its reason is the reason the fill failed.
fn failed(error: ComputerError) -> VaultError {
    match error {
        ComputerError::Runtime(reason) => VaultError::FillFailed(reason),
        other => VaultError::FillFailed(other.to_string()),
    }
}
