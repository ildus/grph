use crate::convert;
use grph_core::{Edge, Grph, Node};
use lsp_types::{CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

pub struct LspHandlers {
    root: PathBuf,
    grph: Grph,
    buffers: HashMap<String, String>,
}

const DEFAULT_CALLERS_LIMIT: u32 = 1000;

impl LspHandlers {
    pub fn new(root: PathBuf) -> grph_core::Result<Self> {
        Ok(Self {
            grph: Grph::open(&root)?,
            root,
            buffers: HashMap::new(),
        })
    }

    pub fn initialize_result(&self) -> Value {
        json!({
            "capabilities": {
                "textDocumentSync": {"openClose": true, "change": 2, "save": true},
                "documentSymbolProvider": true,
                "definitionProvider": true,
                "referencesProvider": true,
                "hoverProvider": true,
                "workspaceSymbolProvider": true,
                "callHierarchyProvider": true
            },
            "serverInfo": {"name": "grph-lsp", "version": env!("CARGO_PKG_VERSION")}
        })
    }

    pub fn document_symbol(&self, params: &Value) -> grph_core::Result<Value> {
        let Some(file_path) = self.text_document_file_path(params) else {
            return Ok(Value::Null);
        };
        let nodes = self.grph.db().list_nodes_by_file(&file_path)?;
        let symbols = convert::nodes_to_document_symbols(&nodes);
        Ok(serde_json::to_value(symbols).unwrap_or(Value::Null))
    }

    pub fn definition(&self, params: &Value) -> grph_core::Result<Value> {
        let node = self.resolve_node_at_params(params)?;
        Ok(node
            .as_ref()
            .and_then(|node| convert::node_location(&self.root, node))
            .and_then(|loc| serde_json::to_value(loc).ok())
            .unwrap_or(Value::Null))
    }

    pub fn references(&self, params: &Value) -> grph_core::Result<Value> {
        let Some(node) = self.resolve_node_at_params(params)? else {
            return Ok(json!([]));
        };
        let mut locations = Vec::new();
        if params
            .pointer("/context/includeDeclaration")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            if let Some(location) = convert::node_location(&self.root, &node) {
                locations.push(location);
            }
        }
        for (related, edge) in self
            .grph
            .traverser()
            .callers(&node.id, DEFAULT_CALLERS_LIMIT)?
        {
            if let Some(location) = self.edge_location(&related, &edge) {
                locations.push(location);
            }
        }
        for (related, edge) in self
            .grph
            .traverser()
            .references_to(&node.id, DEFAULT_CALLERS_LIMIT)?
        {
            if let Some(location) = self.edge_location(&related, &edge) {
                locations.push(location);
            }
        }
        for (related, edge) in self
            .grph
            .traverser()
            .callees(&node.id, DEFAULT_CALLERS_LIMIT)?
        {
            if let Some(location) = self
                .edge_location(&node, &edge)
                .or_else(|| convert::node_location(&self.root, &related))
            {
                locations.push(location);
            }
        }
        Ok(serde_json::to_value(locations).unwrap_or_else(|_| json!([])))
    }

    pub fn hover(&self, params: &Value) -> grph_core::Result<Value> {
        let Some(node) = self.resolve_node_at_params(params)? else {
            return Ok(Value::Null);
        };
        let mut parts = Vec::new();
        if let Some(signature) = node.signature.as_ref().filter(|s| !s.trim().is_empty()) {
            parts.push(format!(
                "```{}\n{}\n```",
                node.language.as_str(),
                signature.trim()
            ));
        }
        if let Some(docstring) = node.docstring.as_ref().filter(|s| !s.trim().is_empty()) {
            parts.push(docstring.trim().to_string());
        }
        if parts.is_empty() {
            parts.push(format!("`{}` {}", node.kind.as_str(), node.name));
        }
        Ok(
            json!({"contents": {"kind": "markdown", "value": parts.join("\n\n")}, "range": convert::node_range(&node)}),
        )
    }

    pub fn workspace_symbol(&self, params: &Value) -> grph_core::Result<Value> {
        let query = params
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.trim().is_empty() {
            return Ok(json!([]));
        }

        // Hybrid workspace search: keep fast symbol-name matches first, then
        // enrich with symbols from files whose indexed source content matches
        // the query. This makes editor symbol search useful for product/error
        // strings and comments, not only identifiers.
        let mut nodes = Vec::<Node>::new();
        let mut seen = std::collections::HashSet::<String>::new();

        for node in self.grph.search(query, None, 50)? {
            if seen.insert(node.id.clone()) {
                nodes.push(node);
            }
        }

        if nodes.len() < 50 {
            for (file_path, _score) in self.grph.db().search_file_contents(query, 12)? {
                let mut file_nodes = self.grph.db().list_nodes_by_file(&file_path)?;
                file_nodes.retain(|node| workspace_symbol_value(node.kind));
                file_nodes.sort_by_key(|node| (workspace_symbol_rank(node.kind), node.start_line));
                for node in file_nodes.into_iter().take(6) {
                    if seen.insert(node.id.clone()) {
                        nodes.push(node);
                        if nodes.len() >= 50 {
                            break;
                        }
                    }
                }
                if nodes.len() >= 50 {
                    break;
                }
            }
        }

        let symbols: Vec<_> = nodes
            .iter()
            .filter_map(|node| convert::node_to_symbol_information(&self.root, node))
            .collect();
        Ok(serde_json::to_value(symbols).unwrap_or_else(|_| json!([])))
    }

    pub fn prepare_call_hierarchy(&self, params: &Value) -> grph_core::Result<Value> {
        let Some(node) = self.resolve_node_at_params(params)? else {
            return Ok(Value::Null);
        };
        Ok(serde_json::to_value(vec![self.call_item(&node)]).unwrap_or(Value::Null))
    }

    pub fn incoming_calls(&self, params: &Value) -> grph_core::Result<Value> {
        let Some(node) = self.node_from_call_item(params)? else {
            return Ok(json!([]));
        };
        let calls: Vec<CallHierarchyIncomingCall> = self
            .grph
            .traverser()
            .callers(&node.id, DEFAULT_CALLERS_LIMIT)?
            .iter()
            .map(|(caller, edge)| CallHierarchyIncomingCall {
                from: self.call_item(caller),
                from_ranges: vec![self
                    .edge_range(edge)
                    .unwrap_or_else(|| convert::node_range(caller))],
            })
            .collect();
        Ok(serde_json::to_value(calls).unwrap_or_else(|_| json!([])))
    }

    pub fn outgoing_calls(&self, params: &Value) -> grph_core::Result<Value> {
        let Some(node) = self.node_from_call_item(params)? else {
            return Ok(json!([]));
        };
        let calls: Vec<CallHierarchyOutgoingCall> = self
            .grph
            .traverser()
            .callees(&node.id, DEFAULT_CALLERS_LIMIT)?
            .iter()
            .map(|(callee, edge)| CallHierarchyOutgoingCall {
                to: self.call_item(callee),
                from_ranges: vec![self
                    .edge_range(edge)
                    .unwrap_or_else(|| convert::node_range(&node))],
            })
            .collect();
        Ok(serde_json::to_value(calls).unwrap_or_else(|_| json!([])))
    }

    pub fn did_open(&mut self, params: &Value) {
        if let (Some(uri), Some(text)) = (
            params.pointer("/textDocument/uri").and_then(Value::as_str),
            params.pointer("/textDocument/text").and_then(Value::as_str),
        ) {
            self.buffers.insert(uri.to_string(), text.to_string());
        }
    }

    pub fn did_change(&mut self, params: &Value) {
        let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) else {
            return;
        };
        let Some(changes) = params.get("contentChanges").and_then(Value::as_array) else {
            return;
        };
        if let Some(text) = changes
            .last()
            .and_then(|change| change.get("text"))
            .and_then(Value::as_str)
        {
            self.buffers.insert(uri.to_string(), text.to_string());
        }
    }

    pub fn did_close(&mut self, params: &Value) {
        if let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) {
            self.buffers.remove(uri);
        }
    }

    pub fn did_save(&mut self, params: &Value) -> grph_core::Result<()> {
        let Some(file_path) = self.text_document_file_path(params) else {
            return Ok(());
        };
        self.grph.sync_file(std::path::Path::new(&file_path))?;
        Ok(())
    }

    fn text_document_file_path(&self, params: &Value) -> Option<String> {
        params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .and_then(|uri| convert::uri_to_file_path(&self.root, uri))
    }

    /// Resolve the node the cursor is pointing at.
    ///
    /// The cursor may sit on a call-site (`IIapi_setDescriptor(...)`) or
    /// reference-site inside a function body. Tree-sitter records those as
    /// *edges* rather than *nodes*, so a pure position-based lookup would
    /// return the enclosing function.  To handle this case we first extract
    /// the identifier under the cursor and try to look it up by name; only
    /// when that fails do we fall back to the position-based containment
    /// query.
    fn resolve_node_at_params(&self, params: &Value) -> grph_core::Result<Option<Node>> {
        Ok(self
            .definition_node_at_params(params)?
            .or(self.node_at_params(params)?))
    }

    fn node_at_params(&self, params: &Value) -> grph_core::Result<Option<Node>> {
        let Some(file_path) = self.text_document_file_path(params) else {
            return Ok(None);
        };
        let line = params
            .pointer("/position/line")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32
            + 1;
        let column = params
            .pointer("/position/character")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        self.grph
            .db()
            .get_node_at_position(&file_path, line, column)
    }

    fn definition_node_at_params(&self, params: &Value) -> grph_core::Result<Option<Node>> {
        let Some(file_path) = self.text_document_file_path(params) else {
            return Ok(None);
        };
        let Some(symbol) = self.symbol_at_params(params, &file_path) else {
            return Ok(None);
        };

        if let Some(node) = self.grph.db().get_node_by_name(&symbol, &file_path)? {
            return Ok(Some(node));
        }

        // Match CLI symbol resolution for references/call hierarchy. A bare
        // name can have many nodes in portable C codebases (platform-specific
        // implementations, header macros/prototypes, etc.). The raw
        // get_node_by_name_any() ordering is intentionally minimal and can pick
        // a declaration-like/header node with no callers. grph search applies
        // the user-facing symbol ranking used by grph callers, so LSP actions
        // on call-sites resolve to the same implementation the CLI reports.
        Ok(self.grph.search(&symbol, None, 1)?.into_iter().next())
    }

    fn symbol_at_params(&self, params: &Value, file_path: &str) -> Option<String> {
        let line = params.pointer("/position/line").and_then(Value::as_u64)? as usize;
        let character = params
            .pointer("/position/character")
            .and_then(Value::as_u64)? as usize;
        let content = self.document_content(params, file_path)?;
        let source_line = content.lines().nth(line)?;
        identifier_at(source_line, character)
    }

    fn document_content(&self, params: &Value, file_path: &str) -> Option<String> {
        if let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) {
            if let Some(content) = self.buffers.get(uri) {
                return Some(content.clone());
            }
        }
        let path = if std::path::Path::new(file_path).is_absolute() {
            PathBuf::from(file_path)
        } else {
            self.root.join(file_path)
        };
        std::fs::read_to_string(path).ok()
    }

    fn node_from_call_item(&self, params: &Value) -> grph_core::Result<Option<Node>> {
        let Some(data) = params.pointer("/item/data") else {
            return Ok(None);
        };
        let Some(id) = data.get("nodeId").and_then(Value::as_str) else {
            return Ok(None);
        };
        self.grph.db().get_node_by_id(id)
    }

    fn call_item(&self, node: &Node) -> CallHierarchyItem {
        let uri = convert::node_location(&self.root, node)
            .map(|location| location.uri)
            .unwrap_or_else(|| {
                lsp_types::Uri::from_str("file:///").expect("static fallback URI is valid")
            });
        CallHierarchyItem {
            name: node.name.clone(),
            kind: convert::symbol_kind(node.kind),
            tags: None,
            detail: node.signature.clone(),
            uri,
            range: convert::node_range(node),
            selection_range: convert::node_range(node),
            data: Some(json!({"nodeId": node.id})),
        }
    }

    fn edge_location(&self, node: &Node, edge: &Edge) -> Option<lsp_types::Location> {
        Some(lsp_types::Location {
            uri: convert::node_location(&self.root, node)?.uri,
            range: self
                .edge_range(edge)
                .unwrap_or_else(|| convert::node_range(node)),
        })
    }

    fn edge_range(&self, edge: &Edge) -> Option<lsp_types::Range> {
        let line = edge.line?.saturating_sub(1);
        let character = edge.col.unwrap_or(0);
        Some(lsp_types::Range {
            start: lsp_types::Position { line, character },
            end: lsp_types::Position {
                line,
                character: character.saturating_add(1),
            },
        })
    }
}

