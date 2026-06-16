//! The index records its schema version, so future builds can detect it. Reads refuse a
//! mismatched index; `vaire index` rebuilds it; `vaire status` reports it.

mod common;

use common::Corpus;
use vaire::commands;
use vaire::error::ExitCode;
use vaire::index::db::SCHEMA_VERSION;

#[test]
fn fresh_index_records_current_schema_version() {
    let c = Corpus::fixture();
    assert_eq!(
        commands::status::run(&c.ctx()).unwrap().schema_version,
        Some(SCHEMA_VERSION)
    );
}

#[test]
fn incompatible_schema_version_blocks_reads_until_rebuilt() {
    let c = Corpus::fixture();
    assert!(commands::resolve::run(&c.ctx(), "person:jane-doe").is_ok());

    // Simulate an index written by a different vaire version.
    {
        let conn = rusqlite::Connection::open(c.repo().index_db()).unwrap();
        conn.execute("UPDATE schema_version SET version = 999", [])
            .unwrap();
    }

    // Read commands refuse the mismatched index (exit 3 — "rebuild with --full").
    let err = commands::resolve::run(&c.ctx(), "person:jane-doe").unwrap_err();
    assert_eq!(err.exit_code(), ExitCode::IndexCorrupt);

    // `status` still reports the version — it's how you detect what you're on, not a gate.
    assert_eq!(
        commands::status::run(&c.ctx()).unwrap().schema_version,
        Some(999)
    );

    // Rebuilding restores the current schema, and reads work again.
    c.build();
    assert_eq!(
        commands::status::run(&c.ctx()).unwrap().schema_version,
        Some(SCHEMA_VERSION)
    );
    assert!(commands::resolve::run(&c.ctx(), "person:jane-doe").is_ok());
}
