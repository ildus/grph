pub mod c;
pub mod cpp;
pub mod esqlc;
pub mod go;
pub mod javascript;
pub mod python;
pub mod rust;
pub mod shell;
mod tree_common;
pub mod tsx;
pub mod typescript;

use crate::errors::Result;
use crate::extraction::tree_sitter::ExtractionResult;
use crate::types::{Edge, EdgeKind, Language, NodeKind};

/// Dispatch extraction based on language
pub fn extract_for_language(
    language: Language,
    source: &str,
    file_path: &str,
) -> Result<ExtractionResult> {
    match language {
        Language::C => c::extract(source, file_path),
        Language::Cpp => cpp::extract(source, file_path),
        Language::Python => python::extract(source, file_path),
        Language::JavaScript => javascript::extract(source, file_path),
        Language::TypeScript => typescript::extract(source, file_path),
        Language::Tsx => tsx::extract(source, file_path),
        Language::Jsx => javascript::extract(source, file_path),
        Language::Go => go::extract(source, file_path),
        Language::Rust => rust::extract(source, file_path),
        Language::Shell => shell::extract(source, file_path),
        Language::Esqlc => esqlc::extract(source, file_path),
    }
}

/// Regex-based extraction as the primary (and currently only) extraction method.
/// Detects: functions, classes, structs, traits, interfaces, methods, imports,
/// and basic call edges within each file.
pub fn extract_with_regex(
    source: &str,
    file_path: &str,
    language: Language,
) -> Result<ExtractionResult> {
    use crate::extraction::tree_sitter::{build_qualified_name, generate_node_id, now_ms};
    use crate::types::{Node, NodeKind};

    let mut nodes: Vec<Node> = Vec::new();
    let mut edges: Vec<Edge> = Vec::new();
    let errors: Vec<String> = Vec::new();

    let lines: Vec<&str> = source.lines().collect();
    let mut current_function_id: Option<String> = None;
    let mut current_class_id: Option<String> = None;

    for (i, line) in lines.iter().enumerate() {
        let line_num = (i + 1) as u32;
        let trimmed = line.trim();

        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with('#')
            || trimmed.starts_with("--")
        {
            continue;
        }

        // ── Classes / Structs / Traits / Interfaces ──
        let class_detected = detect_class(trimmed, language);
        if let Some((kind, name)) = class_detected {
            let id = generate_node_id(
                file_path,
                &format!("{:?}", kind).to_lowercase(),
                &name,
                line_num,
            );
            nodes.push(Node {
                id: id.clone(),
                kind,
                name: name.clone(),
                qualified_name: build_qualified_name(file_path, &name),
                file_path: file_path.to_string(),
                language,
                start_line: line_num,
                end_line: line_num,
                start_column: 0,
                end_column: 0,
                docstring: None,
                signature: Some(trimmed.to_string()),
                visibility: Some(
                    if trimmed.starts_with("pub ") || trimmed.starts_with("export ") {
                        "public"
                    } else {
                        "private"
                    }
                    .to_string(),
                ),
                is_exported: trimmed.starts_with("pub ") || trimmed.starts_with("export "),
                is_async: false,
                is_static: false,
                is_abstract: trimmed.contains("abstract")
                    || trimmed.contains("interface")
                    || trimmed.contains("trait"),
                decorators: None,
                type_parameters: None,
                updated_at: now_ms(),
            });
            current_class_id = Some(id);
            continue;
        }

        // ── Closing brace — reset context ──
        if trimmed == "}" || trimmed == "};" {
            current_function_id = None;
            // Don't reset class on single brace — class ends later
            if trimmed == "}" && current_class_id.is_some() && i > 0 {
                // Check if this is the class closing brace (not method)
                let _prev_indent = lines
                    .get(i.wrapping_sub(1))
                    .map(|l| l.len() - l.trim_start().len())
                    .unwrap_or(0);
                let this_indent = line.len() - trimmed.len();
                if this_indent == 0 {
                    current_class_id = None;
                }
            }
        }

        // ── Functions / Methods ──
        let func_detected = detect_function(trimmed, language);
        if let Some((kind, name, is_exported)) = func_detected {
            let id = generate_node_id(
                file_path,
                &format!("{:?}", kind).to_lowercase(),
                &name,
                line_num,
            );
            nodes.push(Node {
                id: id.clone(),
                kind,
                name: name.clone(),
                qualified_name: build_qualified_name(file_path, &name),
                file_path: file_path.to_string(),
                language,
                start_line: line_num,
                end_line: line_num,
                start_column: 0,
                end_column: 0,
                docstring: None,
                signature: Some(trimmed.to_string()),
                visibility: Some(if is_exported { "public" } else { "private" }.to_string()),
                is_exported,
                is_async: trimmed.contains("async"),
                is_static: (language == Language::Rust
                    && !trimmed.contains("self")
                    && !trimmed.contains("&self"))
                    || trimmed.contains("static"),
                is_abstract: trimmed.contains("abstract"),
                decorators: None,
                type_parameters: None,
                updated_at: now_ms(),
            });

            // Edge: class contains method
            if let Some(ref class_id) = current_class_id {
                edges.push(Edge {
                    source: class_id.clone(),
                    target: id.clone(),
                    kind: EdgeKind::Contains,
                    metadata: None,
                    line: Some(line_num),
                    col: Some(0),
                    provenance: Some("regex".to_string()),
                });
            }

            current_function_id = Some(id);
            continue;
        }

        // ── Import lines ──
        let import_detected = detect_import(trimmed, language);
        if let Some(ref import_name) = import_detected {
            let id = generate_node_id(file_path, "import", import_name, line_num);
            nodes.push(Node {
                id: id.clone(),
                kind: NodeKind::Import,
                name: import_name.clone(),
                qualified_name: build_qualified_name(file_path, import_name),
                file_path: file_path.to_string(),
                language,
                start_line: line_num,
                end_line: line_num,
                start_column: 0,
                end_column: 0,
                docstring: None,
                signature: Some(trimmed.to_string()),
                visibility: None,
                is_exported: false,
                is_async: false,
                is_static: false,
                is_abstract: false,
                decorators: None,
                type_parameters: None,
                updated_at: now_ms(),
            });
            continue;
        }

        // ── Variable / constant declarations ──
        let var_detected = detect_variable(trimmed, language);
        if let Some((kind, name)) = var_detected {
            let id = generate_node_id(
                file_path,
                &format!("{:?}", kind).to_lowercase(),
                &name,
                line_num,
            );
            nodes.push(Node {
                id: id.clone(),
                kind,
                name: name.clone(),
                qualified_name: build_qualified_name(file_path, &name),
                file_path: file_path.to_string(),
                language,
                start_line: line_num,
                end_line: line_num,
                start_column: 0,
                end_column: 0,
                docstring: None,
                signature: Some(trimmed.to_string()),
                visibility: None,
                is_exported: trimmed.starts_with("pub ") || trimmed.starts_with("export "),
                is_async: false,
                is_static: trimmed.contains("const") || trimmed.contains("static"),
                is_abstract: false,
                decorators: None,
                type_parameters: None,
                updated_at: now_ms(),
            });
            continue;
        }

        // ── Call edges: detect function calls on non-declaration lines ──
        if let Some(ref caller_id) = current_function_id {
            let calls = detect_calls(trimmed, language);
            for called_name in calls {
                // Don't create edges for keywords or control flow
                if is_keyword(&called_name, language) {
                    continue;
                }
                edges.push(Edge {
                    source: caller_id.clone(),
                    target: called_name,
                    kind: EdgeKind::Calls,
                    metadata: None,
                    line: Some(line_num),
                    col: Some(0),
                    provenance: Some("regex".to_string()),
                });
            }
        }
    }

    Ok(ExtractionResult {
        nodes,
        edges,
        errors,
    })
}

