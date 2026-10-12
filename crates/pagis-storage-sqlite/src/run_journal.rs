use crate::db_err;
use async_trait::async_trait;
use pagis_core::{JournalCall, RunCheckpoint, RunId, RunJournal, RunJournalEntry, StoreError};
use serde_json::Value;
use sqlx::{Row, SqlitePool};

#[derive(Clone)]
pub struct SqliteRunJournal {
    pool: SqlitePool,
}
impl SqliteRunJournal {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}
fn decode(text: String) -> Result<Value, StoreError> {
    serde_json::from_str(&text).map_err(|error| StoreError::Corrupt(error.to_string()))
}
#[async_trait]
impl RunJournal for SqliteRunJournal {
    async fn checkpoint(
        &self,
        run_id: &RunId,
        turn: usize,
        state: &Value,
    ) -> Result<(), StoreError> {
        let mut tx = crate::begin_write(&self.pool).await.map_err(db_err)?;
        sqlx::query("INSERT INTO run_journal(run_id,turn,state) VALUES (?,?,?) ON CONFLICT(run_id) DO UPDATE SET turn=excluded.turn,state=excluded.state,pending_turn=NULL,resumed=0")
            .bind(run_id.as_str()).bind(turn as i64).bind(state.to_string())
            .execute(&mut *tx).await.map_err(db_err)?;
        sqlx::query("DELETE FROM run_journal_calls WHERE run_id=?")
            .bind(run_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)
    }
    async fn turn_started(&self, run_id: &RunId, state: &Value) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO run_journal(run_id,pending_turn) VALUES (?,?) ON CONFLICT(run_id) DO UPDATE SET pending_turn=excluded.pending_turn").bind(run_id.as_str()).bind(state.to_string()).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
    async fn tool_started(
        &self,
        run_id: &RunId,
        call_id: &str,
        effect_class: &str,
    ) -> Result<(), StoreError> {
        let mut tx = crate::begin_write(&self.pool).await.map_err(db_err)?;
        sqlx::query("INSERT INTO run_journal(run_id) VALUES (?) ON CONFLICT DO NOTHING")
            .bind(run_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query("INSERT INTO run_journal_calls(run_id,call_id,effect_class) VALUES (?,?,?)")
            .bind(run_id.as_str())
            .bind(call_id)
            .bind(effect_class)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)
    }
    async fn tool_ended(
        &self,
        run_id: &RunId,
        call_id: &str,
        result: &Value,
    ) -> Result<(), StoreError> {
        let written =
            sqlx::query("UPDATE run_journal_calls SET result=?3 WHERE run_id=?1 AND call_id=?2")
                .bind(run_id.as_str())
                .bind(call_id)
                .bind(result.to_string())
                .execute(&self.pool)
                .await
                .map_err(db_err)?;
        if written.rows_affected() != 1 {
            return Err(StoreError::Corrupt("tool result has no start".into()));
        }
        Ok(())
    }
    async fn last(&self, run_id: &RunId) -> Result<Option<RunJournalEntry>, StoreError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let Some(row) = sqlx::query("SELECT * FROM run_journal WHERE run_id=?")
            .bind(run_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err)?
        else {
            return Ok(None);
        };
        let turn: Option<i64> = row.get("turn");
        let checkpoint = match turn {
            Some(turn) => Some(RunCheckpoint {
                turn: usize::try_from(turn)
                    .map_err(|error| StoreError::Corrupt(error.to_string()))?,
                state: decode(row.get("state"))?,
            }),
            None => None,
        };
        let pending_turn = row
            .get::<Option<String>, _>("pending_turn")
            .map(decode)
            .transpose()?;
        let rows = sqlx::query("SELECT * FROM run_journal_calls WHERE run_id=? ORDER BY call_id")
            .bind(run_id.as_str())
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?;
        let calls = rows
            .into_iter()
            .map(|row| {
                Ok(JournalCall {
                    call_id: row.get("call_id"),
                    effect_class: row.get("effect_class"),
                    result: row
                        .get::<Option<String>, _>("result")
                        .map(decode)
                        .transpose()?,
                })
            })
            .collect::<Result<_, StoreError>>()?;
        tx.commit().await.map_err(db_err)?;
        Ok(Some(RunJournalEntry {
            checkpoint,
            pending_turn,
            calls,
            resumed: row.get::<i64, _>("resumed") != 0,
        }))
    }
    async fn mark_resumed(&self, run_id: &RunId) -> Result<bool, StoreError> {
        Ok(sqlx::query(
            "UPDATE run_journal SET resumed=1 WHERE run_id=? AND resumed=0 AND turn IS NOT NULL",
        )
        .bind(run_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?
        .rows_affected()
            == 1)
    }
    async fn delete(&self, run_id: &RunId) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM run_journal WHERE run_id=?")
            .bind(run_id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
