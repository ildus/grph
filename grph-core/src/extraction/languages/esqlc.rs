use crate::errors::Result;
use crate::extraction::languages::c;
use crate::extraction::tree_sitter::ExtractionResult;
use crate::types::{Edge, EdgeKind, Language, Node, NodeKind};
use regex::Regex;
use std::collections::HashSet;

/// ESQL/C and EQUEL/C extractor.
///
/// These dialects embed SQL (ESQL/C) or QUEL (EQUEL/C) inside standard C source:
///   ESQL/C (.sc):        `exec sql <statement>;` — may span multiple lines
///   EQUEL/C (.qsc,.qsh): `## <statement>`        — line-at-a-time QUEL
///
/// Strategy:
/// 1. Pre-process to C-compatible source (replace SQL lines with blanks/`//`)
/// 2. Delegate to C tree-sitter for function/struct/variable extraction
/// 3. Overlay SQL-specific analysis: table references, host variables, includes
pub fn extract(source: &str, file_path: &str) -> Result<ExtractionResult> {
    let is_quel = file_path.ends_with(".qsc") || file_path.ends_with(".qsh");

    // 1. Pre-process → valid C
    let c_source = preprocess_for_c(source, is_quel);

    // 2. Delegate to C tree-sitter extractor
    let mut result = c::extract(&c_source, file_path)?;

    // Mark all nodes as esqlc language
    for node in &mut result.nodes {
        node.language = Language::Esqlc;
    }

    // The C grammar can lose statement ownership in legacy K&R-style EQUEL/C
    // files that contain large preprocessor-heavy functions. In that case it
    // may still find the function definition, but report an end line that is
    // much too early and omit call edges for statements later in the function.
    // Recompute C function extents from braces in the preprocessed source, then
    // overlay a conservative textual call scan so callers in .qsc/.qsh files are
    // not missed.
    repair_function_ranges_from_braces(source, &c_source, is_quel, &mut result.nodes);
    extract_c_call_references(source, is_quel, &mut result);

    // 3. SQL overlay
    extract_sql_references(source, file_path, is_quel, &mut result);
    extract_declare_section(source, file_path, &mut result);
    extract_esql_includes(source, file_path, &mut result);

    Ok(result)
}

// ── Pre-processing ──────────────────────────────────────────────────────────

/// Produce a C-parser-friendly copy of the source. Line count is preserved.
fn preprocess_for_c(source: &str, is_quel: bool) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut in_exec_sql = false;

    for line in &lines {
        if is_quel {
            out.push(preprocess_equel_line_for_c(line));
            continue;
        }

        // ESQL: neutralise `exec sql ...` blocks
        if in_exec_sql {
            if line_has_trailing_semicolon(line) {
                in_exec_sql = false;
            }
            out.push(String::new());
            continue;
        }

        if line_starts_exec_sql(line) {
            if line_has_trailing_semicolon(line) {
                out.push(String::new());
            } else {
                in_exec_sql = true;
                out.push(String::new());
            }
            continue;
        }

        out.push((*line).to_string());
    }

    out.join("\n")
}

/// Convert one EQUEL line into C-compatible text while preserving line numbers.
///
/// The embedded grammars distinguish host-language C fragments from EQUEL
/// commands. Mirror that shape here: pass through host declarations, braces,
/// comments, and include directives; comment out database commands so
/// tree-sitter can parse the surrounding C normally.
fn preprocess_equel_line_for_c(line: &str) -> String {
    let Some(marker) = line.find("##") else {
        return line.to_string();
    };

    let (prefix, after_marker) = line.split_at(marker);
    let body = &after_marker[2..];
    let trimmed = body.trim_start();

    if trimmed.is_empty() {
        return replace_equel_marker_with_spaces(line, marker);
    }

    if trimmed.starts_with("include") {
        return format!("{prefix}#{}", body.trim_start());
    }

    if is_equel_host_c_fragment(trimmed) {
        return replace_equel_marker_with_spaces_and_normalized_body(
            line,
            marker,
            normalize_equel_host_c_fragment(body),
        );
    }

    replace_equel_marker_with_comment(line, marker)
}

