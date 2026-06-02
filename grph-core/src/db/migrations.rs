use crate::db::connection::Database;
use crate::errors::Result;

struct Migration {
    version: i64,
    sql: &'static str,
    description: &'static str,
}

// Add migrations here as the schema evolves
const MIGRATIONS: &[Migration] = &[];

pub fn run_migrations(db: &Database) -> Result<()> {
    let current_version = get_schema_version(db)?;

    for migration in MIGRATIONS.iter().filter(|m| m.version > current_version) {
        db.conn().execute_batch(migration.sql)?;
        set_schema_version(db, migration.version, migration.description)?;
    }

    Ok(())
}

fn get_schema_version(db: &Database) -> Result<i64> {
    let version: i64 = db.conn().query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_versions",
        [],
        |r| r.get(0),
    )?;
    Ok(version)
}

fn set_schema_version(db: &Database, version: i64, description: &str) -> Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    db.conn().execute(
        "INSERT INTO schema_versions (version, applied_at, description) VALUES (?1, ?2, ?3)",
        rusqlite::params![version, timestamp, description],
    )?;
    Ok(())
}
