//! Who the daemon serves: the Org, the people in it, and what a
//! signed-in client holds.
//!
//! One installation is one Org. A Person belongs to it and owns one
//! Workspace, which stays their private scope. A Session is what a
//! client holds after it signs in; the cookie carries a secret and the
//! record holds only the hash of it, so a stolen database row cannot
//! sign anybody in.

use async_trait::async_trait;

use crate::id::{OrgId, SessionId, SignInLinkId, UserId, WorkspaceId};
use crate::store::StoreError;
use crate::time::UnixMillis;

/// The installation. Exactly one row exists on a local installation or
/// a server; the Org owns what an administrator configures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Org {
    pub id: OrgId,
    pub name: String,
    /// The Google Web OAuth client of this installation, or
    /// `None` where the installation has none and every person supplies
    /// their own Desktop client. The id is not a secret; the matching
    /// client secret is filed in the secret store under
    /// [`GOOGLE_WEB_CLIENT_SECRET`].
    ///
    /// It belongs to the Org and not to a Workspace: one installation
    /// registers one client, and every person consents against it.
    pub google_client_id: Option<String>,
    /// The Workspace that holds the Org's own records: the installed
    /// Plugins with their Bindings and checkouts, and the Installation
    /// Connections (ADR-0017). No person owns it. The store writes it
    /// with the Org, the Workspace store never answers it, and no
    /// Session reaches it, so every person reads these records through
    /// the Org and none of them through their own Workspace.
    pub workspace_id: WorkspaceId,
    pub created_at: UnixMillis,
}

/// The name the installation's Google Web client secret is filed under.
/// It is an installation secret, like a model provider key: one Org
/// holds it and no Workspace does.
pub const GOOGLE_WEB_CLIENT_SECRET: &str = "google/web_client_secret";

/// What a Person may do across the installation. The role is
/// installation-wide, not per Workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserRole {
    Administrator,
    Member,
}

impl UserRole {
    pub fn as_str(self) -> &'static str {
        match self {
            UserRole::Administrator => "administrator",
            UserRole::Member => "member",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "administrator" => Some(UserRole::Administrator),
            "member" => Some(UserRole::Member),
            _ => None,
        }
    }
}

/// One person in the Org. The name lives here: the agents read it out
/// of the memory file the daemon writes from this record.
///
/// The record is not `Eq`: the Spend Cap is an amount of money, and the
/// cost the router reports is a float, so the two compare as floats.
#[derive(Debug, Clone, PartialEq)]
pub struct User {
    pub id: UserId,
    pub org_id: OrgId,
    /// How the person signs in with a password. `None` for the seeded
    /// person of a local installation, who signs in with the Client
    /// Credential and may set an address later.
    pub email: Option<String>,
    /// What the agents call the person; `None` until onboarding asks.
    pub name: Option<String>,
    /// The argon2id hash of the password. `None` means this person has
    /// no password and cannot sign in with one.
    pub password_hash: Option<String>,
    pub role: UserRole,
    /// When an Administrator disabled the account, or `None` while it is
    /// live. A disabled Person signs in to nothing and their Sessions
    /// are gone; their Workspace and its records stay, so re-enabling
    /// gives the same person the same Workspace back.
    pub disabled_at: Option<UnixMillis>,
    /// When a client last signed in as this Person, for the roster.
    pub last_signed_in_at: Option<UnixMillis>,
    /// What the Person may spend on model calls in a calendar month, in
    /// US dollars. `None` is no cap, which is what a local installation
    /// and every account an Administrator does not cap holds.
    pub monthly_spend_cap_usd: Option<f64>,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
}

impl User {
    /// A new Person of one Org, with nothing an Administrator has set
    /// yet: no address, no name, no password, live, never signed in and
    /// under no Spend Cap. A caller fills in what it knows with struct
    /// update syntax, so a field added later reaches every caller.
    pub fn new(org_id: OrgId, role: UserRole, at: UnixMillis) -> Self {
        Self {
            id: UserId::generate(),
            org_id,
            email: None,
            name: None,
            password_hash: None,
            role,
            disabled_at: None,
            last_signed_in_at: None,
            monthly_spend_cap_usd: None,
            created_at: at,
            updated_at: at,
        }
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled_at.is_some()
    }
}

/// Which client holds a Session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientKind {
    /// A browser that signed in with a password or a sign-in link.
    Browser,
    /// The Client App, which traded the Client Credential.
    Desktop,
}

impl ClientKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ClientKind::Browser => "browser",
            ClientKind::Desktop => "desktop",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "browser" => Some(ClientKind::Browser),
            "desktop" => Some(ClientKind::Desktop),
            _ => None,
        }
    }
}

/// How long a Session lives before the person signs in again.
pub const SESSION_LIFETIME_MS: i64 = 30 * 24 * 60 * 60 * 1_000;

/// What a signed-in client holds. The record never holds the cookie
/// value, only the SHA-256 of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
    pub user_id: UserId,
    pub token_hash: String,
    pub client_kind: ClientKind,
    /// The machine name a Client App reported when it signed in. A
    /// browser Session has none.
    pub client_name: Option<String>,
    pub created_at: UnixMillis,
    pub last_used_at: UnixMillis,
    pub expires_at: UnixMillis,
}

/// How long a one-time sign-in link stays usable.
pub const SIGN_IN_LINK_LIFETIME_MS: i64 = 60 * 1_000;

