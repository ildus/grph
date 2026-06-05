use crate::convert;
use grph_core::{Edge, Grph, Node};
use lsp_types::{CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::str::FromStr;
use tree_sitter::{Language as TsLanguage, Node as TsNode, Parser, Point};

pub struct LspHandlers {
    root: PathBuf,
    grph: Grph,
    buffers: HashMap<String, String>,
    parse_cache: RefCell<HashMap<String, ParsedCacheEntry>>,
}

const DEFAULT_CALLERS_LIMIT: u32 = 1000;

impl LspHandlers {
    pub fn new(root: PathBuf) -> grph_core::Result<Self> {
        Ok(Self {
            grph: Grph::open(&root)?,
            root,
            buffers: HashMap::new(),
            parse_cache: RefCell::new(HashMap::new()),
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
        if let Some(location) = self.definition_location_at_params(params)? {
            return Ok(serde_json::to_value(location).unwrap_or(Value::Null));
        }
        Ok(Value::Null)
    }

    fn definition_location_at_params(
        &self,
        params: &Value,
    ) -> grph_core::Result<Option<lsp_types::Location>> {
        let Some(file_path) = self.text_document_file_path(params) else {
            return Ok(None);
        };
        let Some(symbol) = self.symbol_at_params(params, &file_path) else {
            return Ok(None);
        };

        // Prefer a tree-sitter local declaration for non-call identifiers. This keeps
        // go-to-definition on local C variables such as context from resolving to
        // a same-file/global function with a common name, while still letting
        // call-sites resolve through the indexed graph below.
        if !self.symbol_looks_like_call(params, &file_path, &symbol) {
            if let Some(location) = self.local_symbol_definition(params, &file_path, &symbol) {
                return Ok(Some(location));
            }
        }

        if let Some(node) = self.grph.db().get_node_by_name(&symbol, &file_path)? {
            return Ok(convert::node_location(&self.root, &node));
        }

        // Global lookup is appropriate for call-sites and symbol-like names,
        // but not for plain lowercase local variables. The latter otherwise
        // resolve to arbitrary workspace symbols with the same common name.
        if self.should_global_lookup(params, &file_path, &symbol) {
            if let Some(node) = self.grph.search(&symbol, None, 1)?.into_iter().next() {
                return Ok(convert::node_location(&self.root, &node));
            }
        }

        Ok(self
            .node_at_params(params)?
            .as_ref()
            .and_then(|node| convert::node_location(&self.root, node)))
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
            if let Some(file_path) = convert::uri_to_file_path(&self.root, uri) {
                self.parse_cache.borrow_mut().remove(&file_path);
            }
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
        if let Some(symbol) = self.tree_sitter_identifier_at_params(params, file_path) {
            return Some(symbol);
        }

        let line = params.pointer("/position/line").and_then(Value::as_u64)? as usize;
        let character = params
            .pointer("/position/character")
            .and_then(Value::as_u64)? as usize;
        let content = self.document_content(params, file_path)?;
        let source_line = content.lines().nth(line)?;
        identifier_at(source_line, character)
    }

    fn tree_sitter_identifier_at_params(&self, params: &Value, file_path: &str) -> Option<String> {
        let position = position_from_params(params)?;
        let (content, parsed) = self.parsed_document(params, file_path)?;
        let ident = parsed.identifier_at_point(position)?;
        Some(text(ident, &content).to_string())
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

    fn parsed_document(&self, params: &Value, file_path: &str) -> Option<(String, ParsedDocument)> {
        let content = self.document_content(params, file_path)?;
        let content_hash = stable_hash(&content);
        let cache_key = file_path.to_string();
        if let Some(entry) = self.parse_cache.borrow().get(&cache_key) {
            if entry.content_hash == content_hash {
                return Some((
                    content,
                    ParsedDocument {
                        tree: entry.tree.clone(),
                    },
                ));
            }
        }

        let parsed = ParsedDocument::parse(file_path, &content)?;
        self.parse_cache.borrow_mut().insert(
            cache_key,
            ParsedCacheEntry {
                content_hash,
                tree: parsed.tree.clone(),
            },
        );
        Some((content, parsed))
    }

    fn local_symbol_definition(
        &self,
        params: &Value,
        file_path: &str,
        symbol: &str,
    ) -> Option<lsp_types::Location> {
        let position = position_from_params(params)?;
        let (content, parsed) = self.parsed_document(params, file_path)?;
        let declaration = parsed.local_declaration_before(position, symbol, &content)?;
        Some(lsp_types::Location {
            uri: lsp_types::Uri::from_str(&convert::path_to_uri(&self.root, file_path)).ok()?,
            range: range_for_node(declaration),
        })
    }

    fn should_global_lookup(&self, params: &Value, file_path: &str, symbol: &str) -> bool {
        if symbol.contains('_')
            || symbol.contains('$')
            || symbol.chars().any(|c| c.is_ascii_uppercase())
        {
            return true;
        }
        self.symbol_looks_like_call(params, file_path, symbol)
    }

    fn symbol_looks_like_call(&self, params: &Value, file_path: &str, symbol: &str) -> bool {
        let Some(position) = position_from_params(params) else {
            return false;
        };
        let Some((content, parsed)) = self.parsed_document(params, file_path) else {
            return false;
        };
        parsed.identifier_is_call(position, symbol, &content)
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

struct ParsedCacheEntry {
    content_hash: u64,
    tree: tree_sitter::Tree,
}

struct ParsedDocument {
    tree: tree_sitter::Tree,
}

impl ParsedDocument {
    fn parse(file_path: &str, content: &str) -> Option<Self> {
        let language = tree_sitter_language_for_path(file_path)?;
        let mut parser = Parser::new();
        parser.set_language(&language).ok()?;
        let parse_content = preprocess_embedded_c_for_tree_sitter(file_path, content);
        let tree = parser.parse(parse_content.as_deref().unwrap_or(content), None)?;
        Some(Self { tree })
    }

    fn identifier_at_point(&self, position: Point) -> Option<TsNode<'_>> {
        let root = self.tree.root_node();
        let leaf = root.descendant_for_point_range(position, position)?;
        identifier_from_node_or_ancestor(leaf)
    }

    fn identifier_is_call(&self, position: Point, symbol: &str, source: &str) -> bool {
        let Some(identifier) = self.identifier_at_point(position) else {
            return false;
        };
        if text(identifier, source) != symbol {
            return false;
        }
        let Some(parent) = identifier.parent() else {
            return false;
        };
        is_call_function_node(identifier, parent)
    }

    fn local_declaration_before(
        &self,
        position: Point,
        symbol: &str,
        source: &str,
    ) -> Option<TsNode<'_>> {
        let root = self.tree.root_node();
        let cursor = root.descendant_for_point_range(position, position)?;
        let scope = enclosing_scope(cursor).unwrap_or(root);
        let mut best = None;
        find_local_declaration(scope, position, symbol, source, &mut best);
        best.map(|(_, node)| node)
    }
}

fn stable_hash(content: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

fn tree_sitter_language_for_path(file_path: &str) -> Option<TsLanguage> {
    let ext = std::path::Path::new(file_path).extension()?.to_str()?;
    match ext {
        "c" | "h" | "sc" | "qsc" | "qsh" => Some(tree_sitter_c::LANGUAGE.into()),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Some(tree_sitter_cpp::LANGUAGE.into()),
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "js" | "jsx" | "mjs" | "cjs" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "ts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "py" => Some(tree_sitter_python::LANGUAGE.into()),
        "sh" | "bash" => Some(tree_sitter_bash::LANGUAGE.into()),
        _ => None,
    }
}

fn preprocess_embedded_c_for_tree_sitter(file_path: &str, content: &str) -> Option<String> {
    let ext = std::path::Path::new(file_path).extension()?.to_str()?;
    match ext {
        "qsc" | "qsh" => Some(preprocess_equel_for_tree_sitter(content)),
        "sc" => Some(preprocess_esql_for_tree_sitter(content)),
        _ => None,
    }
}

fn preprocess_equel_for_tree_sitter(content: &str) -> String {
    content
        .split_inclusive('\n')
        .map(|line| {
            let line_body = line.strip_suffix('\n').unwrap_or(line);
            let newline = if line.ends_with('\n') { "\n" } else { "" };
            let leading = line_body.len() - line_body.trim_start().len();
            if line_body[leading..].starts_with("##") {
                format!(
                    "{}//{}{}",
                    &line_body[..leading],
                    &line_body[leading + 2..],
                    newline
                )
            } else {
                line.to_string()
            }
        })
        .collect()
}

fn preprocess_esql_for_tree_sitter(content: &str) -> String {
    let mut in_exec_sql = false;
    let mut out = String::with_capacity(content.len());
    for line in content.split_inclusive('\n') {
        let line_body = line.strip_suffix('\n').unwrap_or(line);
        let newline = if line.ends_with('\n') { "\n" } else { "" };
        let starts_exec_sql = line_starts_exec_sql_lsp(line_body);
        if in_exec_sql || starts_exec_sql {
            out.extend(
                line_body
                    .chars()
                    .map(|ch| if ch == '\t' { '\t' } else { ' ' }),
            );
            out.push_str(newline);
            if line_has_trailing_semicolon_lsp(line_body) {
                in_exec_sql = false;
            } else if starts_exec_sql {
                in_exec_sql = true;
            }
        } else {
            out.push_str(line);
        }
    }
    out
}

fn line_starts_exec_sql_lsp(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.len() >= 8
        && trimmed[..8].eq_ignore_ascii_case("exec sql")
        && (trimmed.len() == 8
            || trimmed.as_bytes()[8].is_ascii_whitespace()
            || trimmed.as_bytes()[8] == b';')
}

fn line_has_trailing_semicolon_lsp(line: &str) -> bool {
    let without_comment = line.split("/*").next().unwrap_or(line);
    without_comment.trim_end().ends_with(';')
}

fn position_from_params(params: &Value) -> Option<Point> {
    Some(Point {
        row: params.pointer("/position/line").and_then(Value::as_u64)? as usize,
        column: params
            .pointer("/position/character")
            .and_then(Value::as_u64)? as usize,
    })
}

fn range_for_node(node: TsNode<'_>) -> lsp_types::Range {
    let start = node.start_position();
    let end = node.end_position();
    lsp_types::Range {
        start: lsp_types::Position {
            line: start.row as u32,
            character: start.column as u32,
        },
        end: lsp_types::Position {
            line: end.row as u32,
            character: end.column as u32,
        },
    }
}

fn text<'a>(node: TsNode<'_>, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or("")
}

fn identifier_from_node_or_ancestor(mut node: TsNode<'_>) -> Option<TsNode<'_>> {
    loop {
        if is_identifier_kind(node.kind()) {
            return Some(node);
        }
        node = node.parent()?;
    }
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(
        kind,
        "identifier" | "field_identifier" | "property_identifier" | "type_identifier"
    )
}

fn enclosing_scope(mut node: TsNode<'_>) -> Option<TsNode<'_>> {
    loop {
        if matches!(
            node.kind(),
            "function_definition"
                | "function_item"
                | "function_declaration"
                | "method_definition"
                | "method_declaration"
                | "function_declarator"
                | "arrow_function"
                | "function"
                | "method_spec"
        ) {
            return Some(node);
        }
        node = node.parent()?;
    }
}

fn find_local_declaration<'a>(
    node: TsNode<'a>,
    position: Point,
    symbol: &str,
    source: &str,
    best: &mut Option<(usize, TsNode<'a>)>,
) {
    if starts_after_or_at(node, position) {
        return;
    }

    if is_local_declaration_kind(node.kind()) {
        if let Some(identifier) = declaration_identifier(node, symbol, source) {
            let start = identifier.start_byte();
            if best
                .map(|(best_start, _)| start > best_start)
                .unwrap_or(true)
            {
                *best = Some((start, identifier));
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        find_local_declaration(child, position, symbol, source, best);
    }
}

fn starts_after_or_at(node: TsNode<'_>, position: Point) -> bool {
    let start = node.start_position();
    start.row > position.row || (start.row == position.row && start.column >= position.column)
}

fn is_local_declaration_kind(kind: &str) -> bool {
    matches!(
        kind,
        "init_declarator"
            | "declaration"
            | "parameter_declaration"
            | "let_declaration"
            | "const_item"
            | "var_declaration"
            | "lexical_declaration"
            | "variable_declarator"
    )
}

fn declaration_identifier<'a>(node: TsNode<'a>, symbol: &str, source: &str) -> Option<TsNode<'a>> {
    if let Some(name) = node.child_by_field_name("name") {
        if is_identifier_kind(name.kind()) && text(name, source) == symbol {
            return Some(name);
        }
    }
    if let Some(declarator) = node.child_by_field_name("declarator") {
        if let Some(identifier) = first_identifier_named(declarator, symbol, source) {
            return Some(identifier);
        }
    }
    first_identifier_named(node, symbol, source)
}

fn first_identifier_named<'a>(node: TsNode<'a>, symbol: &str, source: &str) -> Option<TsNode<'a>> {
    if is_identifier_kind(node.kind()) && text(node, source) == symbol {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(identifier) = first_identifier_named(child, symbol, source) {
            return Some(identifier);
        }
    }
    None
}

fn is_call_function_node(identifier: TsNode<'_>, mut parent: TsNode<'_>) -> bool {
    loop {
        if parent.kind() == "call_expression" {
            return parent
                .child_by_field_name("function")
                .map(|function| contains_node(function, identifier))
                .unwrap_or_else(|| contains_node(parent, identifier));
        }
        if !matches!(
            parent.kind(),
            "identifier"
                | "field_identifier"
                | "property_identifier"
                | "field_expression"
                | "member_expression"
                | "scoped_identifier"
                | "qualified_identifier"
        ) {
            return false;
        }
        let Some(next) = parent.parent() else {
            return false;
        };
        parent = next;
    }
}

fn contains_node(haystack: TsNode<'_>, needle: TsNode<'_>) -> bool {
    needle.start_byte() >= haystack.start_byte() && needle.end_byte() <= haystack.end_byte()
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

    #[test]
    fn definition_prefers_tree_sitter_local_declaration_over_global_symbol() {
        let dir = temp_project("local-definition");
        fs::write(
            dir.join("main.c"),
            r#"void context(void) {}

void run(void) {
    int context = 0;
    context = context + 1;
}
"#,
        )
        .unwrap();
        let mut grph = Grph::init(&dir).unwrap();
        grph.index(|_| {}).unwrap();

        let handlers = LspHandlers::new(dir.clone()).unwrap();
        let value = handlers
            .definition(&json!({
                "textDocument": {"uri": convert::path_to_uri(&dir, "main.c")},
                "position": {"line": 4, "character": 4}
            }))
            .unwrap();
        let location: lsp_types::Location = serde_json::from_value(value).unwrap();
        assert_eq!(location.range.start.line, 3);
        assert_eq!(location.range.start.character, 8);
        assert_eq!(location.range.end.character, 15);

        fs::remove_dir_all(dir).ok();
    }
}
