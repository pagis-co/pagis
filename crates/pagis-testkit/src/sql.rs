//! Portable direct SQL for a test.
//!
//! A few tests plant a row no store method writes, or plant one broken
//! on purpose, and a few read a column no trait answers. Those tests run
//! on both backends, so they cannot name a pool type.
//!
//! [`Rows`] holds the pool of whichever backend the test runs on and
//! takes a statement of the portable subset: `?` marks a bind, in order,
//! and the Postgres side rewrites the marks to `$n`.

use sqlx::{PgPool, SqlitePool};

/// One value a test binds to a `?`. The set is what the portable subset
/// of the schema holds: text, a millisecond time or a counter, and a
/// flag that is an integer on one backend and a boolean on the other.
#[derive(Debug, Clone)]
pub enum Bind {
    Text(String),
    Int(i64),
    Bool(bool),
    /// A column with no value. It binds a text NULL, which every
    /// nullable column of the schema accepts.
    Null,
}

impl From<&str> for Bind {
    fn from(value: &str) -> Self {
        Bind::Text(value.to_string())
    }
}

impl From<&String> for Bind {
    fn from(value: &String) -> Self {
        Bind::Text(value.clone())
    }
}

impl From<String> for Bind {
    fn from(value: String) -> Self {
        Bind::Text(value)
    }
}

impl From<i64> for Bind {
    fn from(value: i64) -> Self {
        Bind::Int(value)
    }
}

impl From<bool> for Bind {
    fn from(value: bool) -> Self {
        Bind::Bool(value)
    }
}

/// The rows of one test database, whichever backend holds them.
#[derive(Clone)]
pub enum Rows {
    Sqlite(SqlitePool),
    Postgres(PgPool),
}

impl Rows {
    /// Run one statement and answer how many rows it changed.
    pub async fn execute(&self, sql: &str, binds: &[Bind]) -> Result<u64, String> {
        match self {
            Rows::Sqlite(pool) => {
                let mut query = sqlx::query(sql);
                for bind in binds {
                    query = bind_sqlite(query, bind);
                }
                query
                    .execute(pool)
                    .await
                    .map(|done| done.rows_affected())
                    .map_err(|error| error.to_string())
            }
            Rows::Postgres(pool) => {
                let numbered = number_placeholders(sql);
                let mut query = sqlx::query(&numbered);
                for bind in binds {
                    query = bind_postgres(query, bind);
                }
                query
                    .execute(pool)
                    .await
                    .map(|done| done.rows_affected())
                    .map_err(|error| error.to_string())
            }
        }
    }

    /// One integer, for a test that counts rows or reads a counter.
    pub async fn count(&self, sql: &str, binds: &[Bind]) -> Result<i64, String> {
        match self {
            Rows::Sqlite(pool) => {
                let mut query = sqlx::query_scalar::<_, i64>(sql);
                for bind in binds {
                    query = bind_sqlite_scalar(query, bind);
                }
                query.fetch_one(pool).await.map_err(|e| e.to_string())
            }
            Rows::Postgres(pool) => {
                let numbered = number_placeholders(sql);
                let mut query = sqlx::query_scalar::<_, i64>(&numbered);
                for bind in binds {
                    query = bind_postgres_scalar(query, bind);
                }
                query.fetch_one(pool).await.map_err(|e| e.to_string())
            }
        }
    }

    /// One text column of the first row, or `None` when no row matches.
    pub async fn text(&self, sql: &str, binds: &[Bind]) -> Result<Option<String>, String> {
        match self {
            Rows::Sqlite(pool) => {
                let mut query = sqlx::query_scalar::<_, Option<String>>(sql);
                for bind in binds {
                    query = bind_sqlite_text(query, bind);
                }
                query
                    .fetch_optional(pool)
                    .await
                    .map(Option::flatten)
                    .map_err(|e| e.to_string())
            }
            Rows::Postgres(pool) => {
                let numbered = number_placeholders(sql);
                let mut query = sqlx::query_scalar::<_, Option<String>>(&numbered);
                for bind in binds {
                    query = bind_postgres_text(query, bind);
                }
                query
                    .fetch_optional(pool)
                    .await
                    .map(Option::flatten)
                    .map_err(|e| e.to_string())
            }
        }
    }

