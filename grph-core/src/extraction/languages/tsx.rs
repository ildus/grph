use crate::errors::Result;
use crate::extraction::tree_sitter::ExtractionResult;
use crate::types::{Language, NodeKind};

use super::tree_common::{extract_with_tree_sitter, KindMap, TreeConfig};

pub fn extract(source: &str, file_path: &str) -> Result<ExtractionResult> {
    extract_with_tree_sitter(
        source,
        file_path,
        TreeConfig {
            language: Language::Tsx,
            grammar: tree_sitter_typescript::language_tsx(),
            parser_name: "TSX",
            container_kinds: &[
                KindMap {
                    ts_kind: "class_declaration",
                    node_kind: NodeKind::Class,
                },
                KindMap {
                    ts_kind: "interface_declaration",
                    node_kind: NodeKind::Interface,
                },
                KindMap {
                    ts_kind: "enum_declaration",
                    node_kind: NodeKind::Enum,
                },
                KindMap {
                    ts_kind: "type_alias_declaration",
                    node_kind: NodeKind::TypeAlias,
                },
                KindMap {
                    ts_kind: "jsx_element",
                    node_kind: NodeKind::Component,
                },
                KindMap {
                    ts_kind: "jsx_self_closing_element",
                    node_kind: NodeKind::Component,
                },
            ],
            function_kinds: &["function_declaration", "generator_function_declaration"],
            method_kinds: &["method_definition", "method_signature"],
            import_kinds: &["import_statement"],
            variable_kinds: &["variable_declarator"],
            call_kinds: &["call_expression", "new_expression"],
        },
    )
}
