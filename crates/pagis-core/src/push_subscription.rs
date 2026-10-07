//! The Push Subscriptions of a Person (ADR-0030).
//!
//! A Push Subscription is the push endpoint of one client, with the
//! client's P-256 public key and auth secret. It belongs to one Session
//! and ends when that Session ends: the row references the Session, and
//! the database deletes it with the Session. A sign-out, a removal from
//! the Sessions list and the expiry sweep therefore need no step of
//! their own.
//!
//! Every read and write names the Workspace, as every other record of a
//! Person does, so one Person never reaches another Person's endpoint.

use async_trait::async_trait;

use crate::id::{PushSubscriptionId, SessionId, WorkspaceId};
use crate::store::StoreError;
use crate::time::UnixMillis;

/// The push endpoint of one client, as the client subscribed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushSubscription {
    pub id: PushSubscriptionId,
    /// The Workspace of the Person whose Session holds the endpoint.
    pub workspace_id: WorkspaceId,
    /// The Session of the client. The Push Subscription ends with it.
    pub session_id: SessionId,
    /// The `https` URL of the push service that takes each Web Push. It
    /// is unique: one client holds one endpoint.
    pub endpoint: String,
    /// The client's P-256 public key, as base64url of the uncompressed
    /// point (RFC 8291).
    pub p256dh: String,
    /// The client's 16-byte auth secret, as base64url (RFC 8291).
    pub auth: String,
    pub created_at: UnixMillis,
    /// When the daemon last sent a Web Push to the endpoint. `None`
    /// until the first one.
    pub last_sent_at: Option<UnixMillis>,
}

#[async_trait]
pub trait PushSubscriptionStore: Send + Sync {
    /// Write the Push Subscription and answer the row that the store
    /// keeps. The endpoint is the identity: a client that subscribes
    /// again lands on its own row. A known endpoint moves to the
    /// Workspace and the Session of `subscription` and takes its keys.
    /// When the Session changes, the row is a new subscription of that
    /// Session, so it takes the new `created_at` and forgets
    /// `last_sent_at`; the same Session keeps both. The id of the row
    /// stays.
    async fn upsert(&self, subscription: &PushSubscription)
    -> Result<PushSubscription, StoreError>;
    /// The Push Subscriptions of one Workspace, oldest first.
    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<PushSubscription>, StoreError>;
    /// Remove one Push Subscription of one Workspace. `false` when the
    /// Workspace holds no such row, so a row of another Workspace reads
    /// as absent.
    async fn delete(
        &self,
        workspace_id: &WorkspaceId,
        id: &PushSubscriptionId,
    ) -> Result<bool, StoreError>;
    /// Remove the Push Subscription of one endpoint in one Workspace: the
    /// push service answered that the endpoint is gone. `false` when the
    /// Workspace holds no row of that endpoint.
    async fn delete_by_endpoint(
        &self,
        workspace_id: &WorkspaceId,
        endpoint: &str,
    ) -> Result<bool, StoreError>;
    /// Remember that a Web Push went to the endpoint at `at`. `false`
    /// when the Workspace holds no such row.
    async fn mark_sent(
        &self,
        workspace_id: &WorkspaceId,
        id: &PushSubscriptionId,
        at: UnixMillis,
    ) -> Result<bool, StoreError>;
}
