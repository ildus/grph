use crate::errors::{GrphError, Result};
use crate::extraction::tree_sitter::{
    build_qualified_name, generate_node_id, now_ms, ExtractionResult,
};
use crate::types::{Edge, EdgeKind, Language, Node, NodeKind};
use tree_sitter::{Node as TsNode, Parser};

#[derive(Clone, Copy)]
pub struct KindMap {
    pub ts_kind: &'static str,
    pub node_kind: NodeKind,
}

pub struct TreeConfig<'a> {
    pub language: Language,
    pub grammar: tree_sitter::Language,
    pub parser_name: &'static str,
    pub container_kinds: &'a [KindMap],
    pub function_kinds: &'a [&'static str],
    pub method_kinds: &'a [&'static str],
    pub import_kinds: &'a [&'static str],
    pub variable_kinds: &'a [&'static str],
    pub call_kinds: &'a [&'static str],
}

pub fn extract_with_tree_sitter(
    source: &str,
    file_path: &str,
    cfg: TreeConfig<'_>,
) -> Result<ExtractionResult> {
    let mut parser = Parser::new();
    parser.set_language(&cfg.grammar).map_err(|e| {
        GrphError::Parse(format!("failed to load {} grammar: {e}", cfg.parser_name))
    })?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| GrphError::Parse(format!("{} tree-sitter parse failed", cfg.parser_name)))?;
    let language = cfg.language;
    let file_node_id = generate_node_id(file_path, NodeKind::File.as_str(), file_path, 1);

    let mut ctx = ExtractCtx {
        source,
        file_path,
        cfg,
        file_node_id: file_node_id.clone(),
        nodes: Vec::new(),
        edges: Vec::new(),
        errors: Vec::new(),
    };
    ctx.nodes
        .push(file_node(source, file_path, language, file_node_id));
    ctx.walk(tree.root_node(), None, None);
    ctx.post_process();

    Ok(ExtractionResult {
        nodes: ctx.nodes,
        edges: ctx.edges,
        errors: ctx.errors,
    })
}

struct ExtractCtx<'a> {
    source: &'a str,
    file_path: &'a str,
    cfg: TreeConfig<'a>,
    file_node_id: String,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    errors: Vec<String>,
}

