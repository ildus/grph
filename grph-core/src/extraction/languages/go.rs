use crate::errors::Result;
use crate::extraction::tree_sitter::ExtractionResult;
use crate::types::{Language, NodeKind};

use super::tree_common::{extract_with_tree_sitter, KindMap, TreeConfig};

pub fn extract(source: &str, file_path: &str) -> Result<ExtractionResult> {
    extract_with_tree_sitter(
        source,
        file_path,
        TreeConfig {
            language: Language::Go,
            grammar: tree_sitter_go::LANGUAGE.into(),
            parser_name: "Go",
            container_kinds: &[KindMap {
                ts_kind: "type_spec",
                node_kind: NodeKind::Struct,
            }],
            function_kinds: &["function_declaration"],
            method_kinds: &["method_declaration"],
            import_kinds: &["import_declaration", "import_spec"],
            variable_kinds: &["var_spec", "const_spec", "short_var_declaration"],
            call_kinds: &["call_expression"],
        },
    )
}
