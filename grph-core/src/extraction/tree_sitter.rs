use crate::errors::{GrphError, Result};
use crate::types::{Edge, EdgeKind, Language, Node, NodeKind};
use sha2::Digest;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct ExtractionResult {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub errors: Vec<String>,
}

/// Generate a deterministic node ID from file path and position
pub fn generate_node_id(file_path: &str, kind: &str, name: &str, start_line: u32) -> String {
    let hash = format!("{}:{}:{}:{}", file_path, kind, name, start_line);
    let mut hasher = sha2::Sha256::new();
    hasher.update(hash.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..8])
}

/// Generate a unique ID (for cases where uniqueness is needed)
pub fn generate_unique_id() -> String {
    uuid::Uuid::new_v4().to_string()[..8].to_string()
}

/// Extract text from a source string at given byte offsets
/// Note: This is a simplified version. Full tree-sitter integration
/// would use the tree-sitter crate's node API directly.
pub fn extract_text(source: &[u8], byte_start: usize, byte_end: usize) -> Result<&str> {
    std::str::from_utf8(&source[byte_start..byte_end])
        .map_err(|e| GrphError::Encoding(format!("Invalid UTF-8: {}", e)))
}

/// Get current timestamp in milliseconds
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Parse a simple identifier name from source text
pub fn extract_identifier(text: &str) -> String {
    text.trim()
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '$')
        .next()
        .unwrap_or("")
        .to_string()
}

/// Build a qualified name from file path and local name
pub fn build_qualified_name(file_path: &str, name: &str) -> String {
    let path = file_path.replace('\\', "/");
    if let Some(idx) = path.rfind('/') {
        let dir = &path[..idx];
        let file = path.split('/').last().unwrap_or("");
        format!("{}/{}#{}", dir, file, name)
    } else {
        format!("{}#{}", path, name)
    }
}

/// Create a basic function node from extracted info
pub fn make_function_node(
    file_path: &str,
    language: Language,
    name: &str,
    start_line: u32,
    end_line: u32,
    start_column: u32,
    end_column: u32,
    signature: Option<String>,
) -> Node {
    Node {
        id: generate_node_id(file_path, "function", name, start_line),
        kind: NodeKind::Function,
        name: name.to_string(),
        qualified_name: build_qualified_name(file_path, name),
        file_path: file_path.to_string(),
        language,
        start_line,
        end_line,
        start_column,
        end_column,
        docstring: None,
        signature,
        visibility: None,
        is_exported: false,
        is_async: false,
        is_static: false,
        is_abstract: false,
        decorators: None,
        type_parameters: None,
        updated_at: now_ms(),
    }
}

/// Create a basic class/struct node
pub fn make_class_node(
    file_path: &str,
    language: Language,
    name: &str,
    start_line: u32,
    end_line: u32,
) -> Node {
    Node {
        id: generate_node_id(file_path, "class", name, start_line),
        kind: match language {
            Language::Rust | Language::Go | Language::C => NodeKind::Struct,
            Language::Cpp => NodeKind::Class,
            _ => NodeKind::Class,
        },
        name: name.to_string(),
        qualified_name: build_qualified_name(file_path, name),
        file_path: file_path.to_string(),
        language,
        start_line,
        end_line,
        start_column: 0,
        end_column: 0,
        docstring: None,
        signature: None,
        visibility: None,
        is_exported: false,
        is_async: false,
        is_static: false,
        is_abstract: false,
        decorators: None,
        type_parameters: None,
        updated_at: now_ms(),
    }
}

/// Create an import edge
pub fn make_import_edge(source_id: &str, target_name: &str, line: u32, col: u32) -> Edge {
    Edge {
        source: source_id.to_string(),
        target: target_name.to_string(),
        kind: EdgeKind::Imports,
        metadata: None,
        line: Some(line),
        col: Some(col),
        provenance: Some("tree-sitter".to_string()),
    }
}

/// Create a call edge
pub fn make_call_edge(source_id: &str, target_name: &str, line: u32, col: u32) -> Edge {
    Edge {
        source: source_id.to_string(),
        target: target_name.to_string(),
        kind: EdgeKind::Calls,
        metadata: None,
        line: Some(line),
        col: Some(col),
        provenance: Some("tree-sitter".to_string()),
    }
}