impl<'a> ExtractCtx<'a> {
    fn walk(
        &mut self,
        node: TsNode<'a>,
        current_container: Option<String>,
        current_function: Option<String>,
    ) {
        let kind = node.kind();

        if matches!(kind, "type_definition" | "alias_declaration") {
            if let Some((node_kind, name)) = type_alias_node(node, self.source) {
                let symbol = self.make_node(
                    node,
                    node_kind,
                    &name,
                    Some(signature(node, self.source)),
                    visibility(node, self.source),
                );
                let id = symbol.id.clone();
                if let Some(parent_id) = current_container.as_ref() {
                    self.edges.push(make_edge(
                        parent_id,
                        &id,
                        EdgeKind::Contains,
                        node,
                        "tree-sitter",
                    ));
                }
                self.nodes.push(symbol);
                return;
            }
        }

        if let Some(mapped) = self.cfg.container_kinds.iter().find(|m| m.ts_kind == kind) {
            // Go `type_spec` can be either a struct or an interface; peek inside.
            let effective_kind = if self.cfg.language == Language::Go
                && kind == "type_spec"
                && has_child_kind(node, "interface_type")
            {
                NodeKind::Interface
            } else {
                mapped.node_kind
            };

            if let Some(name) = node_name(node, self.source) {
                let symbol = self.make_node(
                    node,
                    effective_kind,
                    &name,
                    Some(signature(node, self.source)),
                    visibility(node, self.source),
                );
                let id = symbol.id.clone();
                if let Some(parent_id) = current_container.as_ref() {
                    self.edges.push(make_edge(
                        parent_id,
                        &id,
                        EdgeKind::Contains,
                        node,
                        "tree-sitter",
                    ));
                }
                self.extract_inheritance_edges(node, &id);
                // Extract decorators from preceding siblings and direct children.
                self.extract_js_ts_decorators_for(node, &id);
                self.nodes.push(symbol);
                self.walk_children(node, Some(id), current_function);
                return;
            }
        }

        if contains(self.cfg.function_kinds, kind) || contains(self.cfg.method_kinds, kind) {
            if let Some(name) = function_name(node, self.source) {
                let node_kind =
                    if current_container.is_some() || contains(self.cfg.method_kinds, kind) {
                        NodeKind::Method
                    } else {
                        NodeKind::Function
                    };
                let symbol = self.make_node(
                    node,
                    node_kind,
                    &name,
                    Some(signature(node, self.source)),
                    visibility(node, self.source),
                );
                let id = symbol.id.clone();
                if let Some(parent_id) = current_container.as_ref() {
                    self.edges.push(make_edge(
                        parent_id,
                        &id,
                        EdgeKind::Contains,
                        node,
                        "tree-sitter",
                    ));
                }
                // Extract decorators for methods too (e.g. `@Get('/x') method() {}`).
                self.extract_js_ts_decorators_for(node, &id);
                self.nodes.push(symbol);
                self.walk_children(node, current_container, Some(id));
                return;
            }
        }

        if contains(self.cfg.import_kinds, kind) {
            let import_text = text(node, self.source).trim();
            let name = cleanup_import_name(import_text, self.cfg.language);
            if !name.is_empty() {
                let symbol = self.make_node(
                    node,
                    NodeKind::Import,
                    &name,
                    Some(import_text.to_string()),
                    visibility(node, self.source),
                );
                let id = symbol.id.clone();
                self.edges.push(make_edge(
                    &self.file_node_id,
                    &id,
                    EdgeKind::Imports,
                    node,
                    "tree-sitter",
                ));
                self.nodes.push(symbol);
            }
        }

        if matches!(kind, "preproc_def" | "preproc_function_def") {
            if let Some(name) = preproc_macro_name(node, self.source) {
                let symbol = self.make_node(
                    node,
                    NodeKind::Constant,
                    &name,
                    Some(signature(node, self.source)),
                    visibility(node, self.source),
                );
                let id = symbol.id.clone();
                self.collect_identifier_references(node, &id, &name);
                self.nodes.push(symbol);
            }
        }

        if contains(self.cfg.variable_kinds, kind) {
            if let Some(name) = variable_name(node, self.source) {
                if self.is_js_like_variable_function(node) {
                    let symbol = self.make_node(
                        node,
                        NodeKind::Function,
                        &name,
                        Some(signature(node, self.source)),
                        visibility(node, self.source),
                    );
                    let id = symbol.id.clone();
                    self.nodes.push(symbol);
                    self.walk_children(node, current_container.clone(), Some(id));
                    return;
                }

                let node_kind = variable_node_kind(node, self.source, self.cfg.language, &name);
                let symbol = self.make_node(
                    node,
                    node_kind,
                    &name,
                    Some(signature(node, self.source)),
                    visibility(node, self.source),
                );
                let id = symbol.id.clone();
                self.extract_initializer_references(node, &id, &name);
                self.nodes.push(symbol);
            }
        }

        if contains(self.cfg.call_kinds, kind) {
            if let Some(caller_id) = current_function.as_ref() {
                if let Some(called) = call_name(node, self.source) {
                    if !is_common_keyword_or_builtin(&called) {
                        self.edges.push(make_edge(
                            caller_id,
                            &called,
                            EdgeKind::Calls,
                            node,
                            "tree-sitter",
                        ));
                    }
                }
            }
        }

        if let Some(caller_id) = current_function.as_ref() {
            if let Some(name) = instantiation_name(node, self.source, self.cfg.language) {
                if !is_common_keyword_or_builtin(&name) {
                    self.edges.push(make_edge(
                        caller_id,
                        &name,
                        EdgeKind::Instantiates,
                        node,
                        "tree-sitter",
                    ));
                }
            }
        }

        self.walk_children(node, current_container, current_function);
    }

