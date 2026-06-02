use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    File,
    Module,
    Class,
    Struct,
    Interface,
    Trait,
    Protocol,
    Function,
    Method,
    Property,
    Field,
    Variable,
    Constant,
    Enum,
    EnumMember,
    TypeAlias,
    Namespace,
    Parameter,
    Import,
    Export,
    Component,
}

impl NodeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeKind::File => "file",
            NodeKind::Module => "module",
            NodeKind::Class => "class",
            NodeKind::Struct => "struct",
            NodeKind::Interface => "interface",
            NodeKind::Trait => "trait",
            NodeKind::Protocol => "protocol",
            NodeKind::Function => "function",
            NodeKind::Method => "method",
            NodeKind::Property => "property",
            NodeKind::Field => "field",
            NodeKind::Variable => "variable",
            NodeKind::Constant => "constant",
            NodeKind::Enum => "enum",
            NodeKind::EnumMember => "enum_member",
            NodeKind::TypeAlias => "type_alias",
            NodeKind::Namespace => "namespace",
            NodeKind::Parameter => "parameter",
            NodeKind::Import => "import",
            NodeKind::Export => "export",
            NodeKind::Component => "component",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "file" => Some(NodeKind::File),
            "module" => Some(NodeKind::Module),
            "class" => Some(NodeKind::Class),
            "struct" => Some(NodeKind::Struct),
            "interface" => Some(NodeKind::Interface),
            "trait" => Some(NodeKind::Trait),
            "protocol" => Some(NodeKind::Protocol),
            "function" => Some(NodeKind::Function),
            "method" => Some(NodeKind::Method),
            "property" => Some(NodeKind::Property),
            "field" => Some(NodeKind::Field),
            "variable" => Some(NodeKind::Variable),
            "constant" => Some(NodeKind::Constant),
            "enum" => Some(NodeKind::Enum),
            "enum_member" => Some(NodeKind::EnumMember),
            "type_alias" => Some(NodeKind::TypeAlias),
            "namespace" => Some(NodeKind::Namespace),
            "parameter" => Some(NodeKind::Parameter),
            "import" => Some(NodeKind::Import),
            "export" => Some(NodeKind::Export),
            "component" => Some(NodeKind::Component),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    Contains,
    Calls,
    Imports,
    Exports,
    Extends,
    Implements,
    References,
    TypeOf,
    Returns,
    Instantiates,
    Overrides,
    Decorates,
}

impl EdgeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeKind::Contains => "contains",
            EdgeKind::Calls => "calls",
            EdgeKind::Imports => "imports",
            EdgeKind::Exports => "exports",
            EdgeKind::Extends => "extends",
            EdgeKind::Implements => "implements",
            EdgeKind::References => "references",
            EdgeKind::TypeOf => "type_of",
            EdgeKind::Returns => "returns",
            EdgeKind::Instantiates => "instantiates",
            EdgeKind::Overrides => "overrides",
            EdgeKind::Decorates => "decorates",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "contains" => Some(EdgeKind::Contains),
            "calls" => Some(EdgeKind::Calls),
            "imports" => Some(EdgeKind::Imports),
            "exports" => Some(EdgeKind::Exports),
            "extends" => Some(EdgeKind::Extends),
            "implements" => Some(EdgeKind::Implements),
            "references" => Some(EdgeKind::References),
            "type_of" => Some(EdgeKind::TypeOf),
            "returns" => Some(EdgeKind::Returns),
            "instantiates" => Some(EdgeKind::Instantiates),
            "overrides" => Some(EdgeKind::Overrides),
            "decorates" => Some(EdgeKind::Decorates),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    C,
    Cpp,
    JavaScript,
    TypeScript,
    Tsx,
    Jsx,
    Python,
    Go,
    Rust,
    Shell,
    Esqlc,
}