    /// Every value that every table of the database holds, as bytes: a
    /// blob as it is, and any other value as its text. A test that reads
    /// a copy of the database the way an attacker does uses it, so no
    /// table and no column is left out, the full-text tables included.
    pub async fn every_value(&self) -> Result<Vec<Vec<u8>>, String> {
        let error = |error: sqlx::Error| error.to_string();
        let mut values = Vec::new();
        match self {
            Rows::Sqlite(pool) => {
                let columns: Vec<(String, String)> = sqlx::query_as(
                    "SELECT t.name, c.name FROM sqlite_schema t, pragma_table_info(t.name) c \
                     WHERE t.type = 'table'",
                )
                .fetch_all(pool)
                .await
                .map_err(error)?;
                for (table, column) in columns {
                    // A cast to BLOB gives the bytes of a blob and the
                    // text of every other value.
                    let sql = format!(
                        "SELECT CAST(\"{column}\" AS BLOB) FROM \"{table}\" \
                         WHERE \"{column}\" IS NOT NULL"
                    );
                    let found: Vec<Vec<u8>> = sqlx::query_scalar(&sql)
                        .fetch_all(pool)
                        .await
                        .map_err(error)?;
                    values.extend(found);
                }
            }
            Rows::Postgres(pool) => {
                let columns: Vec<(String, String, String)> = sqlx::query_as(
                    "SELECT table_name::text, column_name::text, data_type::text \
                     FROM information_schema.columns WHERE table_schema = current_schema()",
                )
                .fetch_all(pool)
                .await
                .map_err(error)?;
                for (table, column, data_type) in columns {
                    let sql = if data_type == "bytea" {
                        format!(
                            "SELECT \"{column}\" FROM \"{table}\" WHERE \"{column}\" IS NOT NULL"
                        )
                    } else {
                        format!(
                            "SELECT convert_to(\"{column}\"::text, 'UTF8') FROM \"{table}\" \
                             WHERE \"{column}\" IS NOT NULL"
                        )
                    };
                    let found: Vec<Vec<u8>> = sqlx::query_scalar(&sql)
                        .fetch_all(pool)
                        .await
                        .map_err(error)?;
                    values.extend(found);
                }
            }
        }
        Ok(values)
    }
}

/// `?` becomes `$1`, `$2` and so on, in order. A `?` inside a quoted
/// string is text, not a bind, so the scan tracks the quotes.
fn number_placeholders(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len() + 8);
    let mut next = 1;
    let mut quoted = false;
    for character in sql.chars() {
        if character == '\'' {
            quoted = !quoted;
        }
        if character == '?' && !quoted {
            out.push('$');
            out.push_str(&next.to_string());
            next += 1;
        } else {
            out.push(character);
        }
    }
    out
}

macro_rules! bind_body {
    ($query:ident, $bind:ident) => {
        match $bind {
            Bind::Text(value) => $query.bind(value.clone()),
            Bind::Int(value) => $query.bind(*value),
            Bind::Bool(value) => $query.bind(*value),
            Bind::Null => $query.bind(None::<String>),
        }
    };
}

type SqliteQuery<'q> = sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>;
type PostgresQuery<'q> = sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>;
type SqliteScalar<'q, T> =
    sqlx::query::QueryScalar<'q, sqlx::Sqlite, T, sqlx::sqlite::SqliteArguments<'q>>;
type PostgresScalar<'q, T> =
    sqlx::query::QueryScalar<'q, sqlx::Postgres, T, sqlx::postgres::PgArguments>;

fn bind_sqlite<'q>(query: SqliteQuery<'q>, bind: &Bind) -> SqliteQuery<'q> {
    bind_body!(query, bind)
}

fn bind_postgres<'q>(query: PostgresQuery<'q>, bind: &Bind) -> PostgresQuery<'q> {
    bind_body!(query, bind)
}

fn bind_sqlite_scalar<'q>(query: SqliteScalar<'q, i64>, bind: &Bind) -> SqliteScalar<'q, i64> {
    bind_body!(query, bind)
}

fn bind_postgres_scalar<'q>(
    query: PostgresScalar<'q, i64>,
    bind: &Bind,
) -> PostgresScalar<'q, i64> {
    bind_body!(query, bind)
}

fn bind_sqlite_text<'q>(
    query: SqliteScalar<'q, Option<String>>,
    bind: &Bind,
) -> SqliteScalar<'q, Option<String>> {
    bind_body!(query, bind)
}

fn bind_postgres_text<'q>(
    query: PostgresScalar<'q, Option<String>>,
    bind: &Bind,
) -> PostgresScalar<'q, Option<String>> {
    bind_body!(query, bind)
}