fn workspace_symbol_value(kind: grph_core::NodeKind) -> bool {
    !matches!(
        kind,
        grph_core::NodeKind::Import | grph_core::NodeKind::Export | grph_core::NodeKind::Parameter
    )
}

fn workspace_symbol_rank(kind: grph_core::NodeKind) -> u8 {
    match kind {
        grph_core::NodeKind::Function | grph_core::NodeKind::Method => 0,
        grph_core::NodeKind::Class
        | grph_core::NodeKind::Struct
        | grph_core::NodeKind::Interface
        | grph_core::NodeKind::Trait
        | grph_core::NodeKind::Protocol
        | grph_core::NodeKind::Component => 1,
        grph_core::NodeKind::Enum | grph_core::NodeKind::TypeAlias => 2,
        grph_core::NodeKind::Module | grph_core::NodeKind::Namespace => 3,
        grph_core::NodeKind::Variable
        | grph_core::NodeKind::Constant
        | grph_core::NodeKind::Property
        | grph_core::NodeKind::Field => 4,
        _ => 5,
    }
}

fn identifier_at(line: &str, character: usize) -> Option<String> {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    if chars.is_empty() {
        return None;
    }

    let mut byte_index = line.len();
    for (char_index, (byte, _)) in chars.iter().enumerate() {
        if char_index >= character {
            byte_index = *byte;
            break;
        }
    }
    if byte_index == line.len() && character < chars.len() {
        byte_index = chars[character].0;
    }

    let mut start = byte_index;
    while start > 0 {
        let Some((prev, ch)) = line[..start].char_indices().next_back() else {
            break;
        };
        if !is_ident_char(ch) {
            break;
        }
        start = prev;
    }

    let mut end = byte_index;
    while end < line.len() {
        let Some(ch) = line[end..].chars().next() else {
            break;
        };
        if !is_ident_char(ch) {
            break;
        }
        end += ch.len_utf8();
    }

    if start == end {
        return None;
    }
    Some(line[start..end].to_string())
}

fn is_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '$'
}

#[cfg(test)]
mod tests {
    use super::*;
    use grph_core::Grph;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_project(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("grph-lsp-{name}-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn workspace_symbol_uses_file_content_fts() {
        let dir = temp_project("workspace-fts");
        fs::write(
            dir.join("billing.py"),
            r#"
def reconcile_account(record):
    # Handles invoice mismatch recovery for imported ledger rows.
    if record.get('status') == 'invoice mismatch':
        return 'needs-review'
    return 'ok'
"#,
        )
        .unwrap();
        let mut grph = Grph::init(&dir).unwrap();
        grph.index(|_| {}).unwrap();

        let handlers = LspHandlers::new(dir.clone()).unwrap();
        let value = handlers
            .workspace_symbol(&json!({"query": "invoice mismatch recovery"}))
            .unwrap();
        let text = serde_json::to_string(&value).unwrap();
        assert!(text.contains("reconcile_account"), "{text}");

        fs::remove_dir_all(dir).ok();
    }
}
