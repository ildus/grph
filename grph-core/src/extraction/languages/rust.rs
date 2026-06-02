use crate::errors::{GrphError, Result};
use crate::extraction::tree_sitter::{
    build_qualified_name, generate_node_id, now_ms, ExtractionResult,
};
use crate::types::{Edge, EdgeKind, Language, Node, NodeKind};
use tree_sitter::{Node as TsNode, Parser};

pub fn extract(source: &str, file_path: &str) -> Result<ExtractionResult> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::language())
        .map_err(|e| GrphError::Parse(format!("failed to load Rust grammar: {e}")))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| GrphError::Parse("Rust tree-sitter parse failed".to_string()))?;

    let mut ctx = ExtractCtx {
        source,
        file_path,
        nodes: Vec::new(),
        edges: Vec::new(),
        errors: Vec::new(),
    };
    ctx.walk(tree.root_node(), None, None);
    ctx.extract_rust_impl_edges();

    Ok(ExtractionResult {
        nodes: ctx.nodes,
        edges: ctx.edges,
        errors: ctx.errors,
    })
}

struct ExtractCtx<'a> {
    source: &'a str,
    file_path: &'a str,
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
        match node.kind() {
            "struct_item" | "enum_item" | "trait_item" => {
                let Some(name) = field_text(node, "name", self.source) else {
                    self.walk_children(node, current_container, current_function);
                    return;
                };
                let kind = match node.kind() {
                    "struct_item" => NodeKind::Struct,
                    "enum_item" => NodeKind::Enum,
                    "trait_item" => NodeKind::Trait,
                    _ => NodeKind::Struct,
                };
                let container = self.make_node(
                    node,
                    kind,
                    &name,
                    Some(signature(node, self.source)),
                    visibility(node, self.source),
                );
                let id = container.id.clone();
                if kind == NodeKind::Trait {
                    self.extract_trait_supertrait_edges(node, &id);
                }
                self.nodes.push(container);
                self.walk_children(node, Some(id), current_function);
                return;
            }
            "impl_item" => {
                let name = impl_name(node, self.source).unwrap_or_else(|| "impl".to_string());
                let container = self.make_node(
                    node,
                    NodeKind::Namespace,
                    &name,
                    Some(signature(node, self.source)),
                    visibility(node, self.source),
                );
                let id = container.id.clone();
                self.nodes.push(container);
                self.walk_children(node, Some(id), current_function);
                return;
            }
            "function_item" => {
                let Some(name) = field_text(node, "name", self.source) else {
                    self.walk_children(node, current_container, current_function);
                    return;
                };
                let node_kind = if current_container.is_some() {
                    NodeKind::Method
                } else {
                    NodeKind::Function
                };
                let fn_node = self.make_node(
                    node,
                    node_kind,
                    &name,
                    Some(signature(node, self.source)),
                    visibility(node, self.source),
                );
                let fn_id = fn_node.id.clone();
                if let Some(container_id) = current_container.as_ref() {
                    self.edges.push(edge(
                        container_id,
                        &fn_id,
                        EdgeKind::Contains,
                        node,
                        "tree-sitter",
                    ));
                }
                self.nodes.push(fn_node);
                self.walk_children(node, current_container, Some(fn_id));
                return;
            }
            "use_declaration" => {
                let import_text = text(node, self.source).trim();
                let name = rust_import_root(import_text);
                let import_node = self.make_node(
                    node,
                    NodeKind::Import,
                    &name,
                    Some(import_text.to_string()),
                    visibility(node, self.source),
                );
                self.nodes.push(import_node);
            }
            "call_expression" => {
                if let Some(caller_id) = current_function.as_ref() {
                    if let Some(function) = node.child_by_field_name("function") {
                        let called = call_target(function, self.source);
                        if !called.is_empty() && !is_rust_keyword_or_builtin(&called) {
                            self.edges.push(edge(
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
            "macro_invocation" => {
                if let Some(caller_id) = current_function.as_ref() {
                    if let Some(mac) = node.child_by_field_name("macro") {
                        let name = format!("{}!", text(mac, self.source).trim());
                        self.edges.push(edge(
                            caller_id,
                            &name,
                            EdgeKind::Calls,
                            node,
                            "tree-sitter",
                        ));
                    }
                }
            }
            _ => {}
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
            language: Language::Rust,
            start_line: (start.row + 1) as u32,
            end_line: (end.row + 1) as u32,
            start_column: start.column as u32,
            end_column: end.column as u32,
            docstring: None,
            signature,
            visibility,
            is_exported: text(node, self.source).trim_start().starts_with("pub"),
            is_async: text(node, self.source).contains("async fn"),
            is_static: false,
            is_abstract: kind == NodeKind::Trait,
            decorators: rust_attributes_before(self.source, (start.row + 1) as u32),
            type_parameters: None,
            updated_at: now_ms(),
        }
    }

    fn extract_trait_supertrait_edges(&mut self, node: TsNode<'a>, trait_id: &str) {
        let header = signature(node, self.source);
        let Some(after_colon) = header.split_once(':').map(|(_, rest)| rest) else {
            return;
        };
        let supertraits = after_colon
            .split('{')
            .next()
            .unwrap_or(after_colon)
            .split('+')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.split('<').next().unwrap_or(s).trim())
            .filter(|s| !s.is_empty() && *s != "?Sized")
            .map(|s| s.rsplit("::").next().unwrap_or(s).to_string())
            .collect::<Vec<_>>();

        for target in supertraits {
            self.edges.push(edge(
                trait_id,
                &target,
                EdgeKind::Extends,
                node,
                "tree-sitter",
            ));
        }
    }

    fn extract_rust_impl_edges(&mut self) {
        use std::collections::HashMap;

        let struct_ids: HashMap<String, String> = self
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Struct | NodeKind::Enum))
            .map(|n| (n.name.clone(), n.id.clone()))
            .collect();

        let impls = self
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Namespace && n.name.starts_with("impl "))
            .cloned()
            .collect::<Vec<_>>();

        for impl_node in impls {
            let Some((trait_name, type_name)) = parse_impl_trait_for_type(&impl_node.name) else {
                continue;
            };
            let source = struct_ids
                .get(&type_name)
                .cloned()
                .unwrap_or_else(|| type_name.clone());
            self.edges.push(Edge {
                source,
                target: trait_name,
                kind: EdgeKind::Implements,
                metadata: Some(serde_json::json!({
                    "implNode": impl_node.id,
                })),
                line: Some(impl_node.start_line),
                col: Some(impl_node.start_column),
                provenance: Some("tree-sitter".to_string()),
            });
        }
    }
}