// ── Detection helpers ──

fn detect_class(line: &str, language: Language) -> Option<(NodeKind, String)> {
    let trimmed = line.trim();
    match language {
        Language::Rust | Language::Esqlc => {
            if let Some(rest) = trimmed.strip_prefix("pub struct ") {
                let name = rest
                    .split(|c: char| c == '<' || c == ' ' || c == '{')
                    .next()?
                    .to_string();
                return Some((NodeKind::Struct, name));
            }
            if let Some(rest) = trimmed.strip_prefix("struct ") {
                let name = rest
                    .split(|c: char| c == '<' || c == ' ' || c == '{')
                    .next()?
                    .to_string();
                return Some((NodeKind::Struct, name));
            }
            if let Some(rest) = trimmed.strip_prefix("pub trait ") {
                let name = rest
                    .split(|c: char| c == '<' || c == ' ' || c == '{')
                    .next()?
                    .to_string();
                return Some((NodeKind::Trait, name));
            }
            if let Some(rest) = trimmed.strip_prefix("trait ") {
                let name = rest
                    .split(|c: char| c == '<' || c == ' ' || c == '{')
                    .next()?
                    .to_string();
                return Some((NodeKind::Trait, name));
            }
            if trimmed.starts_with("pub enum ") || trimmed.starts_with("enum ") {
                let rest = if trimmed.starts_with("pub ") {
                    &trimmed[8..]
                } else {
                    &trimmed[5..]
                };
                let name = rest
                    .split(|c: char| c == '<' || c == ' ' || c == '{')
                    .next()?
                    .to_string();
                return Some((NodeKind::Enum, name));
            }
            if trimmed.starts_with("impl ") {
                let rest = &trimmed[5..];
                let name = rest
                    .split(|c: char| c == '<' || c == ' ' || c == '{')
                    .next()?
                    .to_string();
                return Some((NodeKind::Struct, format!("impl {}", name)));
            }
        }
        Language::Go => {
            if let Some(rest) = trimmed.strip_prefix("type ") {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                if parts.len() >= 3 && parts[1] == "struct" {
                    return Some((NodeKind::Struct, parts[0].to_string()));
                }
                if parts.len() >= 3 && parts[1] == "interface" {
                    return Some((NodeKind::Interface, parts[0].to_string()));
                }
            }
        }
        Language::C | Language::Cpp => {
            if trimmed.starts_with("struct ") || trimmed.starts_with("class ") {
                let rest = if trimmed.starts_with("struct ") {
                    &trimmed[7..]
                } else {
                    &trimmed[6..]
                };
                let name = rest
                    .split(|c: char| c == ':' || c == '{' || c == ';')
                    .next()?
                    .trim()
                    .to_string();
                if !name.is_empty() {
                    return Some((NodeKind::Struct, name));
                }
            }
        }
        Language::Python => {
            if let Some(rest) = trimmed.strip_prefix("class ") {
                let name = rest
                    .split(|c: char| c == '(' || c == ':')
                    .next()?
                    .trim()
                    .to_string();
                if !name.is_empty() {
                    return Some((NodeKind::Class, name));
                }
            }
        }
        Language::JavaScript | Language::TypeScript | Language::Tsx | Language::Jsx => {
            if let Some(rest) = trimmed.strip_prefix("export class ") {
                let name = rest
                    .split(|c: char| c == '<' || c == ' ' || c == '{' || c == 'e')
                    .next()?
                    .to_string();
                return Some((NodeKind::Class, name));
            }
            if let Some(rest) = trimmed.strip_prefix("class ") {
                let name = rest
                    .split(|c: char| c == '<' || c == ' ' || c == '{' || c == 'e')
                    .next()?
                    .to_string();
                return Some((NodeKind::Class, name));
            }
            if trimmed.starts_with("export interface ") || trimmed.starts_with("interface ") {
                let rest = if trimmed.starts_with("export ") {
                    &trimmed[16..]
                } else {
                    &trimmed[10..]
                };
                let name = rest
                    .split(|c: char| c == '<' || c == ' ' || c == '{')
                    .next()?
                    .to_string();
                return Some((NodeKind::Interface, name));
            }
        }
        Language::Shell => {}
    }
    None // all language variants covered above
}

