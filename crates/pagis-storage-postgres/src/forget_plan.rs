//! The exact set of source items, Subject Pages and Schedules that one
//! Forget reaches.

use crate::{db_err, forget_store::encode};
use pagis_core::{
    ConnectionId, ForgetPreview, ForgetTarget, MemoryExposure, StoreError, WorkspaceId,
    knowledge::SourceKey,
};
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, Row};

#[derive(Deserialize, Serialize)]
pub(crate) struct Plan {
    pub connection: ConnectionId,
    pub account: bool,
    pub sources: Vec<(SourceKey, String)>,
    pub schedules: Vec<String>,
    /// The Subject Pages of the forgotten sources, in order. The purge
    /// deletes the structured rows that name one of them.
    pub pages: Vec<String>,
    source_records: Vec<String>,
    suppression_epoch: Vec<String>,
}

impl Plan {
    pub fn preview(&self) -> Result<ForgetPreview, StoreError> {
        use sha2::{Digest, Sha256};
        Ok(ForgetPreview {
            revision: hex::encode(Sha256::digest(encode(self)?.as_bytes())),
            source_items: self.sources.len() as u64,
            raw_account_retrieval_blocked: true,
            ..Default::default()
        })
    }

    pub async fn affects(
        &self,
        tx: &mut PgConnection,
        exposures: &[MemoryExposure],
    ) -> Result<bool, StoreError> {
        if !self.account || exposures.is_empty() {
            return Ok(false);
        }
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jsonb_array_elements(($1)::jsonb) AS x(value) WHERE NOT EXISTS(SELECT 1 FROM grants g WHERE g.id=x.value ->> 'grant_id') OR EXISTS(SELECT 1 FROM grants g WHERE g.id=x.value ->> 'grant_id' AND g.resource_kind='connection' AND g.resource_id=$2))")
            .bind(encode(&exposures)?)
            .bind(self.connection.as_str())
            .fetch_one(tx)
            .await
            .map_err(db_err)
    }
}

pub(crate) async fn load(
    tx: &mut PgConnection,
    workspace: &WorkspaceId,
    target: &ForgetTarget,
) -> Result<Plan, StoreError> {
    let (connection, account, selected) = match target {
        ForgetTarget::Source { source, source_id } => {
            if &source.workspace_id != workspace {
                return Err(StoreError::Conflict(
                    "target is outside this workspace".into(),
                ));
            }
            (
                source.connection_id.clone(),
                false,
                Some((source.clone(), source_id.clone())),
            )
        }
        ForgetTarget::Account { connection_id } => (connection_id.clone(), true, None),
    };
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM connections WHERE workspace_id=$1 AND id=$2)",
    )
    .bind(workspace.as_str())
    .bind(connection.as_str())
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err)?;
    if !exists {
        return Err(StoreError::Conflict("account is unavailable".into()));
    }
    let rows = sqlx::query(
        "SELECT resource,id,record FROM source_items WHERE workspace_id=$1 AND connection_id=$2 ORDER BY resource,id",
    )
    .bind(workspace.as_str())
    .bind(connection.as_str())
    .fetch_all(&mut *tx)
    .await
    .map_err(db_err)?;
    let sources = if let Some((source, id)) = selected {
        if !rows.iter().any(|row| {
            row.get::<&str, _>("resource") == source.resource && row.get::<&str, _>("id") == id
        }) {
            return Err(StoreError::Conflict("source item is unavailable".into()));
        }
        vec![(source, id)]
    } else {
        rows.iter()
            .map(|row| {
                (
                    SourceKey {
                        workspace_id: workspace.clone(),
                        connection_id: connection.clone(),
                        resource: row.get("resource"),
                    },
                    row.get("id"),
                )
            })
            .collect()
    };
    let paths = rows
        .iter()
        .filter(|row| {
            sources.iter().any(|(source, id)| {
                source.resource == row.get::<&str, _>("resource") && id == row.get::<&str, _>("id")
            })
        })
        .filter_map(|row| {
            let record: serde_json::Value = serde_json::from_str(row.get("record")).ok()?;
            let subject = record["parent"].as_str().unwrap_or(row.get("id"));
            Some(format!(
                "private/subjects/{}/{}.md",
                page_segment(row.get("resource")),
                page_segment(subject)
            ))
        })
        .collect::<std::collections::BTreeSet<_>>();
    let schedules = sqlx::query(
        "SELECT s.id,p.path FROM schedules s JOIN schedule_subject_pages p ON p.schedule_id=s.id \
         WHERE s.workspace_id=$1 AND s.forgotten=false",
    )
    .bind(workspace.as_str())
    .fetch_all(&mut *tx)
    .await
    .map_err(db_err)?
    .into_iter()
    .filter(|row| paths.contains(row.get::<&str, _>("path")))
    .map(|row| row.get("id"))
    .collect();
    let source_records = rows
        .iter()
        .filter(|row| {
            sources.iter().any(|(source, id)| {
                source.resource == row.get::<&str, _>("resource") && id == row.get::<&str, _>("id")
            })
        })
        .map(|row| row.get("record"))
        .collect();
    let suppression_epoch = sqlx::query_scalar(
        "SELECT DISTINCT operation_id FROM forget_suppressions WHERE workspace_id=$1 AND connection_id=$2 ORDER BY operation_id",
    )
    .bind(workspace.as_str())
    .bind(connection.as_str())
    .fetch_all(&mut *tx)
    .await
    .map_err(db_err)?;
    Ok(Plan {
        connection,
        account,
        sources,
        schedules,
        pages: paths.into_iter().collect(),
        source_records,
        suppression_epoch,
    })
}

fn page_segment(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
            output.push(char::from(byte));
        } else {
            output.push_str(&format!("_{byte:02x}"));
        }
    }
    if output.is_empty() {
        "item".into()
    } else {
        output
    }
}