fn replace_equel_marker_with_spaces_and_normalized_body(
    line: &str,
    marker: usize,
    body: String,
) -> String {
    let mut out = String::with_capacity(line.len().max(marker + 2 + body.len()));
    out.push_str(&line[..marker]);
    out.push_str("  ");
    out.push_str(&body);
    out
}

fn normalize_equel_host_c_fragment(body: &str) -> String {
    let leading = body.len() - body.trim_start().len();
    let trimmed = &body[leading..];
    if let Some(rest) = trimmed.strip_prefix("typedef struct") {
        let rest_trimmed = rest.trim_start();
        if rest_trimmed.starts_with('_')
            || rest_trimmed
                .chars()
                .next()
                .map_or(false, |ch| ch.is_ascii_alphabetic())
        {
            let after_tag = rest_trimmed
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                .map(|idx| &rest_trimmed[idx..])
                .unwrap_or("");
            return format!("{}typedef struct{}", &body[..leading], after_tag);
        }
    }
    body.to_string()
}

fn replace_equel_marker_with_spaces(line: &str, marker: usize) -> String {
    let mut out = String::with_capacity(line.len());
    out.push_str(&line[..marker]);
    out.push_str("  ");
    out.push_str(&line[marker + 2..]);
    out
}

fn replace_equel_marker_with_comment(line: &str, marker: usize) -> String {
    let mut out = String::with_capacity(line.len());
    out.push_str(&line[..marker]);
    out.push_str("//");
    out.push_str(&line[marker + 2..]);
    out
}

fn is_equel_host_c_fragment(trimmed: &str) -> bool {
    if matches!(trimmed, "{" | "}" | ";")
        || trimmed.starts_with('}')
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
        || trimmed.starts_with("*/")
        || trimmed.starts_with('#')
    {
        return true;
    }

    let head = first_word(trimmed).unwrap_or("");
    if head.is_empty() || is_equel_command_word(head) {
        return false;
    }

    trimmed.starts_with("typedef struct")
        || trimmed.starts_with("typedef enum")
        || trimmed.starts_with("typedef union")
        || trimmed.starts_with("struct ")
        || trimmed.starts_with("enum ")
        || trimmed.starts_with("union ")
        || (trimmed.contains(';')
            && (is_c_type_intro(head)
                || trimmed.contains("typedef")
                || trimmed.contains("struct")
                || trimmed.contains("enum")
                || trimmed.contains("union")))
}

fn first_word(text: &str) -> Option<&str> {
    text.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .find(|word| !word.is_empty())
}

fn is_c_type_intro(word: &str) -> bool {
    matches!(
        word,
        "auto"
            | "bool"
            | "char"
            | "const"
            | "double"
            | "enum"
            | "extern"
            | "float"
            | "GLOBALDEF"
            | "GLOBALREF"
            | "i1"
            | "i2"
            | "i4"
            | "i8"
            | "int"
            | "long"
            | "nat"
            | "PTR"
            | "register"
            | "short"
            | "signed"
            | "static"
            | "struct"
            | "typedef"
            | "u_i1"
            | "u_i2"
            | "u_i4"
            | "u_i8"
            | "union"
            | "unsigned"
            | "VOID"
            | "void"
    ) || word.chars().next().map_or(false, char::is_uppercase)
}

fn is_equel_command_word(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "append"
            | "call"
            | "clear"
            | "close"
            | "commit"
            | "copy"
            | "delete"
            | "display"
            | "enddata"
            | "endforms"
            | "fetch"
            | "finalize"
            | "flush"
            | "getform"
            | "getoper"
            | "help"
            | "include"
            | "ingres"
            | "inquire_equel"
            | "message"
            | "modify"
            | "open"
            | "prepare"
            | "print"
            | "prompt"
            | "putform"
            | "range"
            | "redisplay"
            | "repeat"
            | "replace"
            | "retrieve"
            | "rollback"
            | "run"
            | "save"
            | "set"
            | "sleep"
            | "unloadtable"
            | "validate"
    )
}

fn line_starts_exec_sql(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.len() >= 8
        && trimmed[..8].eq_ignore_ascii_case("exec sql")
        && (trimmed.len() == 8
            || trimmed.as_bytes()[8].is_ascii_whitespace()
            || trimmed.as_bytes()[8] == b';')
}

