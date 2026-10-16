//! A Workspace Number cannot be released while a Call on it is active.

use async_trait::async_trait;
use pagis_core::PhoneNumberId;

#[async_trait]
pub trait ActiveCalls: Send + Sync {
    /// True while a Call on this number is running.
    async fn is_active(&self, number_id: &PhoneNumberId) -> bool;
}

/// A seam that reports no active Call on any number.
pub struct NoActiveCalls;

#[async_trait]
impl ActiveCalls for NoActiveCalls {
    async fn is_active(&self, _number_id: &PhoneNumberId) -> bool {
        false
    }
}
