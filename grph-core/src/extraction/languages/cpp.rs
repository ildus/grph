use crate::errors::Result;
use crate::extraction::tree_sitter::ExtractionResult;
use crate::types::{Language, NodeKind};

use super::tree_common::{extract_with_tree_sitter, KindMap, TreeConfig};

pub fn extract(source: &str, file_path: &str) -> Result<ExtractionResult> {
    extract_with_tree_sitter(
        source,
        file_path,
        TreeConfig {
            language: Language::Cpp,
            grammar: tree_sitter_cpp::language(),
            parser_name: "C++",
            container_kinds: &[
                KindMap {
                    ts_kind: "class_specifier",
                    node_kind: NodeKind::Class,
                },
                KindMap {
                    ts_kind: "struct_specifier",
                    node_kind: NodeKind::Struct,
                },
                KindMap {
                    ts_kind: "enum_specifier",
                    node_kind: NodeKind::Enum,
                },
                KindMap {
                    ts_kind: "namespace_definition",
                    node_kind: NodeKind::Namespace,
                },
            ],
            function_kinds: &["function_definition"],
            method_kinds: &[],
            import_kinds: &["preproc_include"],
            variable_kinds: &["init_declarator"],
            call_kinds: &["call_expression"],
        },
    )
}