fn detect_function(line: &str, language: Language) -> Option<(NodeKind, String, bool)> {
    let trimmed = line.trim();
    match language {
        Language::JavaScript | Language::TypeScript | Language::Tsx | Language::Jsx => {
            if let Some(rest) = trimmed.strip_prefix("export async function ") {
                let name = rest
                    .split(|c: char| c == '(' || c == '<')
                    .next()?
                    .to_string();
                return Some((NodeKind::Function, name, true));
            }
            if let Some(rest) = trimmed.strip_prefix("export function ") {
                let name = rest
                    .split(|c: char| c == '(' || c == '<')
                    .next()?
                    .to_string();
                return Some((NodeKind::Function, name, true));
            }
            if let Some(rest) = trimmed.strip_prefix("async function ") {
                let name = rest
                    .split(|c: char| c == '(' || c == '<')
                    .next()?
                    .to_string();
                return Some((NodeKind::Function, name, false));
            }
            if let Some(rest) = trimmed.strip_prefix("function ") {
                let name = rest
                    .split(|c: char| c == '(' || c == '<')
                    .next()?
                    .to_string();
                return Some((NodeKind::Function, name, false));
            }
            // Arrow functions assigned to const/let
            if (trimmed.starts_with("const ")
                || trimmed.starts_with("let ")
                || trimmed.starts_with("export const "))
                && trimmed.contains("=>")
            {
                let rest = if trimmed.starts_with("export ") {
                    &trimmed[13..]
                } else if trimmed.starts_with("const ") {
                    &trimmed[6..]
                } else {
                    &trimmed[4..]
                };
                let name = rest
                    .split(|c: char| c == '=' || c == ':')
                    .next()?
                    .trim()
                    .to_string();
                if !name.is_empty() && name.chars().next()?.is_lowercase() {
                    let is_exported = trimmed.starts_with("export ");
                    return Some((NodeKind::Function, name, is_exported));
                }
            }
            // Method in class: name(...) {
            if trimmed.contains('(')
                && trimmed.ends_with(") {")
                && !trimmed.starts_with("if ")
                && !trimmed.starts_with("for ")
                && !trimmed.starts_with("while ")
                && !trimmed.starts_with("switch ")
            {
                let name = trimmed.split('(').next()?.trim().to_string();
                let first_word = name.split_whitespace().last()?;
                if !first_word.is_empty() && first_word.chars().next()?.is_alphanumeric() {
                    return Some((NodeKind::Method, first_word.to_string(), false));
                }
            }
        }
        Language::Python => {
            if let Some(rest) = trimmed.strip_prefix("def ") {
                let name = rest.split('(').next()?.trim().to_string();
                if !name.is_empty() {
                    return Some((NodeKind::Function, name, false));
                }
            }
            if let Some(rest) = trimmed.strip_prefix("async def ") {
                let name = rest.split('(').next()?.trim().to_string();
                if !name.is_empty() {
                    return Some((NodeKind::Function, name, false));
                }
            }
        }
        Language::Go => {
            if let Some(rest) = trimmed.strip_prefix("func ") {
                let rest = rest.trim();
                // Handle receiver: func (r *Receiver) Name(...)
                let name = if rest.starts_with('(') {
                    if let Some(idx) = rest.find(')') {
                        rest[idx + 1..].trim().split('(').next()?.to_string()
                    } else {
                        return None;
                    }
                } else {
                    rest.split('(').next()?.to_string()
                };
                if !name.is_empty() {
                    let is_exported = name.chars().next().map_or(false, |c| c.is_uppercase());
                    return Some((NodeKind::Function, name, is_exported));
                }
            }
        }
        Language::Rust | Language::Esqlc => {
            let is_pub = trimmed.starts_with("pub ");
            let rest = if is_pub { &trimmed[4..] } else { trimmed };
            if let Some(rest) = rest.strip_prefix("fn ") {
                let name = rest
                    .split(|c: char| c == '(' || c == '<')
                    .next()?
                    .to_string();
                if !name.is_empty() {
                    return Some((NodeKind::Function, name, is_pub));
                }
            }
            if rest.starts_with("async fn ") {
                let rest = &rest[10..];
                let name = rest
                    .split(|c: char| c == '(' || c == '<')
                    .next()?
                    .to_string();
                if !name.is_empty() {
                    return Some((NodeKind::Function, name, is_pub));
                }
            }
        }
        Language::C | Language::Cpp => {
            // Match: return_type function_name(params)
            if trimmed.ends_with('{') || trimmed.ends_with("){") {
                let words: Vec<&str> = trimmed.split_whitespace().collect();
                if words.len() >= 2 {
                    let type_keywords = [
                        "void", "int", "char", "float", "double", "long", "short", "unsigned",
                        "signed", "static", "extern", "const", "bool", "auto", "size_t", "uint8_t",
                        "uint16_t", "uint32_t", "uint64_t", "int8_t", "int16_t", "int32_t",
                        "int64_t",
                    ];
                    let last = words.last()?;
                    let name = last.split('(').next()?.to_string();
                    // Filter out control statements
                    let control = ["if", "else", "for", "while", "switch", "do"];
                    if !control.contains(&name.as_str()) && !name.is_empty() {
                        let is_type = words[0] == "class"
                            || words[0] == "struct"
                            || type_keywords.contains(&words[0]);
                        if is_type {
                            return Some((NodeKind::Function, name, false));
                        }
                    }
                }
            }
        }
        Language::Shell => {}
    }
    None
}