fn line_has_trailing_semicolon(line: &str) -> bool {
    let stripped = strip_trailing_comment(line);
    stripped.trim_end().ends_with(';')
}

fn strip_trailing_comment(line: &str) -> &str {
    if let Some(idx) = line.find("/*") {
        &line[..idx]
    } else {
        line
    }
}

/// Repair function end lines by counting braces in the C-compatible source.
///
/// Tree-sitter can recover from legacy embedded-C syntax by producing a
/// function_definition node with a truncated range. Later call extraction relies
/// on accurate ranges to determine the enclosing caller, so prefer the brace
/// extent when it clearly encloses more source than the parser reported.
fn repair_function_ranges_from_braces(
    source: &str,
    c_source: &str,
    is_quel: bool,
    nodes: &mut [Node],
) {
    let source_lines: Vec<&str> = source.lines().collect();
    let c_lines: Vec<&str> = c_source.lines().collect();

    for node in nodes.iter_mut() {
        if node.kind != NodeKind::Function && node.kind != NodeKind::Method {
            continue;
        }

        let start_idx = node.start_line.saturating_sub(1) as usize;
        if start_idx >= source_lines.len() {
            continue;
        }

        let search_end = (start_idx + 80).min(source_lines.len());
        let Some(open_idx) = (start_idx..search_end).find(|&idx| {
            line_has_body_open_brace(source_lines[idx], c_lines.get(idx).copied(), is_quel)
        }) else {
            continue;
        };

        let mut depth: i32 = 0;
        let mut saw_open = false;
        for (idx, line) in source_lines.iter().enumerate().skip(open_idx) {
            for ch in brace_scan_line(line, c_lines.get(idx).copied(), is_quel).chars() {
                match ch {
                    '{' => {
                        depth += 1;
                        saw_open = true;
                    }
                    '}' if saw_open => {
                        depth -= 1;
                        if depth == 0 {
                            let end_line = (idx + 1) as u32;
                            if end_line > node.end_line {
                                node.end_line = end_line;
                            }
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if saw_open && depth == 0 {
                break;
            }
        }
    }
}

fn line_has_body_open_brace(source_line: &str, c_line: Option<&str>, is_quel: bool) -> bool {
    if strip_c_line_noise(c_line.unwrap_or(source_line)).contains('{') {
        return true;
    }

    is_quel && source_line.trim_start().starts_with("##{")
}

fn brace_scan_line(source_line: &str, c_line: Option<&str>, is_quel: bool) -> String {
    if is_quel {
        let trimmed = source_line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("##") {
            if rest.trim() == "{" || rest.trim() == "}" {
                return rest.to_string();
            }
        }
    }

    strip_c_line_noise(c_line.unwrap_or(source_line))
}

/// Remove enough comments/string literals for simple brace and call scanning.
/// This intentionally preserves line-local code and does not try to be a full C
/// lexer; it is used only as an overlay after tree-sitter extraction.
fn strip_c_line_noise(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut in_string: Option<char> = None;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
                out.push(' ');
                continue;
            }
            if ch == '\\' {
                escaped = true;
                out.push(' ');
                continue;
            }
            if ch == quote {
                in_string = None;
            }
            out.push(' ');
            continue;
        }

        if ch == '"' || ch == '\'' {
            in_string = Some(ch);
            out.push(' ');
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'/') {
            break;
        }
        if ch == '/' && chars.peek() == Some(&'*') {
            break;
        }
        out.push(ch);
    }

    out
}

/// Add call edges found textually in embedded C source.
///
/// This complements the C parser for `.sc`, `.qsc`, and `.qsh` files. In
/// particular, historical EQUEL/C uses K&R function definitions plus `##` QUEL
/// lines; parser recovery may skip ordinary C calls after preprocessor branches.
fn extract_c_call_references(source: &str, is_quel: bool, result: &mut ExtractionResult) {
    let re = match Regex::new(r"\b([A-Za-z_][A-Za-z0-9_]*)\s*\(") {
        Ok(re) => re,
        Err(_) => return,
    };

    let lines: Vec<&str> = source.lines().collect();
    let mut seen: HashSet<(String, String, u32)> = result
        .edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::Calls)
        .map(|edge| {
            (
                edge.source.clone(),
                edge.target.clone(),
                edge.line.unwrap_or(0),
            )
        })
        .collect();

    for (i, line) in lines.iter().enumerate() {
        let line_num = (i + 1) as u32;
        let trimmed = line.trim_start();

        if trimmed.is_empty()
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
            || trimmed.starts_with("//")
            || trimmed.starts_with('#')
            || (is_quel && trimmed.starts_with("##"))
            || line_starts_exec_sql(line)
            || looks_like_c_declaration(trimmed)
        {
            continue;
        }

        let Some(caller_id) = resolve_owner_node(line_num, &result.nodes) else {
            continue;
        };

        let code = strip_c_line_noise(line);
        for caps in re.captures_iter(&code) {
            let name = match caps.get(1) {
                Some(m) => m.as_str(),
                None => continue,
            };

            if is_c_call_noise(name) {
                continue;
            }

            let key = (caller_id.clone(), name.to_string(), line_num);
            if !seen.insert(key) {
                continue;
            }

            result.edges.push(Edge {
                source: caller_id.clone(),
                target: name.to_string(),
                kind: EdgeKind::Calls,
                metadata: None,
                line: Some(line_num),
                col: caps.get(1).map(|m| m.start() as u32),
                provenance: Some("esqlc-call-overlay".to_string()),
            });
        }
    }
}