/// A one-time sign-in link, for a browser a person opens by hand on a
/// local installation. It is good for one minute and one use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInLink {
    pub id: SignInLinkId,
    pub user_id: UserId,
    pub token_hash: String,
    pub created_at: UnixMillis,
    pub expires_at: UnixMillis,
    pub used_at: Option<UnixMillis>,
}

#[async_trait]
pub trait OrgStore: Send + Sync {
    /// Write the Org and, in the same transaction, the Workspace that
    /// holds its records, with no person as its owner.
    async fn create(&self, org: &Org) -> Result<(), StoreError>;
    /// Every Org, oldest first. There is exactly one.
    async fn list(&self) -> Result<Vec<Org>, StoreError>;
    /// Record the installation's Google Web OAuth client id, or clear
    /// it with `None`. Returns false when the Org is missing.
    async fn set_google_client_id(
        &self,
        id: &OrgId,
        client_id: Option<&str>,
    ) -> Result<bool, StoreError>;
}

#[async_trait]
pub trait UserStore: Send + Sync {
    async fn create(&self, user: &User) -> Result<(), StoreError>;
    async fn get(&self, id: &UserId) -> Result<Option<User>, StoreError>;
    /// The person with this address, whatever its case.
    async fn find_by_email(&self, email: &str) -> Result<Option<User>, StoreError>;
    /// The people of one Org, oldest first.
    async fn list_by_org(&self, org_id: &OrgId) -> Result<Vec<User>, StoreError>;
    /// Record what the agents call the person.
    async fn set_name(&self, id: &UserId, name: &str, at: UnixMillis) -> Result<(), StoreError>;
    /// Give the Person the address and the password they sign in with.
    /// An Administrator writes both at once, because an
    /// address without a password and a password without an address are
    /// each half a way in. `false` when the Person is missing, and a
    /// [`StoreError::Conflict`] when another Person holds the address.
    async fn set_email_and_password(
        &self,
        id: &UserId,
        email: &str,
        password_hash: &str,
        at: UnixMillis,
    ) -> Result<bool, StoreError>;
    /// Claim the installation's first Administrator: give this
    /// Administrator the address and the password they sign in with,
    /// but only while they hold no password and no Administrator of
    /// their Org holds one. The check and the write are one
    /// transaction, so of two Server Setups at once, exactly one claim
    /// wins. `false` when the claim loses, or when the Person is missing
    /// or is not an Administrator. A [`StoreError::Conflict`] when
    /// another Person holds the address.
    async fn claim_first_administrator(
        &self,
        id: &UserId,
        email: &str,
        password_hash: &str,
        at: UnixMillis,
    ) -> Result<bool, StoreError>;
    /// Replace the password hash, or take the password away with
    /// `None`. An Administrator resets a password through this.
    /// `false` when the Person is missing.
    async fn set_password_hash(
        &self,
        id: &UserId,
        hash: Option<&str>,
        at: UnixMillis,
    ) -> Result<bool, StoreError>;
    /// Disable the account, or give it back with `None`. `false` when
    /// the Person is missing.
    async fn set_disabled(
        &self,
        id: &UserId,
        disabled_at: Option<UnixMillis>,
        at: UnixMillis,
    ) -> Result<bool, StoreError>;
    /// Set or clear the Person's monthly Spend Cap. `false` when the
    /// Person is missing.
    async fn set_monthly_spend_cap(
        &self,
        id: &UserId,
        cap_usd: Option<f64>,
        at: UnixMillis,
    ) -> Result<bool, StoreError>;
    /// Remember that a client signed in as this Person.
    async fn record_sign_in(&self, id: &UserId, at: UnixMillis) -> Result<(), StoreError>;
}

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn create(&self, session: &Session) -> Result<(), StoreError>;
    /// The Session this cookie hash names, if it has not expired.
    async fn find_live(
        &self,
        token_hash: &str,
        now: UnixMillis,
    ) -> Result<Option<Session>, StoreError>;
    /// The Session with this id, if it has not expired and nobody ended
    /// it. A pending Google authorization names the Session that started
    /// it and reads it again when Google sends the browser back.
    async fn find_live_by_id(
        &self,
        id: &SessionId,
        now: UnixMillis,
    ) -> Result<Option<Session>, StoreError>;
    /// Remember that the Session was used. The daemon does this rarely,
    /// so one request is not one write.
    async fn touch(&self, id: &SessionId, at: UnixMillis) -> Result<(), StoreError>;
    /// Every Session that has not expired, newest first. The
    /// Administration Interface reads it to say who is signed in, from
    /// which kind of client, and since when. It holds every Person of
    /// the installation, because one installation is one Org.
    async fn list_live(&self, now: UnixMillis) -> Result<Vec<Session>, StoreError>;
    /// Sign out. `false` when the Session was already gone.
    async fn delete(&self, id: &SessionId) -> Result<bool, StoreError>;
    /// End every Session of one Person, and answer how many ended.
    /// An Administrator who disables an account or resets a
    /// password ends the Sessions with it: a reset that left a stolen
    /// Session alive would reset nothing.
    async fn delete_for_user(&self, user_id: &UserId) -> Result<u64, StoreError>;
    /// Drop the Sessions that expired. The daemon sweeps once at boot.
    async fn delete_expired(&self, now: UnixMillis) -> Result<u64, StoreError>;
}

#[async_trait]
pub trait SignInLinkStore: Send + Sync {
    async fn create(&self, link: &SignInLink) -> Result<(), StoreError>;
    /// Spend the link. It answers the person once, and never again.
    async fn consume(
        &self,
        token_hash: &str,
        now: UnixMillis,
    ) -> Result<Option<UserId>, StoreError>;
}
