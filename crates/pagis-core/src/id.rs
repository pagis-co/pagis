//! Typed ULID identifiers for durable records.

use serde::{Deserialize, Serialize};

/// Process-wide monotonic ULIDs: ids generated in the same millisecond
/// still sort in generation order. Stores order and page by id, so
/// non-monotonic ids would shuffle same-millisecond records.
fn next_ulid() -> ulid::Ulid {
    static GENERATOR: std::sync::Mutex<Option<ulid::Generator>> = std::sync::Mutex::new(None);
    let mut guard = GENERATOR.lock().expect("ulid generator lock");
    guard
        .get_or_insert_with(ulid::Generator::new)
        .generate()
        // The same-millisecond random part overflowed: a fresh random
        // ULID is still correct, just not ordered within this ms.
        .unwrap_or_else(|_| ulid::Ulid::new())
}

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Generate a new ULID-backed id.
            pub fn generate() -> Self {
                Self(next_ulid().to_string())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_type!(
    /// The installation. Exactly one row exists: the Org owns
    /// what an administrator configures.
    OrgId
);
id_type!(
    /// One person in the Org.
    UserId
);
id_type!(
    /// What a signed-in client holds. The cookie carries a
    /// secret, never this id.
    SessionId
);
id_type!(
    /// One one-time sign-in link.
    SignInLinkId
);
id_type!(
    /// One Usage Record: what one model call of one Run spent.
    UsageId
);
id_type!(
    /// One Model Request Capture: one model request of a Run and the
    /// provider's answer.
    ModelRequestCaptureId
);
id_type!(
    /// One machine a Person's client runs on. A host action runs
    /// there and never in the daemon.
    HostId
);
id_type!(
    /// One Push Subscription: the push endpoint of one client of one
    /// Session (ADR-0030).
    PushSubscriptionId
);
id_type!(WorkspaceId);
id_type!(AgentId);
id_type!(ChannelId);
id_type!(MessageId);
id_type!(RunId);
id_type!(ParticipantId);
id_type!(EventId);
id_type!(RequestId);
id_type!(ArtifactId);
id_type!(GrantId);
id_type!(ModelAliasId);
id_type!(ConnectionId);
id_type!(CredentialId);
id_type!(ScheduleId);
id_type!(ScheduleOccurrenceId);
id_type!(WakeupId);
id_type!(PendingEvidenceId);
id_type!(EventSubscriptionId);
id_type!(IncomingEventId);
id_type!(SourceBatchId);
id_type!(PhoneNumberId);
id_type!(
    /// One Agent Mailbox (ADR-0019). The record outlives its delete as
    /// a tombstone, so the id names a row in the Address Ledger for as
    /// long as the Workspace exists.
    AgentMailboxId
);
id_type!(
    /// One Call (ADR-0020). The id is minted here so the audit events
    /// and the bridge share one.
    CallId
);
id_type!(
    /// One Text Record (ADR-0020): one text the Agent sent or
    /// received. The record stays with the Agent whatever happens to
    /// the number, as a Call does.
    TextRecordId
);
id_type!(PurchaseIntentId);
id_type!(
    /// One row of the Trust List (ADR-0021, ADR-0019).
    TrustEntryId
);
id_type!(
    /// One Software Package in the Workspace's Software List
    /// (ADR-0016). Its versions are git tags, not records with ids.
    SoftwarePackageId
);
id_type!(
    /// One Contribution from a Fork to its origin Software Package
    /// (ADR-0016).
    ContributionId
);
id_type!(
    /// One installed Plugin of the Workspace (ADR-0017). Its
    /// installed states are commits in its own repository, not records
    /// with ids.
    PluginId
);

#[cfg(test)]
mod tests {
    use super::MessageId;

    #[test]
    fn ids_generated_in_the_same_millisecond_sort_in_generation_order() {
        let ids: Vec<String> = (0..1000)
            .map(|_| MessageId::generate().as_str().to_string())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
    }
}