fn looks_like_c_declaration(trimmed: &str) -> bool {
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("typedef ")
        || lower.starts_with("struct ")
        || lower.starts_with("enum ")
        || lower.starts_with("union ")
        || lower.starts_with("extern ")
        || lower.starts_with("static ") && !trimmed.contains('(')
    {
        return true;
    }

    // K&R parameter declaration lines and prototypes/declarations that begin
    // with common C/Ingres scalar types. Calls in expressions generally do not
    // start with a type keyword.
    const TYPE_PREFIXES: &[&str] = &[
        "void",
        "char",
        "int",
        "float",
        "double",
        "long",
        "short",
        "unsigned",
        "signed",
        "bool",
        "size_t",
        "i1",
        "i2",
        "i4",
        "i8",
        "u_i1",
        "u_i2",
        "u_i4",
        "nat",
        "i4nat",
        "f4",
        "f8",
        "status",
        "db_status",
        "ptr",
    ];

    TYPE_PREFIXES.iter().any(|prefix| {
        lower == *prefix
            || lower.starts_with(&format!("{prefix} "))
            || lower.starts_with(&format!("{prefix}\t"))
            || lower.starts_with(&format!("{prefix}*"))
    })
}

fn is_c_call_noise(name: &str) -> bool {
    matches!(
        name,
        "if" | "for"
            | "while"
            | "switch"
            | "return"
            | "sizeof"
            | "defined"
            | "case"
            | "do"
            | "else"
            | "VOID"
            | "void"
            | "char"
            | "int"
            | "float"
            | "double"
            | "long"
            | "short"
            | "bool"
    )
}

// ── SQL overlay ─────────────────────────────────────────────────────────

/// SQL keywords that follow FROM/INTO/UPDATE but aren't table names.
const SQL_NOISE: &[&str] = &[
    "select",
    "where",
    "set",
    "values",
    "null",
    "not",
    "and",
    "or",
    "in",
    "is",
    "as",
    "on",
    "all",
    "distinct",
    "order",
    "group",
    "by",
    "having",
    "union",
    "exists",
    "case",
    "when",
    "then",
    "else",
    "end",
    "count",
    "sum",
    "max",
    "min",
    "avg",
    "upper",
    "lower",
    "trim",
    "length",
    "substr",
    "substring",
    "like",
    "between",
    "to",
    "for",
    "with",
    "session",
    "current",
    "user",
    "table",
    "view",
    "index",
    "unique",
    "primary",
    "key",
    "foreign",
    "references",
    "default",
    "check",
    "constraint",
    "smallint",
    "integer",
    "int",
    "float",
    "char",
    "varchar",
    "date",
    "money",
];

