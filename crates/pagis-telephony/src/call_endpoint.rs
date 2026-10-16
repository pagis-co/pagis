//! The carrier operations the realtime bridge needs.

use crate::hub::MediaHub;
use crate::{BridgeError, TransportCapabilities};
use async_trait::async_trait;
use pagis_core::{PhoneNumberId, WorkspaceId};
use std::sync::Arc;

/// A call the line answered, ready for whoever runs it. The carrier
/// names only the dialed number, so the line says whose number it is:
/// the stored record of the number names the Workspace that holds it.
pub struct IncomingHub {
    pub workspace_id: WorkspaceId,
    pub phone_number_id: PhoneNumberId,
    /// The number the Remote Party dialed.
    pub e164: String,
    pub from_e164: String,
    pub hub: Arc<MediaHub>,
}

/// A carrier supplies its capabilities and opens outbound media.
#[async_trait]
pub trait CallEndpoints: Send + Sync {
    fn capabilities(&self) -> TransportCapabilities;
    async fn place_call(
        &self,
        workspace_id: &WorkspaceId,
        number_id: &PhoneNumberId,
        to_e164: &str,
    ) -> Result<Arc<MediaHub>, BridgeError>;
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl CallEndpoints for crate::Endpoints {
    fn capabilities(&self) -> TransportCapabilities {
        self.capabilities()
    }

    async fn place_call(
        &self,
        workspace_id: &WorkspaceId,
        number_id: &PhoneNumberId,
        to_e164: &str,
    ) -> Result<Arc<MediaHub>, BridgeError> {
        self.place_call(workspace_id, number_id, to_e164)
            .await
            .map_err(|error| BridgeError(error.as_str().to_string()))
    }
}
