//! One integration test binary for this crate. Each file under `tests/`
//! is a module here, so the crate links once instead of once per file.
//!
//! The store traits are proven by the shared suite in `pagis-testkit`,
//! which runs every body against both backends. What lives here
//! is what is true of this backend alone: the schema, the pragmas and the
//! FTS5 segments of the full-text indexes.

mod conversation_search;
mod migrations;
mod pragmas;