    fn walk_children(
        &mut self,
        node: TsNode<'a>,
        current_container: Option<String>,
        current_function: Option<String>,
    ) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk(child, current_container.clone(), current_function.clone());
        }
    }

    fn make_node(
        &self,
        node: TsNode<'a>,
        kind: NodeKind,
        name: &str,
        signature: Option<String>,
        visibility: Option<String>,
    ) -> Node {
        let start = node.start_position();
        let end = node.end_position();
        Node {
            id: generate_node_id(self.file_path, kind.as_str(), name, (start.row + 1) as u32),
            kind,
            name: name.to_string(),
            qualified_name: build_qualified_name(self.file_path, name),
            file_path: self.file_path.to_string(),
            language: self.cfg.language,
            start_line: (start.row + 1) as u32,
            end_line: (end.row + 1) as u32,
            start_column: start.column as u32,
            end_column: end.column as u32,
            docstring: None,
            signature,
            visibility,
            is_exported: is_exported(node, self.source, self.cfg.language, name),
            is_async: text(node, self.source).contains("async"),
            is_static: text(node, self.source).contains("static"),
            is_abstract: kind == NodeKind::Interface || kind == NodeKind::Trait,
            decorators: None,
            type_parameters: None,
            updated_at: now_ms(),
        }
    }

    fn is_js_like_variable_function(&self, node: TsNode<'a>) -> bool {
        if !matches!(
            self.cfg.language,
            Language::JavaScript | Language::TypeScript | Language::Tsx | Language::Jsx
        ) {
            return false;
        }
        node.child_by_field_name("value")
            .map(|value| matches!(value.kind(), "arrow_function" | "function_expression"))
            .unwrap_or(false)
    }

    fn extract_inheritance_edges(&mut self, node: TsNode<'a>, class_id: &str) {
        let header = signature(node, self.source);
        if matches!(
            self.cfg.language,
            Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Jsx
        ) {
            for target in js_ts_extends_targets(&header) {
                self.edges.push(make_edge(
                    class_id,
                    &target,
                    EdgeKind::Extends,
                    node,
                    "tree-sitter",
                ));
            }
            for target in js_ts_implements_targets(&header) {
                self.edges.push(make_edge(
                    class_id,
                    &target,
                    EdgeKind::Implements,
                    node,
                    "tree-sitter",
                ));
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            let edge_kind = match child.kind() {
                "base_class_clause" | "superclass" | "extends_clause" => Some(EdgeKind::Extends),
                "implements_clause" | "class_heritage" => Some(EdgeKind::Implements),
                _ => None,
            };
            if let Some(kind) = edge_kind {
                for target in type_names_in(child, self.source) {
                    self.edges
                        .push(make_edge(class_id, &target, kind, child, "tree-sitter"));
                }
            }
        }
    }

    fn post_process(&mut self) {
        self.synthesize_file_contains_edges();
        if self.cfg.language == Language::Cpp {
            self.synthesize_cpp_override_edges();
        }
    }

    fn synthesize_file_contains_edges(&mut self) {
        use std::collections::HashSet;

        let contained: HashSet<&str> = self
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Contains)
            .map(|edge| edge.target.as_str())
            .collect();
        let top_level_ids: Vec<String> = self
            .nodes
            .iter()
            .filter(|node| node.id != self.file_node_id && !contained.contains(node.id.as_str()))
            .map(|node| node.id.clone())
            .collect();

        for target in top_level_ids {
            self.edges.push(Edge {
                source: self.file_node_id.clone(),
                target,
                kind: EdgeKind::Contains,
                metadata: None,
                line: Some(1),
                col: Some(0),
                provenance: Some("tree-sitter".to_string()),
            });
        }
    }

    /// Extract decorators for JS/TS/TSX declarations.
    ///
    /// In TypeScript, `@Foo class Bar {}` parses as a `decorator` node followed
    /// by a `class_declaration` inside an `export_statement` (or top-level
    /// wrapper). The decorator is a **preceding sibling** of the class, not a
    /// child. For methods/properties, the decorator is a **direct child** of
    /// the declaration.
    ///
    /// We scan both locations and emit a `decorates` edge from the decorated
    /// symbol to each decorator's function name.
    fn extract_js_ts_decorators_for(&mut self, decl_node: TsNode<'a>, decorated_id: &str) {
        if !matches!(
            self.cfg.language,
            Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Jsx
        ) {
            return;
        }

        // 1. Decorators that are direct children of the declaration
        //    (method/property style, also some grammars for class).
        let mut cursor = decl_node.walk();
        for child in decl_node.children(&mut cursor) {
            self.emit_decorator_edge(child, decorated_id);
        }

        // 2. Decorators that are PRECEDING siblings of the declaration
        //    inside the parent's children (TypeScript class style).
        //    Walk BACKWARDS from the declaration and stop at the first
        //    non-decorator sibling — without that stop, decorators
        //    belonging to an EARLIER unrelated declaration leak in.
        if let Some(parent) = decl_node.parent() {
            let decl_start = decl_node.start_position();
            // Collect all children and find the index of the declaration.
            let mut children: Vec<TsNode<'a>> = Vec::new();
            let mut child_cursor = parent.walk();
            for child in parent.children(&mut child_cursor) {
                children.push(child);
            }

            let decl_idx = children.iter().position(|c| {
                c.start_position() == decl_start && c.end_position() == decl_node.end_position()
            });

            if let Some(idx) = decl_idx {
                // Walk backwards from idx-1, collecting decorators until we hit
                // a non-decorator.
                let mut j = idx;
                while j > 0 {
                    j -= 1;
                    let sibling = children[j];
                    if sibling.kind() != "decorator" {
                        break; // non-decorator separator → stop
                    }
                    self.emit_decorator_edge(sibling, decorated_id);
                }
            }
        }
    }

    /// Emit a `decorates` edge for a single decorator node.
    fn emit_decorator_edge(&mut self, decorator_node: TsNode<'a>, decorated_id: &str) {
        if decorator_node.kind() != "decorator" {
            return;
        }
        let name = decorator_function_name(decorator_node, self.source);
        if let Some(name) = name {
            if !name.is_empty() {
                self.edges.push(make_edge(
                    decorated_id,
                    &name,
                    EdgeKind::Decorates,
                    decorator_node,
                    "tree-sitter",
                ));
            }
        }
    }

    fn synthesize_cpp_override_edges(&mut self) {
        use std::collections::HashMap;

        let class_by_name: HashMap<String, String> = self
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Class | NodeKind::Struct))
            .map(|n| (n.name.clone(), n.id.clone()))
            .collect();

        let methods_by_class: HashMap<String, Vec<Node>> = self
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Contains)
            .filter_map(|e| {
                self.nodes
                    .iter()
                    .find(|n| n.id == e.target && n.kind == NodeKind::Method)
                    .map(|m| (e.source.clone(), m.clone()))
            })
            .fold(HashMap::new(), |mut acc, (class_id, method)| {
                acc.entry(class_id).or_default().push(method);
                acc
            });

        let extends = self
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Extends)
            .cloned()
            .collect::<Vec<_>>();

        for edge in extends {
            let Some(base_id) = class_by_name.get(&edge.target) else {
                continue;
            };
            let Some(base_methods) = methods_by_class.get(base_id) else {
                continue;
            };
            let Some(child_methods) = methods_by_class.get(&edge.source) else {
                continue;
            };

            for base in base_methods {
                for child in child_methods.iter().filter(|m| m.name == base.name) {
                    if !self.edges.iter().any(|e| {
                        e.kind == EdgeKind::Calls && e.source == base.id && e.target == child.id
                    }) {
                        self.edges.push(Edge {
                            source: base.id.clone(),
                            target: child.id.clone(),
                            kind: EdgeKind::Calls,
                            metadata: Some(serde_json::json!({
                                "synthesizedBy": "cpp-override",
                                "via": child.name,
                            })),
                            line: Some(child.start_line),
                            col: Some(child.start_column),
                            provenance: Some("tree-sitter+cpp-override".to_string()),
                        });
                    }
                }
            }
        }
    }

    fn extract_initializer_references(
        &mut self,
        node: TsNode<'a>,
        source_id: &str,
        source_name: &str,
    ) {
        let Some(value) = node
            .child_by_field_name("value")
            .or_else(|| initializer_child(node))
        else {
            return;
        };
        self.collect_identifier_references(value, source_id, source_name);
    }

    fn collect_identifier_references(
        &mut self,
        node: TsNode<'a>,
        source_id: &str,
        source_name: &str,
    ) {
        if node.kind() == "identifier" {
            let name = text(node, self.source).trim();
            if !name.is_empty() && name != source_name && !is_common_keyword_or_builtin(name) {
                self.edges.push(make_edge(
                    source_id,
                    name,
                    EdgeKind::References,
                    node,
                    "tree-sitter",
                ));
            }
            return;
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_identifier_references(child, source_id, source_name);
        }
    }
}

