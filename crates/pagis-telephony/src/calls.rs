//! Whether a number is on a call right now.
//!
//! Assign, unassign and release all refuse while a Call on the number
//! is active (ADR-0018), and so does deleting the carrier Connection.
//! This seam is how the refusals ask, so a test can script the answer.

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
