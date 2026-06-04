use crate::db::migrations::run_migrations;
use crate::db::schema::SCHEMA_SQL;
use crate::errors::Result;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

pub struct Database {
    conn: Connection,
    db_path: PathBuf,
}

impl Clone for Database {
    fn clone(&self) -> Self {
        let conn = Connection::open(&self.db_path)
            .unwrap_or_else(|_| Connection::open(":memory:").unwrap());
        configure_connection(&conn).ok();
        Self {
            conn,
            db_path: self.db_path.clone(),
        }
    }
}

impl Database {
    /// Open (or create) the database at `.grph/grph.db`
    pub fn open(project_root: &Path) -> Result<Self> {
        let db_path = project_root.join(".grph").join("grph.db");
        let db_path_parent = db_path.parent().unwrap();
        std::fs::create_dir_all(db_path_parent)?;

        let conn = Connection::open(&db_path)?;
        configure_connection(&conn)?;
        let db = Self { conn, db_path };
        // Existing projects may predate newer schema objects. Run idempotent
        // migrations on open so MCP/CLI tools can use new indexes immediately.
        if db
            .conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='schema_versions'",
                [],
                |r| r.get::<_, String>(0),
            )
            .is_ok()
        {
            run_migrations(&db)?;
        }
        Ok(db)
    }

    /// Open from an explicit path
    pub fn open_at(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        configure_connection(&conn)?;
        Ok(Self {
            conn,
            db_path: path.to_path_buf(),
        })
    }

    /// Initialize schema if first run
    pub fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(SCHEMA_SQL)?;
        run_migrations(self)?;
        Ok(())
    }

    /// Enable WAL mode for concurrent reads
    pub fn enable_wal(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA journal_mode=WAL")?;
        configure_connection(&self.conn)?;
        Ok(())
    }

    /// Get the underlying connection
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Get the database path
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Close and clean up
    pub fn close(self) -> Result<()> {
        let _ = self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
        Ok(())
    }
}

fn configure_connection(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA synchronous=NORMAL;
         PRAGMA busy_timeout=5000;
         PRAGMA cache_size=-2000;
         PRAGMA mmap_size=268435456",
    )?;
    Ok(())
}