/// Extract table/relation references from SQL and QUEL statements.
fn extract_sql_references(
    source: &str,
    _file_path: &str,
    _is_quel: bool,
    result: &mut ExtractionResult,
) {
    let owner_node_id = resolve_owner_node(1, &result.nodes);
    if owner_node_id.is_none() {
        return;
    }
    let owner_node_id = owner_node_id.unwrap();

    // Table-bearing patterns: FROM tbl, INTO tbl, UPDATE tbl, etc.
    let patterns: &[(&str, &str)] = &[
        ("FROM", r"(?i)\bFROM\s+:?([a-zA-Z_][a-zA-Z0-9_]*)\b"),
        ("INTO", r"(?i)\bINTO\s+:?([a-zA-Z_][a-zA-Z0-9_]*)\b"),
        ("UPDATE", r"(?i)\bUPDATE\s+:?([a-zA-Z_][a-zA-Z0-9_]*)\b"),
        ("JOIN", r"(?i)\bJOIN\s+:?([a-zA-Z_][a-zA-Z0-9_]*)\b"),
        ("TABLE", r"(?i)\bTABLE\s+:?([a-zA-Z_][a-zA-Z0-9_]*)\b"),
        (
            "APPEND",
            r"(?i)\bAPPEND\s+TO\s+:?([a-zA-Z_][a-zA-Z0-9_]*)\b",
        ),
        (
            "DELETE",
            r"(?i)\bDELETE\s+FROM\s+:?([a-zA-Z_][a-zA-Z0-9_]*)\b",
        ),
    ];

    let compiled: Vec<(&str, Regex)> = patterns
        .iter()
        .filter_map(|(name, pat)| Regex::new(pat).ok().map(|re| (*name, re)))
        .collect();

    let mut seen = HashSet::new();

    let lines: Vec<&str> = source.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let is_esql_line = line.trim_start().to_lowercase().starts_with("exec sql");
        let is_quel_line = line.trim_start().starts_with("##");
        let is_continuation = !is_esql_line && !is_quel_line && is_sql_continuation(&lines, i);

        if !is_esql_line && !is_quel_line && !is_continuation {
            continue;
        }

        let enclosing =
            resolve_owner_node((i + 1) as u32, &result.nodes).unwrap_or(owner_node_id.clone());

        for (_name, re) in &compiled {
            for caps in re.captures_iter(line) {
                let raw = caps.get(1).unwrap().as_str();
                let name_lower = raw.to_lowercase();
                if SQL_NOISE.iter().any(|n| *n == name_lower) {
                    continue;
                }
                let key = format!("{i}:{name_lower}");
                if seen.contains(&key) {
                    continue;
                }
                seen.insert(key);

                result.edges.push(Edge {
                    source: enclosing.clone(),
                    target: raw.to_string(),
                    kind: EdgeKind::References,
                    metadata: None,
                    line: Some((i + 1) as u32),
                    col: Some(caps.get(0).unwrap().start() as u32),
                    provenance: Some("esqlc-overlay".to_string()),
                });
            }
        }
    }
}

/// Check if line i is a continuation of a multi-line exec sql block.
fn is_sql_continuation(lines: &[&str], i: usize) -> bool {
    let start = if i > 20 { i - 20 } else { 0 };
    for j in (start..i).rev() {
        let prev = lines[j];
        if prev.trim_start().to_lowercase().starts_with("exec sql") {
            // Check if a semicolon already closed it between j and i-1
            for k in j..i {
                if line_has_trailing_semicolon(lines[k])
                    && !lines[k].trim_start().to_lowercase().starts_with("exec sql")
                {
                    return false;
                }
            }
            return true;
        }
        if line_has_trailing_semicolon(prev)
            && !prev.trim_start().to_lowercase().starts_with("exec sql")
        {
            break;
        }
    }
    false
}

