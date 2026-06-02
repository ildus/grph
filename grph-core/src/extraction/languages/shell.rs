use crate::errors::{GrphError, Result};
use crate::extraction::tree_sitter::{
    build_qualified_name, generate_node_id, now_ms, ExtractionResult,
};
use crate::types::{Edge, EdgeKind, Language, Node, NodeKind};
use tree_sitter::{Node as TsNode, Parser};

pub fn extract(source: &str, file_path: &str) -> Result<ExtractionResult> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::language())
        .map_err(|e| GrphError::Parse(format!("failed to load Bash grammar: {e}")))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| GrphError::Parse("Bash tree-sitter parse failed".to_string()))?;

    let file_node_id = generate_node_id(file_path, NodeKind::File.as_str(), file_path, 1);
    let mut ctx = ExtractCtx {
        source,
        file_path,
        file_node_id: file_node_id.clone(),
        nodes: vec![file_node(source, file_path, file_node_id)],
        edges: Vec::new(),
        errors: Vec::new(),
    };
    ctx.walk(tree.root_node(), None);

    Ok(ExtractionResult {
        nodes: ctx.nodes,
        edges: ctx.edges,
        errors: ctx.errors,
    })
}

struct ExtractCtx<'a> {
    source: &'a str,
    file_path: &'a str,
    file_node_id: String,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    errors: Vec<String>,
}

impl<'a> ExtractCtx<'a> {
    fn walk(&mut self, node: TsNode<'a>, current_function: Option<String>) {
        match node.kind() {
            "function_definition" => {
                if let Some(name) = function_name(node, self.source) {
                    let function = make_node(
                        self.file_path,
                        node,
                        NodeKind::Function,
                        &name,
                        Some(signature(node, self.source)),
                    );
                    let id = function.id.clone();
                    self.nodes.push(function);
                    self.walk_children(node, Some(id));
                    return;
                }
            }
            "command" => {
                if let Some(command) = command_name(node, self.source) {
                    if matches!(command.as_str(), "source" | ".") {
                        if let Some(path) = command_arg(node, self.source) {
                            let import = make_node(
                                self.file_path,
                                node,
                                NodeKind::Import,
                                &path,
                                Some(text(node, self.source).trim().to_string()),
                            );
                            let id = import.id.clone();
                            self.edges.push(make_edge(
                                &self.file_node_id,
                                &id,
                                EdgeKind::Imports,
                                node,
                            ));
                            self.nodes.push(import);
                        }
                    } else if let Some(caller_id) = current_function.as_ref() {
                        if !is_shell_builtin(&command) {
                            self.edges
                                .push(make_edge(caller_id, &command, EdgeKind::Calls, node));
                        }
                    }
                }
            }
            _ => {}
        }

        self.walk_children(node, current_function);
    }

    fn walk_children(&mut self, node: TsNode<'a>, current_function: Option<String>) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk(child, current_function.clone());
        }
    }
}

fn file_node(source: &str, file_path: &str, id: String) -> Node {
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
        language: Language::Shell,
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

fn make_node(
    file_path: &str,
    node: TsNode<'_>,
    kind: NodeKind,
    name: &str,
    signature: Option<String>,
) -> Node {
    let start = node.start_position();
    let end = node.end_position();
    Node {
        id: generate_node_id(file_path, kind.as_str(), name, (start.row + 1) as u32),
        kind,
        name: name.to_string(),
        qualified_name: build_qualified_name(file_path, name),
        file_path: file_path.to_string(),
        language: Language::Shell,
        start_line: (start.row + 1) as u32,
        end_line: (end.row + 1) as u32,
        start_column: start.column as u32,
        end_column: end.column as u32,
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

fn make_edge(source: &str, target: &str, kind: EdgeKind, node: TsNode<'_>) -> Edge {
    let start = node.start_position();
    Edge {
        source: source.to_string(),
        target: target.to_string(),
        kind,
        metadata: None,
        line: Some((start.row + 1) as u32),
        col: Some(start.column as u32),
        provenance: Some("tree-sitter".to_string()),
    }
}

fn function_name(node: TsNode<'_>, source: &str) -> Option<String> {
    node.child_by_field_name("name")
        .map(|n| text(n, source).trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| shallow_word(node, source))
}

fn command_name(node: TsNode<'_>, source: &str) -> Option<String> {
    node.child_by_field_name("name")
        .or_else(|| first_child_kind(node, "command_name"))
        .and_then(|n| shallow_word(n, source).or_else(|| Some(text(n, source).trim().to_string())))
        .map(clean_word)
        .filter(|s| !s.is_empty())
}

fn command_arg(node: TsNode<'_>, source: &str) -> Option<String> {
    let mut seen_name = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "command_name" | "word") {
            if !seen_name {
                seen_name = true;
                continue;
            }
            let arg = clean_word(text(child, source).trim().to_string());
            if !arg.is_empty() {
                return Some(arg);
            }
        }
    }
    None
}

fn first_child_kind<'a>(node: TsNode<'a>, kind: &str) -> Option<TsNode<'a>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| child.kind() == kind);
    found
}

fn shallow_word(node: TsNode<'_>, source: &str) -> Option<String> {
    if matches!(node.kind(), "word" | "command_name") {
        return Some(clean_word(text(node, source).trim().to_string())).filter(|s| !s.is_empty());
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "word" | "command_name") {
            return Some(clean_word(text(child, source).trim().to_string()))
                .filter(|s| !s.is_empty());
        }
    }
    None
}

fn clean_word(value: String) -> String {
    value.trim_matches(['"', '\'', '`']).to_string()
}

fn signature(node: TsNode<'_>, source: &str) -> String {
    text(node, source)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

fn text(node: TsNode<'_>, source: &str) -> String {
    node.utf8_text(source.as_bytes()).unwrap_or("").to_string()
}

fn is_shell_builtin(name: &str) -> bool {
    matches!(
        name,
        "cd" | "echo"
            | "export"
            | "local"
            | "readonly"
            | "shift"
            | "test"
            | "true"
            | "false"
            | "printf"
            | "read"
            | "return"
            | "source"
            | "set"
            | "unset"
            | "trap"
            | "type"
            | "ulimit"
            | "umask"
            | "wait"
    )
}