fn contains(list: &[&str], kind: &str) -> bool {
    list.iter().any(|candidate| *candidate == kind)
}

fn text<'a>(node: TsNode<'_>, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or("")
}

fn type_alias_node(node: TsNode<'_>, source: &str) -> Option<(NodeKind, String)> {
    let name = typedef_alias_name(node, source)
        .or_else(|| field_text(node, "name", source))
        .or_else(|| declarator_identifier(node, source))?;

    let mut node_kind = NodeKind::TypeAlias;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "struct_specifier" => {
                node_kind = NodeKind::Struct;
                break;
            }
            "enum_specifier" => {
                node_kind = NodeKind::Enum;
                break;
            }
            "class_specifier" => {
                node_kind = NodeKind::Class;
                break;
            }
            _ => {}
        }
    }

    Some((node_kind, name))
}

fn node_name(node: TsNode<'_>, source: &str) -> Option<String> {
    if matches!(
        node.kind(),
        "struct_specifier" | "enum_specifier" | "class_specifier"
    ) {
        return field_text(node, "name", source);
    }

    field_text(node, "name", source)
        .or_else(|| typedef_alias_name(node, source))
        .or_else(|| {
            node.child_by_field_name("declarator")
                .and_then(|n| declarator_identifier(n, source))
        })
        .or_else(|| shallow_identifier_in(node, source))
}

