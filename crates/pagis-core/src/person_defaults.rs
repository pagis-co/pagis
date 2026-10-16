//! The Org defaults for a new Person.
use crate::{ConnectionId, WorkspaceId, WorkspaceRole};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AddressRule {
    FirstName,
    FirstLast,
    FirstInitialLast,
}
impl AddressRule {
    /// Use the first and last words of the Person's name.
    pub fn local_part(self, name: &str) -> Result<String, &'static str> {
        let parts: Vec<_> = name.split_whitespace().collect();
        let first = parts
            .first()
            .ok_or("Enter a name or an explicit Workspace Mailbox address.")?;
        let clean = |part: &str| {
            part.to_ascii_lowercase()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect::<String>()
        };
        let first = clean(first);
        let last = clean(parts.last().unwrap());
        let value = match self {
            Self::FirstName => first,
            Self::FirstLast | Self::FirstInitialLast if parts.len() < 2 || last.is_empty() => {
                return Err(
                    "Enter a first and last name or an explicit Workspace Mailbox address.",
                );
            }
            Self::FirstLast => format!("{first}.{last}"),
            Self::FirstInitialLast => format!(
                "{}{last}",
                first
                    .chars()
                    .next()
                    .ok_or("Enter an explicit Workspace Mailbox address.")?
            ),
        };
        if value.is_empty()
            || value.len() > 64
            || !value.starts_with(|c: char| c.is_ascii_alphanumeric())
            || !value.ends_with(|c: char| c.is_ascii_alphanumeric())
        {
            return Err("Enter an explicit Workspace Mailbox address.");
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DefaultSharedWorkspace {
    #[schema(value_type = String)]
    pub workspace_id: WorkspaceId,
    pub role: WorkspaceRole,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PersonDefaults {
    pub address_rule: Option<AddressRule>,
    #[schema(value_type = Option<String>)]
    pub mailbox_connection_id: Option<ConnectionId>,
    pub shared_workspaces: Vec<DefaultSharedWorkspace>,
}
