use crate::errors::Result;
use crate::extraction::tree_sitter::ExtractionResult;
use crate::types::{Language, NodeKind};

use super::tree_common::{extract_with_tree_sitter, KindMap, TreeConfig};

pub fn extract(source: &str, file_path: &str) -> Result<ExtractionResult> {
    extract_with_tree_sitter(
        source,
        file_path,
        TreeConfig {
            language: Language::JavaScript,
            grammar: tree_sitter_javascript::LANGUAGE.into(),
            parser_name: "JavaScript",
            container_kinds: &[KindMap {
                ts_kind: "class_declaration",
                node_kind: NodeKind::Class,
            }],
            function_kinds: &["function_declaration", "generator_function_declaration"],
            method_kinds: &["method_definition"],
            import_kinds: &["import_statement"],
            variable_kinds: &["variable_declarator"],
            call_kinds: &["call_expression", "new_expression"],
        },
    )
}
