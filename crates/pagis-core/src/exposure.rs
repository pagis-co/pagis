//! Whether one stored message may still show its prose (ADR-0004).
//!
//! The daemon stamps a generated message with the source Grants its
//! author held when it wrote the words. The stamp is a statement about
//! the author: these permissions could have influenced this prose. It
//! settles against the author's own live Grants, and revocation makes
//! the row unavailable everywhere it is read.
//!
//! A Grant belongs to one Agent, so no reader ever holds the Grants of
//! another Agent. A stamp read against the reader's Grants therefore
//! hides every word one Agent says to another, which is the opposite
//! of the delegation the Agent channel exists for (ADR-0003).

use crate::{AuthorKind, GrantStore, Message, MessageStore, StoreError};

/// True when the source scope behind this message is still live.
///
/// The user's own words carry no source scope. A generated message
/// with no stamp at all cannot prove its scope, so it stays hidden.
pub async fn message_source_is_live(
    messages: &dyn MessageStore,
    grants: &dyn GrantStore,
    message: &Message,
) -> Result<bool, StoreError> {
    if message.author_kind == AuthorKind::User {
        return Ok(true);
    }
    let Some(exposures) = messages
        .exposures(&message.workspace_id, &message.id)
        .await?
    else {
        return Ok(false);
    };
    if exposures.is_empty() {
        return Ok(true);
    }
    let Some(author) = &message.author_agent_id else {
        return Ok(false);
    };
    let live = grants
        .list_live_for_agent(&message.workspace_id, author)
        .await?;
    Ok(exposures.iter().all(|exposure| {
        live.iter()
            .any(|grant| grant.id == exposure.grant_id && grant.revision == exposure.revision)
    }))
}