/// Extract host variables from EXEC SQL BEGIN/END DECLARE SECTION.
fn extract_declare_section(source: &str, file_path: &str, result: &mut ExtractionResult) {
    let lines: Vec<&str> = source.lines().collect();
    let mut in_section = false;

    for (i, line) in lines.iter().enumerate() {
        let lower = line.trim_start().to_lowercase();

        if lower.starts_with("exec sql begin declare section") {
            in_section = true;
            continue;
        }
        if lower.starts_with("exec sql end declare section") {
            in_section = false;
            continue;
        }
        if !in_section {
            continue;
        }

        // Match: [qualifiers] type [*] varname
        // Legacy embedded-SQL types plus standard C.
        let re = Regex::new(
            r"^\s+(?:unsigned|signed|long|short|const|static)\s+)*(?:char|int|float|double|i4|i2|i1|f4|f8|nat|longnat|bool|GLOBALDEF\s+\w+|\w+)\s*\**\s*([a-zA-Z_][a-zA-Z0-9_]*)\b"
        ).ok();

        let re = match re {
            Some(r) => r,
            None => continue,
        };

        if let Some(caps) = re.captures(line) {
            let var_name = caps.get(1).unwrap().as_str().to_string();
            let line_num = (i + 1) as u32;

            // Skip if already extracted by C parser (same name + line)
            if result
                .nodes
                .iter()
                .any(|n| n.name == var_name && n.start_line == line_num)
            {
                continue;
            }

            let id = crate::extraction::tree_sitter::generate_node_id(
                file_path, "variable", &var_name, line_num,
            );
            let enclosing = resolve_owner_node(line_num, &result.nodes);

            result.nodes.push(Node {
                id: id.clone(),
                kind: NodeKind::Variable,
                name: var_name.clone(),
                qualified_name: format!("{}::{}", file_path, var_name),
                file_path: file_path.to_string(),
                language: Language::Esqlc,
                start_line: line_num,
                end_line: line_num,
                start_column: 0,
                end_column: line.len() as u32,
                docstring: None,
                signature: Some(line.trim().to_string()),
                visibility: None,
                is_exported: false,
                is_async: false,
                is_static: false,
                is_abstract: false,
                decorators: None,
                type_parameters: None,
                updated_at: crate::extraction::tree_sitter::now_ms(),
            });

            if let Some(ref enc) = enclosing {
                result.edges.push(Edge {
                    source: enc.clone(),
                    target: id,
                    kind: EdgeKind::Contains,
                    metadata: None,
                    line: Some(line_num),
                    col: Some(0),
                    provenance: Some("esqlc-overlay".to_string()),
                });
            }
        }
    }
}

/// Emit import-kind references for EXEC SQL INCLUDE directives.
fn extract_esql_includes(source: &str, _file_path: &str, result: &mut ExtractionResult) {
    let file_node_id = result
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::File)
        .map(|n| n.id.clone());

    let lines: Vec<&str> = source.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        let lower = line.trim_start().to_lowercase();
        if !lower.starts_with("exec sql include") {
            continue;
        }

        // Extract the include name: exec sql include sqlca; or exec sql include <name>;
        let re = Regex::new(
            r#"(?i)^\s*exec\s+sql\s+include\s+(?:[<"']?([a-zA-Z_][a-zA-Z0-9_.]*)[>"']?)\s*;?"#,
        )
        .ok();

        let re = match re {
            Some(r) => r,
            None => continue,
        };

        if let Some(caps) = re.captures(line) {
            let include_name = caps.get(1).unwrap().as_str().to_string();
            let ref_from = file_node_id.clone().unwrap_or_else(|| {
                resolve_owner_node((i + 1) as u32, &result.nodes).unwrap_or_default()
            });

            if ref_from.is_empty() {
                continue;
            }

            result.edges.push(Edge {
                source: ref_from,
                target: include_name,
                kind: EdgeKind::Imports,
                metadata: None,
                line: Some((i + 1) as u32),
                col: Some(0),
                provenance: Some("esqlc-overlay".to_string()),
            });
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Find the id of the innermost function node that contains `line_num`.
fn resolve_owner_node(line_num: u32, nodes: &[Node]) -> Option<String> {
    // Prefer the tightest enclosing function
    let mut best: Option<&Node> = None;
    for node in nodes {
        if node.kind != NodeKind::Function && node.kind != NodeKind::Method {
            continue;
        }
        if node.start_line > line_num || node.end_line < line_num {
            continue;
        }
        if best.map_or(true, |b| {
            node.end_line - node.start_line < b.end_line - b.start_line
        }) {
            best = Some(node);
        }
    }
    if let Some(b) = best {
        return Some(b.id.clone());
    }

    nodes
        .iter()
        .find(|n| n.kind == NodeKind::File)
        .or_else(|| nodes.first())
        .map(|n| n.id.clone())
}