fn function_name(node: TsNode<'_>, source: &str) -> Option<String> {
    field_text(node, "name", source)
        .or_else(|| {
            node.child_by_field_name("declarator")
                .and_then(|n| declarator_identifier(n, source))
        })
        .or_else(|| declarator_identifier(node, source))
}

fn variable_name(node: TsNode<'_>, source: &str) -> Option<String> {
    field_text(node, "name", source)
        .or_else(|| {
            node.child_by_field_name("pattern")
                .and_then(|n| declarator_identifier(n, source))
        })
        .or_else(|| {
            node.child_by_field_name("declarator")
                .and_then(|n| declarator_identifier(n, source))
        })
        .or_else(|| shallow_identifier_in(node, source))
}

fn variable_node_kind(node: TsNode<'_>, source: &str, language: Language, name: &str) -> NodeKind {
    let s = text(node, source).trim_start();
    let parent_is_const = ancestor_text_starts_with_const(node, source);
    if s.starts_with("const ")
        || parent_is_const
        || (matches!(
            language,
            Language::JavaScript | Language::TypeScript | Language::Tsx | Language::Jsx
        ) && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit()))
    {
        NodeKind::Constant
    } else {
        NodeKind::Variable
    }
}

fn file_node(source: &str, file_path: &str, language: Language, id: String) -> Node {
    let line_count = source.lines().count().max(1) as u32;
    Node {
        id,
        kind: NodeKind::File,
        name: file_path
            .rsplit('/')
            .next()
            .unwrap_or(file_path)
            .to_string(),
        qualified_name: file_path.to_string(),
        file_path: file_path.to_string(),
        language,
        start_line: 1,
        end_line: line_count,
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

fn instantiation_name(node: TsNode<'_>, source: &str, language: Language) -> Option<String> {
    if !matches!(
        language,
        Language::Cpp | Language::JavaScript | Language::TypeScript | Language::Tsx | Language::Jsx
    ) {
        return None;
    }
    if !matches!(node.kind(), "new_expression" | "object_creation_expression") {
        return None;
    }
    type_names_in(node, source).into_iter().next().or_else(|| {
        node.child_by_field_name("type")
            .map(|node| text(node, source).trim().to_string())
            .filter(|name| !name.is_empty())
    })
}

fn initializer_child<'a>(node: TsNode<'a>) -> Option<TsNode<'a>> {
    if matches!(node.kind(), "initializer_list" | "initializer_pair") {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(initializer) = initializer_child(child) {
            return Some(initializer);
        }
    }
    None
}

fn ancestor_text_starts_with_const(node: TsNode<'_>, source: &str) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        let kind = parent.kind();
        if matches!(
            kind,
            "lexical_declaration" | "variable_declaration" | "const_declaration" | "const_spec"
        ) {
            return text(parent, source).trim_start().starts_with("const");
        }
        if matches!(kind, "program" | "source_file" | "translation_unit") {
            break;
        }
        current = parent.parent();
    }
    false
}