fn edge(source: &str, target: &str, kind: EdgeKind, node: TsNode<'_>, provenance: &str) -> Edge {
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

fn text<'a>(node: TsNode<'_>, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or("")
}

fn field_text(node: TsNode<'_>, field: &str, source: &str) -> Option<String> {
    node.child_by_field_name(field)
        .map(|n| text(n, source).trim().to_string())
        .filter(|s| !s.is_empty())
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
    if text(node, source).trim_start().starts_with("pub") {
        Some("public".to_string())
    } else {
        Some("private".to_string())
    }
}

fn impl_name(node: TsNode<'_>, source: &str) -> Option<String> {
    let full = text(node, source).trim();
    let head = full.split('{').next().unwrap_or(full).trim();
    Some(head.to_string()).filter(|s| !s.is_empty())
}

fn rust_import_root(import_text: &str) -> String {
    import_text
        .trim()
        .trim_start_matches("pub ")
        .trim_start_matches("use ")
        .trim_end_matches(';')
        .trim()
        .split("::")
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

fn parse_impl_trait_for_type(name: &str) -> Option<(String, String)> {
    let rest = name.trim().strip_prefix("impl ")?.trim();
    let (trait_part, type_part) = rest.split_once(" for ")?;
    let trait_name = trait_part
        .trim()
        .split('<')
        .next()
        .unwrap_or(trait_part)
        .rsplit("::")
        .next()
        .unwrap_or(trait_part)
        .trim()
        .to_string();
    let type_name = type_part
        .trim()
        .split(|c: char| c == '<' || c == ' ' || c == '{')
        .next()
        .unwrap_or(type_part)
        .rsplit("::")
        .next()
        .unwrap_or(type_part)
        .trim()
        .to_string();
    if trait_name.is_empty() || type_name.is_empty() {
        None
    } else {
        Some((trait_name, type_name))
    }
}

fn call_target(function: TsNode<'_>, source: &str) -> String {
    let raw = text(function, source).trim();
    raw.rsplit([':', '.'])
        .next()
        .unwrap_or(raw)
        .trim()
        .to_string()
}

fn is_rust_keyword_or_builtin(name: &str) -> bool {
    matches!(
        name,
        "if" | "for"
            | "while"
            | "loop"
            | "match"
            | "return"
            | "Some"
            | "None"
            | "Ok"
            | "Err"
            | "vec"
    )
}

fn rust_attributes_before(source: &str, start_line: u32) -> Option<Vec<String>> {
    let lines: Vec<&str> = source.lines().collect();
    let mut attrs = Vec::new();
    let mut idx = start_line.saturating_sub(1) as usize;
    while idx > 0 {
        idx -= 1;
        let trimmed = lines.get(idx).map(|line| line.trim()).unwrap_or("");
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("#[") && trimmed.ends_with(']') {
            attrs.push(
                trimmed
                    .trim_start_matches("#[")
                    .trim_end_matches(']')
                    .to_string(),
            );
            continue;
        }
        break;
    }
    attrs.reverse();
    if attrs.is_empty() {
        None
    } else {
        Some(attrs)
    }
}