fn detect_import(line: &str, language: Language) -> Option<String> {
    let trimmed = line.trim();
    match language {
        Language::JavaScript | Language::TypeScript | Language::Tsx | Language::Jsx => {
            if trimmed.starts_with("import ") {
                // Extract module name: import ... from "module"
                if let Some(idx) = trimmed.rfind("\"") {
                    let start = trimmed[..idx].rfind("\"")?;
                    return Some(trimmed[start + 1..idx].to_string());
                }
                if let Some(idx) = trimmed.rfind('\'') {
                    let start = trimmed[..idx].rfind('\'')?;
                    return Some(trimmed[start + 1..idx].to_string());
                }
            }
            if trimmed.starts_with("require(") {
                let inner = &trimmed[8..];
                if let Some(idx) = inner.rfind(')') {
                    return Some(
                        inner[..idx]
                            .trim_matches(|c| c == '\'' || c == '"')
                            .to_string(),
                    );
                }
            }
        }
        Language::Python => {
            if trimmed.starts_with("import ") {
                let module = &trimmed[7..].trim();
                let name = module.split(" as ").next()?.split('.').next()?.to_string();
                return Some(name);
            }
            if trimmed.starts_with("from ") {
                let rest = &trimmed[5..];
                let module = rest.split_whitespace().next()?;
                // Skip relative imports
                if module.starts_with('.') {
                    return Some(format!("relative:{}", module));
                }
                return Some(module.to_string());
            }
        }
        Language::Rust | Language::Esqlc => {
            if trimmed.starts_with("use ") {
                let rest = &trimmed[4..].trim_end_matches(';').trim();
                let parts: Vec<&str> = rest.split("::").collect();
                // First part is the crate name
                if !parts.is_empty() {
                    let crate_name = parts[0].to_string();
                    // Skip self/super/crate
                    if crate_name != "self" && crate_name != "super" && crate_name != "crate" {
                        return Some(crate_name);
                    }
                }
            }
        }
        Language::Go => {
            if trimmed.starts_with("import \"") {
                let inner = &trimmed[8..];
                let module = inner.trim_end_matches('"');
                return Some(format!("go:{}", module));
            }
            if trimmed.starts_with("import (") {
                return Some("go:import_block".to_string());
            }
        }
        Language::C | Language::Cpp => {
            if trimmed.starts_with("#include ") {
                let rest = &trimmed[9..].trim();
                let name = rest.trim_matches(|c: char| c == '"' || c == '<' || c == '>');
                return Some(format!("c:{}", name));
            }
        }
        Language::Shell => {
            if let Some(rest) = trimmed.strip_prefix("source ") {
                return Some(rest.split_whitespace().next()?.to_string());
            }
            if let Some(rest) = trimmed.strip_prefix(". ") {
                return Some(rest.split_whitespace().next()?.to_string());
            }
        }
    }
    None
}

