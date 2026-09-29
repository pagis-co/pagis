//! The Keypad Code check the bridge proves a tier against
//! (ADR-0021). The vault owns each Workspace's code and its hash;
//! telephony asks this adapter with the call's Workspace and never
//! holds either.

use std::sync::Arc;

use async_trait::async_trait;
use pagis_core::{SecretStore, WorkspaceId};
use pagis_telephony::keypad::CodeCheck;
use pagis_vault::KeypadCode;

pub struct VaultCodeCheck {
    secrets: Arc<dyn SecretStore>,
}

impl VaultCodeCheck {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self { secrets }
    }
}

#[async_trait]
impl CodeCheck for VaultCodeCheck {
    async fn is_set(&self, workspace_id: &WorkspaceId) -> bool {
        KeypadCode::new(self.secrets.as_ref(), workspace_id)
            .is_set()
            .unwrap_or_else(|error| {
                tracing::error!(%error, %workspace_id, "the keypad code was not read");
                false
            })
    }

    async fn verify(&self, workspace_id: &WorkspaceId, digits: &str) -> bool {
        KeypadCode::new(self.secrets.as_ref(), workspace_id)
            .verify(digits)
            .unwrap_or_else(|error| {
                tracing::error!(%error, %workspace_id, "the keypad code was not verified");
                false
            })
    }
}
