use thiserror::Error;

#[derive(Error, Debug)]
pub enum GrphError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Not initialized: run `grph init` first")]
    NotInitialized,

    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("Symbol not found: {0}")]
    SymbolNotFound(String),

    #[error("Extraction error: {0}")]
    Extraction(String),

    #[error("Resolution error: {0}")]
    Resolution(String),

    #[error("Graph error: {0}")]
    Graph(String),

    #[error("Context error: {0}")]
    Context(String),

    #[error("Search error: {0}")]
    Search(String),

    #[error("MCP error: {0}")]
    Mcp(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Encoding error: {0}")]
    Encoding(String),
}

pub type Result<T> = std::result::Result<T, GrphError>;
