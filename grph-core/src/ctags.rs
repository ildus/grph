use crate::db::Database;
use crate::errors::Result;
use crate::types::{Language, Node, NodeKind};
use std::path::Path;

/// Generate a Universal Ctags-compatible tags file from the indexed nodes.
///
/// We emit format=2 extended fields and line-number addresses. Line-number
/// addresses are accepted by vi/vim and avoid re-reading/parsing source lines
/// while still using the exact tree-sitter start lines recorded in the index.
pub struct CtagsGenerator {
    db: Database,
}

impl CtagsGenerator {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    pub fn generate_to_file(&self, path: &Path) -> Result<usize> {
        let content = self.generate()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content.as_bytes())?;
        Ok(content
            .lines()
            .filter(|line| !line.starts_with("!_TAG_"))
            .count())
    }

    pub fn generate(&self) -> Result<String> {
        let mut nodes = self
            .db
            .list_nodes_for_ctags()?
            .into_iter()
            .filter(is_tag_node)
            .collect::<Vec<_>>();

        nodes.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.file_path.cmp(&b.file_path))
                .then_with(|| a.start_line.cmp(&b.start_line))
        });

        let mut out = String::new();
        out.push_str(
            "!_TAG_FILE_FORMAT\t2\t/extended format; --format=1 will not append ;\" to lines/\n",
        );
        out.push_str("!_TAG_FILE_SORTED\t1\t/0=unsorted, 1=sorted, 2=foldcase/\n");
        out.push_str("!_TAG_PROGRAM_NAME\tgrph\t//\n");
        out.push_str("!_TAG_PROGRAM_VERSION\t0.3.0\t//\n");

        for node in &nodes {
            out.push_str(&format_tag_line(node));
            out.push('\n');
        }

        Ok(out)
    }
}

fn is_tag_node(node: &Node) -> bool {
    !matches!(
        node.kind,
        NodeKind::Import | NodeKind::Export | NodeKind::File | NodeKind::Module
    )
}

fn format_tag_line(node: &Node) -> String {
    let mut fields = vec![
        escape_field(&node.name),
        escape_field(&node.file_path),
        format!("{};\"", node.start_line.max(1)),
        kind_letter(node.kind).to_string(),
        format!("kind:{}", node.kind.as_str()),
        format!("line:{}", node.start_line),
        format!("end:{}", node.end_line),
        format!("language:{}", language_name(node.language)),
    ];

    if !node.qualified_name.is_empty() {
        fields.push(format!("qualified:{}", escape_field(&node.qualified_name)));
    }
    if let Some(signature) = node.signature.as_ref().and_then(clean_signature) {
        fields.push(format!("signature:{}", escape_field(&signature)));
    }

    fields.join("\t")
}

fn clean_signature(signature: &String) -> Option<String> {
    let cleaned = signature.trim().trim_end_matches('{').trim_end();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_string())
    }
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', " ")
        .replace('\r', " ")
        .replace('\n', " ")
}

fn kind_letter(kind: NodeKind) -> char {
    match kind {
        NodeKind::Class => 'c',
        NodeKind::Struct => 's',
        NodeKind::Interface => 'i',
        NodeKind::Trait | NodeKind::Protocol => 't',
        NodeKind::Function => 'f',
        NodeKind::Method => 'm',
        NodeKind::Property => 'p',
        NodeKind::Field => 'F',
        NodeKind::Variable => 'v',
        NodeKind::Constant => 'C',
        NodeKind::Enum => 'e',
        NodeKind::EnumMember => 'E',
        NodeKind::TypeAlias => 'T',
        NodeKind::Namespace | NodeKind::Module => 'n',
        NodeKind::Parameter => 'a',
        NodeKind::Component => 'x',
        NodeKind::File | NodeKind::Import | NodeKind::Export => 'u',
    }
}

fn language_name(language: Language) -> &'static str {
    match language {
        Language::C => "C",
        Language::Cpp => "C++",
        Language::JavaScript | Language::Jsx => "JavaScript",
        Language::TypeScript | Language::Tsx => "TypeScript",
        Language::Python => "Python",
        Language::Go => "Go",
        Language::Rust => "Rust",
        Language::Shell => "Sh",
        Language::Esqlc => "C",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_line_uses_universal_ctags_extended_fields() {
        let node = Node {
            id: "1".to_string(),
            kind: NodeKind::Function,
            name: "main".to_string(),
            qualified_name: "src/main.rs#main".to_string(),
            file_path: "src/main.rs".to_string(),
            language: Language::Rust,
            start_line: 10,
            end_line: 12,
            start_column: 0,
            end_column: 1,
            docstring: None,
            signature: Some("fn main() {".to_string()),
            visibility: None,
            is_exported: false,
            is_async: false,
            is_static: false,
            is_abstract: false,
            decorators: None,
            type_parameters: None,
            updated_at: 0,
        };

        let line = format_tag_line(&node);
        assert!(line.starts_with("main\tsrc/main.rs\t10;\"\tf\tkind:function"));
        assert!(line.contains("language:Rust"));
        assert!(line.contains("signature:fn main()"));
    }
}
