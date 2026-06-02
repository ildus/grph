use crate::errors::{GrphError, Result};
use crate::extraction::tree_sitter::{
    build_qualified_name, generate_node_id, now_ms, ExtractionResult,
};
use crate::types::{Edge, EdgeKind, Language, Node, NodeKind};
use tree_sitter::{Node as TsNode, Parser};

pub fn extract(source: &str, file_path: &str) -> Result<ExtractionResult> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::language())
        .map_err(|e| GrphError::Parse(format!("failed to load Python grammar: {e}")))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| GrphError::Parse("Python tree-sitter parse failed".to_string()))?;

    let mut ctx = ExtractCtx {
        source,
        file_path,
        nodes: Vec::new(),
        edges: Vec::new(),
        errors: Vec::new(),
    };
    ctx.walk(tree.root_node(), None, None);

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
        let kind = node.kind();

        if kind == "class_definition" {
            let Some(name) = field_text(node, "name", self.source) else {
                self.walk_children(node, current_container, current_function);
                return;
            };
            let class_node = self.make_node(
                node,
                NodeKind::Class,
                &name,
                Some(signature(node, self.source)),
                None,
            );
            let class_id = class_node.id.clone();
            self.nodes.push(class_node);
            self.walk_children(node, Some(class_id), current_function);
            return;
        }

        if kind == "function_definition" {
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
                None,
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

        if matches!(kind, "import_statement" | "import_from_statement") {
            let import_text = text(node, self.source).trim();
            let name = import_name(import_text);
            let import_node = self.make_node(
                node,
                NodeKind::Import,
                &name,
                Some(import_text.to_string()),
                None,
            );
            self.nodes.push(import_node);
            self.walk_children(node, current_container, current_function);
            return;
        }

        if kind == "call" {
            if let Some(caller_id) = current_function.as_ref() {
                if let Some(function) = node.child_by_field_name("function") {
                    let called = call_target(function, self.source);
                    if !called.is_empty() && !is_python_builtin_or_keyword(&called) {
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
            language: Language::Python,
            start_line: (start.row + 1) as u32,
            end_line: (end.row + 1) as u32,
            start_column: start.column as u32,
            end_column: end.column as u32,
            docstring: None,
            signature,
            visibility,
            is_exported: !name.starts_with('_'),
            is_async: text(node, self.source).trim_start().starts_with("async "),
            is_static: false,
            is_abstract: false,
            decorators: None,
            type_parameters: None,
            updated_at: now_ms(),
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

fn import_name(import_text: &str) -> String {
    import_text
        .trim_start_matches("from ")
        .trim_start_matches("import ")
        .split_whitespace()
        .next()
        .unwrap_or(import_text)
        .trim_matches(',')
        .to_string()
}

fn call_target(function: TsNode<'_>, source: &str) -> String {
    let raw = text(function, source).trim();
    raw.rsplit(['.', ':'])
        .next()
        .unwrap_or(raw)
        .trim()
        .to_string()
}

fn is_python_builtin_or_keyword(name: &str) -> bool {
    matches!(
        name,
        "if" | "for"
            | "while"
            | "with"
            | "return"
            | "yield"
            | "print"
            | "len"
            | "range"
            | "str"
            | "int"
            | "float"
            | "list"
            | "dict"
            | "set"
            | "tuple"
            | "super"
    )
}