fn detect_variable(line: &str, language: Language) -> Option<(NodeKind, String)> {
    let trimmed = line.trim();
    match language {
        Language::Rust | Language::Esqlc => {
            if let Some(rest) = trimmed.strip_prefix("let ") {
                let name = rest
                    .split(|c: char| c == ':' || c == '=' || c == ' ' || c == ';')
                    .next()?
                    .to_string();
                if !name.is_empty() {
                    return Some((NodeKind::Variable, name));
                }
            }
            if let Some(rest) = trimmed.strip_prefix("let mut ") {
                let name = rest
                    .split(|c: char| c == ':' || c == '=' || c == ' ' || c == ';')
                    .next()?
                    .to_string();
                if !name.is_empty() {
                    return Some((NodeKind::Variable, name));
                }
            }
            if let Some(rest) = trimmed.strip_prefix("const ") {
                let name = rest
                    .split(|c: char| c == ':' || c == '=' || c == ' ' || c == ';')
                    .next()?
                    .to_string();
                if !name.is_empty() && name.chars().next()?.is_uppercase() {
                    return Some((NodeKind::Constant, name));
                }
            }
            if let Some(rest) = trimmed.strip_prefix("static ") {
                let name = rest
                    .split(|c: char| c == ':' || c == '=' || c == ' ' || c == ';')
                    .next()?
                    .to_string();
                if !name.is_empty() {
                    return Some((NodeKind::Constant, name));
                }
            }
        }
        Language::JavaScript | Language::TypeScript | Language::Tsx | Language::Jsx => {
            if let Some(rest) = trimmed.strip_prefix("const ") {
                if !rest.contains('=') {
                    return None;
                }
                let name = rest
                    .split(|c: char| c == ':' || c == '=' || c == ' ' || c == ';')
                    .next()?
                    .to_string();
                if !name.is_empty() && !name.contains('(') {
                    let is_uppercase = name
                        .chars()
                        .all(|c| c.is_uppercase() || c == '_' || c.is_numeric());
                    return Some((
                        if is_uppercase {
                            NodeKind::Constant
                        } else {
                            NodeKind::Variable
                        },
                        name,
                    ));
                }
            }
            if let Some(rest) = trimmed.strip_prefix("let ") {
                let name = rest
                    .split(|c: char| c == ':' || c == '=' || c == ' ' || c == ';')
                    .next()?
                    .to_string();
                if !name.is_empty() && !name.contains('(') {
                    return Some((NodeKind::Variable, name));
                }
            }
            if let Some(rest) = trimmed.strip_prefix("var ") {
                let name = rest
                    .split(|c: char| c == ':' || c == '=' || c == ' ' || c == ';')
                    .next()?
                    .to_string();
                if !name.is_empty() {
                    return Some((NodeKind::Variable, name));
                }
            }
        }
        Language::Python => {
            if let Some(rest) = trimmed.strip_prefix("    ") {
                if rest.contains('=')
                    && !rest.contains("==")
                    && !rest.contains("!=")
                    && !rest.contains("+=")
                {
                    let name = rest.split('=').next()?.trim().to_string();
                    if name.chars().next()?.is_lowercase()
                        || name.chars().next()?.is_uppercase()
                            && name.chars().all(|c| c.is_uppercase() || c == '_')
                    {
                        return Some((
                            if name.chars().all(|c| c.is_uppercase() || c == '_') {
                                NodeKind::Constant
                            } else {
                                NodeKind::Variable
                            },
                            name,
                        ));
                    }
                }
            }
        }
        _ => {}
    }
    None
}