fn field_text(node: TsNode<'_>, field: &str, source: &str) -> Option<String> {
    node.child_by_field_name(field)
        .map(|n| text(n, source).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn preproc_macro_name(node: TsNode<'_>, source: &str) -> Option<String> {
    field_text(node, "name", source).or_else(|| shallow_identifier_in(node, source))
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "property_identifier"
            | "type_identifier"
            | "field_identifier"
            | "namespace_identifier"
    )
}

fn shallow_identifier_in(node: TsNode<'_>, source: &str) -> Option<String> {
    if is_identifier_kind(node.kind()) {
        return Some(text(node, source).trim().to_string()).filter(|s| !s.is_empty());
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if is_identifier_kind(child.kind()) {
            return Some(text(child, source).trim().to_string()).filter(|s| !s.is_empty());
        }
    }
    None
}

fn declarator_identifier(node: TsNode<'_>, source: &str) -> Option<String> {
    if is_identifier_kind(node.kind()) {
        return Some(text(node, source).trim().to_string()).filter(|s| !s.is_empty());
    }

    // Prefer the identifier that belongs to the declarator rather than identifiers
    // that appear in parameter lists or function bodies. This matters for C/C++
    // function and typedef declarators.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "parameter_list" | "field_declaration_list" | "compound_statement"
        ) {
            continue;
        }
        if let Some(name) = declarator_identifier(child, source) {
            return Some(name);
        }
    }
    None
}

fn typedef_alias_name(node: TsNode<'_>, source: &str) -> Option<String> {
    if node.kind() != "type_definition" {
        return None;
    }
    node.child_by_field_name("declarator")
        .and_then(|n| declarator_identifier(n, source))
}

fn js_ts_extends_targets(header: &str) -> Vec<String> {
    let Some((_, after)) = header.split_once(" extends ") else {
        return Vec::new();
    };
    let before_implements = after.split(" implements ").next().unwrap_or(after);
    clean_type_target(before_implements).into_iter().collect()
}

fn js_ts_implements_targets(header: &str) -> Vec<String> {
    let Some((_, after)) = header.split_once(" implements ") else {
        return Vec::new();
    };
    after.split(',').filter_map(clean_type_target).collect()
}