impl Language {
    pub fn as_str(&self) -> &'static str {
        match self {
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
            Language::Tsx => "tsx",
            Language::Jsx => "jsx",
            Language::Python => "python",
            Language::Go => "go",
            Language::Rust => "rust",
            Language::Shell => "shell",
            Language::Esqlc => "esqlc",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "c" => Some(Language::C),
            "cpp" => Some(Language::Cpp),
            "javascript" => Some(Language::JavaScript),
            "typescript" => Some(Language::TypeScript),
            "tsx" => Some(Language::Tsx),
            "jsx" => Some(Language::Jsx),
            "python" => Some(Language::Python),
            "go" => Some(Language::Go),
            "rust" => Some(Language::Rust),
            "shell" | "bash" | "sh" => Some(Language::Shell),
            "esqlc" => Some(Language::Esqlc),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub language: Language,
    pub start_line: u32,
    pub end_line: u32,
    pub start_column: u32,
    pub end_column: u32,
    pub docstring: Option<String>,
    pub signature: Option<String>,
    pub visibility: Option<String>,
    pub is_exported: bool,
    pub is_async: bool,
    pub is_static: bool,
    pub is_abstract: bool,
    pub decorators: Option<Vec<String>>,
    pub type_parameters: Option<Vec<String>>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub source: String,
    pub target: String,
    pub kind: EdgeKind,
    pub metadata: Option<serde_json::Value>,
    pub line: Option<u32>,
    pub col: Option<u32>,
    pub provenance: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: String,
    pub content_hash: String,
    pub language: Language,
    pub size: u64,
    pub modified_at: i64,
    pub indexed_at: i64,
    pub node_count: u32,
    pub errors: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexProgress {
    pub current: u64,
    pub total: u64,
    pub phase: String,
    pub current_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexResult {
    pub files_indexed: u64,
    pub nodes_created: u64,
    pub edges_created: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResult {
    pub files_changed: u64,
    pub files_added: u64,
    pub files_deleted: u64,
    pub nodes_created: u64,
    pub edges_created: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnresolvedRef {
    pub id: Option<i64>,
    pub from_node_id: String,
    pub reference_name: String,
    pub reference_kind: String,
    pub line: u32,
    pub col: u32,
    pub candidates: Option<Vec<String>>,
    pub file_path: String,
    pub language: String,
}

pub struct GraphStats {
    pub nodes_by_kind: serde_json::Value,
    pub nodes_by_language: serde_json::Value,
    pub edges_by_kind: serde_json::Value,
    pub total_files: u64,
    pub total_nodes: u64,
    pub total_edges: u64,
}

impl FromStr for NodeKind {
    type Err = ();
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "file" => Ok(NodeKind::File),
            "module" => Ok(NodeKind::Module),
            "class" => Ok(NodeKind::Class),
            "struct" => Ok(NodeKind::Struct),
            "interface" => Ok(NodeKind::Interface),
            "trait" => Ok(NodeKind::Trait),
            "protocol" => Ok(NodeKind::Protocol),
            "function" => Ok(NodeKind::Function),
            "method" => Ok(NodeKind::Method),
            "property" => Ok(NodeKind::Property),
            "field" => Ok(NodeKind::Field),
            "variable" => Ok(NodeKind::Variable),
            "constant" => Ok(NodeKind::Constant),
            "enum" => Ok(NodeKind::Enum),
            "enum_member" => Ok(NodeKind::EnumMember),
            "type_alias" => Ok(NodeKind::TypeAlias),
            "namespace" => Ok(NodeKind::Namespace),
            "parameter" => Ok(NodeKind::Parameter),
            "import" => Ok(NodeKind::Import),
            "export" => Ok(NodeKind::Export),
            "component" => Ok(NodeKind::Component),
            _ => Err(()),
        }
    }
}

impl FromStr for EdgeKind {
    type Err = ();
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "contains" => Ok(EdgeKind::Contains),
            "calls" => Ok(EdgeKind::Calls),
            "imports" => Ok(EdgeKind::Imports),
            "exports" => Ok(EdgeKind::Exports),
            "extends" => Ok(EdgeKind::Extends),
            "implements" => Ok(EdgeKind::Implements),
            "references" => Ok(EdgeKind::References),
            "type_of" => Ok(EdgeKind::TypeOf),
            "returns" => Ok(EdgeKind::Returns),
            "instantiates" => Ok(EdgeKind::Instantiates),
            "overrides" => Ok(EdgeKind::Overrides),
            "decorates" => Ok(EdgeKind::Decorates),
            _ => Err(()),
        }
    }
}

impl FromStr for Language {
    type Err = ();
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "c" => Ok(Language::C),
            "cpp" | "c++" => Ok(Language::Cpp),
            "javascript" | "js" => Ok(Language::JavaScript),
            "typescript" | "ts" => Ok(Language::TypeScript),
            "tsx" => Ok(Language::Tsx),
            "jsx" => Ok(Language::Jsx),
            "python" | "py" => Ok(Language::Python),
            "go" => Ok(Language::Go),
            "rust" | "rs" => Ok(Language::Rust),
            "shell" | "bash" | "sh" => Ok(Language::Shell),
            "esqlc" => Ok(Language::Esqlc),
            _ => Err(()),
        }
    }
}
