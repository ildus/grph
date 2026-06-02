use grph_core::{Node, NodeKind};
use lsp_types::{DocumentSymbol, Location, Position, Range, SymbolInformation, SymbolKind};
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub fn node_range(node: &Node) -> Range {
    Range {
        start: Position {
            line: node.start_line.saturating_sub(1),
            character: node.start_column,
        },
        end: Position {
            line: node.end_line.saturating_sub(1),
            character: node.end_column,
        },
    }
}

pub fn node_location(root: &Path, node: &Node) -> Option<Location> {
    Some(Location {
        uri: lsp_types::Uri::from_str(&path_to_uri(root, &node.file_path)).ok()?,
        range: node_range(node),
    })
}

#[allow(deprecated)]
pub fn node_to_document_symbol(node: &Node) -> DocumentSymbol {
    DocumentSymbol {
        name: node.name.clone(),
        detail: node
            .signature
            .clone()
            .or_else(|| Some(node.kind.as_str().to_string())),
        kind: symbol_kind(node.kind),
        tags: None,
        deprecated: None,
        range: node_range(node),
        selection_range: node_range(node),
        children: None,
    }
}

pub fn nodes_to_document_symbols(nodes: &[Node]) -> Vec<DocumentSymbol> {
    nodes
        .iter()
        .enumerate()
        .filter(|(index, _)| parent_index(nodes, *index).is_none())
        .map(|(index, _)| document_symbol_with_children(nodes, index))
        .collect()
}

fn document_symbol_with_children(nodes: &[Node], index: usize) -> DocumentSymbol {
    let mut symbol = node_to_document_symbol(&nodes[index]);
    let children: Vec<_> = nodes
        .iter()
        .enumerate()
        .filter(|(child_index, _)| parent_index(nodes, *child_index) == Some(index))
        .map(|(child_index, _)| document_symbol_with_children(nodes, child_index))
        .collect();
    if !children.is_empty() {
        symbol.children = Some(children);
    }
    symbol
}

fn parent_index(nodes: &[Node], child_index: usize) -> Option<usize> {
    nodes
        .iter()
        .enumerate()
        .filter(|(index, node)| *index != child_index && contains(node, &nodes[child_index]))
        .min_by_key(|(_, node)| {
            (
                node.end_line - node.start_line,
                node.end_column.saturating_sub(node.start_column),
            )
        })
        .map(|(index, _)| index)
}

fn contains(parent: &Node, child: &Node) -> bool {
    (parent.start_line < child.start_line
        || parent.start_line == child.start_line && parent.start_column <= child.start_column)
        && (parent.end_line > child.end_line
            || parent.end_line == child.end_line && parent.end_column >= child.end_column)
}

#[allow(deprecated)]
pub fn node_to_symbol_information(root: &Path, node: &Node) -> Option<SymbolInformation> {
    Some(SymbolInformation {
        name: node.name.clone(),
        kind: symbol_kind(node.kind),
        tags: None,
        deprecated: None,
        location: node_location(root, node)?,
        container_name: container_name(node),
    })
}

pub fn symbol_kind(kind: NodeKind) -> SymbolKind {
    match kind {
        NodeKind::File => SymbolKind::FILE,
        NodeKind::Module | NodeKind::Namespace => SymbolKind::MODULE,
        NodeKind::Class | NodeKind::Struct | NodeKind::Component => SymbolKind::CLASS,
        NodeKind::Interface | NodeKind::Trait | NodeKind::Protocol => SymbolKind::INTERFACE,
        NodeKind::Function => SymbolKind::FUNCTION,
        NodeKind::Method => SymbolKind::METHOD,
        NodeKind::Property => SymbolKind::PROPERTY,
        NodeKind::Field => SymbolKind::FIELD,
        NodeKind::Variable | NodeKind::Parameter => SymbolKind::VARIABLE,
        NodeKind::Constant => SymbolKind::CONSTANT,
        NodeKind::Enum => SymbolKind::ENUM,
        NodeKind::EnumMember => SymbolKind::ENUM_MEMBER,
        NodeKind::TypeAlias => SymbolKind::TYPE_PARAMETER,
        NodeKind::Import | NodeKind::Export => SymbolKind::NAMESPACE,
    }
}

pub fn uri_to_file_path(root: &Path, uri: &str) -> Option<String> {
    let raw = uri.strip_prefix("file://")?;
    let decoded = urlencoding::decode(raw).ok()?.to_string();
    let absolute = PathBuf::from(decoded);
    if let Ok(relative) = absolute.strip_prefix(root) {
        return Some(relative.to_string_lossy().replace('\\', "/"));
    }
    Some(absolute.to_string_lossy().replace('\\', "/"))
}

pub fn path_to_uri(root: &Path, file_path: &str) -> String {
    let path = if Path::new(file_path).is_absolute() {
        PathBuf::from(file_path)
    } else {
        root.join(file_path)
    };
    format!("file://{}", urlencoding::encode(&path.to_string_lossy())).replace("%2F", "/")
}

fn container_name(node: &Node) -> Option<String> {
    let prefix = node.qualified_name.strip_suffix(&node.name)?;
    let trimmed = prefix.trim_end_matches(['.', ':', '/', '#']);
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