fn clean_type_target(raw: &str) -> Option<String> {
    let name = raw
        .trim()
        .split(['{', '<', ' '])
        .next()
        .unwrap_or(raw)
        .trim()
        .rsplit('.')
        .next()
        .unwrap_or(raw)
        .trim()
        .to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn type_names_in(node: TsNode<'_>, source: &str) -> Vec<String> {
    let mut names = Vec::new();
    collect_type_names(node, source, &mut names);
    names.sort();
    names.dedup();
    names
}

fn collect_type_names(node: TsNode<'_>, source: &str, names: &mut Vec<String>) {
    if matches!(
        node.kind(),
        "type_identifier" | "qualified_identifier" | "template_type" | "identifier"
    ) {
        let name = text(node, source)
            .trim()
            .split("::")
            .last()
            .unwrap_or("")
            .trim()
            .to_string();
        if !name.is_empty() && !matches!(name.as_str(), "public" | "private" | "protected") {
            names.push(name);
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_type_names(child, source, names);
    }
}

fn signature(node: TsNode<'_>, source: &str) -> String {
    text(node, source)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

fn visibility(node: TsNode<'_>, source: &str) -> Option<String> {
    let s = text(node, source).trim_start();
    if s.starts_with("pub") || s.starts_with("export") {
        Some("public".to_string())
    } else {
        Some("private".to_string())
    }
}

fn is_exported(node: TsNode<'_>, source: &str, language: Language, name: &str) -> bool {
    let s = text(node, source).trim_start();
    s.starts_with("pub")
        || s.starts_with("export")
        || ancestor_text_starts_with_export(node)
        || matches!(language, Language::Go) && name.chars().next().map_or(false, char::is_uppercase)
}

fn ancestor_text_starts_with_export(node: TsNode<'_>) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if matches!(parent.kind(), "export_statement" | "export_declaration") {
            return true;
        }
        if matches!(
            parent.kind(),
            "program" | "source_file" | "translation_unit"
        ) {
            break;
        }
        current = parent.parent();
    }
    false
}

fn cleanup_import_name(import_text: &str, language: Language) -> String {
    let cleaned = import_text
        .trim()
        .trim_end_matches(';')
        .trim_start_matches("pub ")
        .trim_start_matches("import ")
        .trim_start_matches("use ")
        .trim_start_matches("#include")
        .trim();

    match language {
        Language::Go => cleanup_go_import_name(cleaned),
        Language::C | Language::Cpp => cleaned.trim_matches(['<', '>', '"']).trim().to_string(),
        _ => cleaned
            .split_whitespace()
            .last()
            .unwrap_or(cleaned)
            .trim_matches(['"', '\'', ','])
            .to_string(),
    }
}

fn cleanup_go_import_name(cleaned: &str) -> String {
    let trimmed = cleaned.trim();
    if trimmed.starts_with('(') && trimmed.ends_with(')') {
        return String::new();
    }
    trimmed
        .split_whitespace()
        .last()
        .unwrap_or(trimmed)
        .trim_matches(['(', ')', '"', '`'])
        .trim()
        .to_string()
}

fn call_name(node: TsNode<'_>, source: &str) -> Option<String> {
    let function = node
        .child_by_field_name("function")
        .or_else(|| node.child_by_field_name("operand"))
        .unwrap_or(node);
    let raw = text(function, source).trim();
    let name = raw
        .trim_end_matches('!')
        .rsplit(|c| matches!(c, '.' | ':' | '>' | '-' | '/'))
        .find(|part| !part.is_empty())
        .unwrap_or(raw)
        .trim()
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '$')
        .to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn is_common_keyword_or_builtin(name: &str) -> bool {
    matches!(
        name,
        "if" | "for"
            | "while"
            | "switch"
            | "match"
            | "return"
            | "sizeof"
            | "typeof"
            | "new"
            | "delete"
            | "console"
            | "print"
            | "println"
            | "len"
            | "range"
            | "Some"
            | "None"
            | "Ok"
            | "Err"
    )
}

/// Check if a node has a direct child of the given kind.
fn has_child_kind(node: TsNode<'_>, target_kind: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == target_kind {
            return true;
        }
    }
    false
}

/// Extract the function name from a JS/TS decorator node.
///
/// Handles:
///   @Foo          → "Foo"
///   @Foo()        → "Foo"
///   @ns.Foo()     → "Foo"
///   @ns.Foo       → "Foo"
///   @Get('/x')    → "Get"
fn decorator_function_name<'a>(node: TsNode<'a>, source: &'a str) -> Option<String> {
    // Find the first named child that is a call_expression, identifier, or
    // member_expression. Unwrap call_expression to get the function name.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "call_expression" {
            // Unwrap: `@Get('/x')` → extract "Get" from the function field.
            if let Some(fn_child) = child
                .child_by_field_name("function")
                .or_else(|| child.named_child(0))
            {
                return decorator_leaf_name(fn_child, source);
            }
        }
        if matches!(
            child.kind(),
            "identifier" | "member_expression" | "scoped_identifier" | "navigation_expression"
        ) {
            return decorator_leaf_name(child, source);
        }
    }
    None
}

/// Extract the trailing identifier from a decorator target expression.
fn decorator_leaf_name<'a>(node: TsNode<'a>, source: &'a str) -> Option<String> {
    if node.kind() == "identifier" {
        return Some(text(node, source).trim().to_string());
    }
    // For member_expression / scoped_identifier / navigation_expression,
    // return the last dot-separated segment.
    let full = text(node, source).trim().to_string();
    let name = full
        .rsplit(&['.', ':', '>'][..])
        .find(|p| !p.is_empty())
        .unwrap_or(&full)
        .trim()
        .to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn make_edge(
    source: &str,
    target: &str,
    kind: EdgeKind,
    node: TsNode<'_>,
    provenance: &str,
) -> Edge {
    let start = node.start_position();
    Edge {
        source: source.to_string(),
        target: target.to_string(),
        kind,
        metadata: None,
        line: Some((start.row + 1) as u32),
        col: Some(start.column as u32),
        provenance: Some(provenance.to_string()),
    }
}
