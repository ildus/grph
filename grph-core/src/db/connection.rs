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
        let conn = Connection::open(&self.db_path).unwrap_or_else(|err| {
            panic!(
                "failed to reopen grph database at {}: {err}",
                self.db_path.display()
            )
        });
        configure_connection(&conn).unwrap_or_else(|err| {
            panic!(
                "failed to configure grph database at {}: {err}",
                self.db_path.display()
            )
        });
        Self {
            conn,
            db_path: self.db_path.clone(),
        }
    }
}

impl Database {
    /// Open (or create) the database at `.grph/grph.db`.
    ///
    /// Creates the `.grph` directory and schema on first use. Callers that must
    /// not create a project should check that `grph.db` exists before calling.
    pub fn open(project_root: &Path) -> Result<Self> {
        let db_path = project_root.join(".grph").join("grph.db");
        let db_path_parent = db_path.parent().unwrap();
        std::fs::create_dir_all(db_path_parent)?;

        let conn = Connection::open(&db_path)?;
        configure_connection(&conn)?;
        let db = Self { conn, db_path };
        db.init_schema()?;
        db.enable_wal()?;
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