/// Detect function calls in a non-declaration line
fn detect_calls(line: &str, language: Language) -> Vec<String> {
    let mut calls = Vec::new();
    let trimmed = line.trim();

    // Skip comment, import, declaration lines
    if trimmed.starts_with("//")
        || trimmed.starts_with('#')
        || trimmed.starts_with("import ")
        || trimmed.starts_with("from ")
        || trimmed.starts_with("use ")
        || trimmed.starts_with("func ")
        || trimmed.starts_with("fn ")
        || trimmed.starts_with("def ")
        || trimmed.starts_with("function ")
        || trimmed.starts_with("class ")
        || trimmed.starts_with("struct ")
        || trimmed.starts_with("let ")
        || trimmed.starts_with("const ")
        || trimmed.starts_with("var ")
        || trimmed.starts_with("#include ")
    {
        return calls;
    }

    // Simple pattern: identifier followed by (
    // This is crude but catches most calls
    let chars: Vec<char> = trimmed.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '(' && i > 0 {
            // Find the start of the identifier before (
            let mut j = i;
            while j > 0
                && (chars[j - 1].is_alphanumeric() || chars[j - 1] == '_' || chars[j - 1] == '.')
            {
                j -= 1;
            }
            if j < i {
                let raw = &chars[j..i].iter().collect::<String>();
                // Take the last part after a dot (method call)
                let name = if let Some(dot_pos) = raw.rfind('.') {
                    raw[dot_pos + 1..].to_string()
                } else {
                    raw.clone()
                };
                if !name.is_empty()
                    && name
                        .chars()
                        .next()
                        .map_or(false, |c| c.is_alphabetic() || c == '_')
                {
                    // Filter out keywords pretending to be calls
                    calls.push(name);
                }
            }
        }
        i += 1;
    }

    // Also catch macro! invocations in Rust
    if language == Language::Rust {
        let mut i = 0;
        while i < chars.len().saturating_sub(1) {
            if chars[i] == '!' && chars[i + 1] == '(' && i > 0 {
                let mut j = i;
                while j > 0 && (chars[j - 1].is_alphanumeric() || chars[j - 1] == '_') {
                    j -= 1;
                }
                if j < i {
                    let name: String = chars[j..i].iter().collect();
                    calls.push(format!("{}!", name));
                }
            }
            i += 1;
        }
    }

    calls
}

fn is_keyword(name: &str, _language: Language) -> bool {
    let keywords = [
        "if",
        "else",
        "for",
        "while",
        "switch",
        "return",
        "break",
        "continue",
        "match",
        "case",
        "default",
        "throw",
        "try",
        "catch",
        "finally",
        "new",
        "delete",
        "typeof",
        "instanceof",
        "in",
        "of",
        "println",
        "print",
        "assert",
        "len",
        "append",
        "range",
        "Some",
        "None",
        "Ok",
        "Err",
        "true",
        "false",
        "nil",
        "null",
    ];
    keywords.contains(&name)
}
