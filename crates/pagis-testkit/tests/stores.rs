//! The store-trait suite, on both backends.
//!
//! One line: `store_suite!` writes a SQLite test and a Postgres test for
//! every body of `pagis_testkit::store_suite::bodies`. There is no list
//! to keep here, so a body cannot reach one backend only.
//!
//! The Postgres tests need the Docker daemon. Without it they skip with
//! a message that says so.

pagis_testkit::store_suite!();
