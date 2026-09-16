//! Query intent and compiler-diagnostic parsing for context ranking.
//!
//! Pure heuristics — no ML. Goal: turn noisy agent/compiler text into a small
//! set of high-precision focus signals (files, lines, symbols) and drop
//! generic type tokens that otherwise drown lexical/content retrieval.

use std::collections::HashSet;
use std::path::Path;

/// High-level intent inferred from the task string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryIntent {
    /// Find a named symbol / definition.
    Locate,
    /// Explain how something works / architecture.
    Explain,
    /// Compiler / type / prototype mismatch.
    DebugCompile,
    /// Runtime bug / crash / wrong behavior.
    DebugRuntime,
    /// Default catch-all.
    General,
}

impl QueryIntent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Locate => "locate",
            Self::Explain => "explain",
            Self::DebugCompile => "debug-compile",
            Self::DebugRuntime => "debug-runtime",
            Self::General => "general",
        }
    }
}

/// One compiler/tool diagnostic extracted from the query text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagnosticHint {
    pub file_path: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub symbols: Vec<String>,
    pub expected_type: Option<String>,
    pub actual_type: Option<String>,
}

/// Parsed view of a context query used by ranking.
#[derive(Debug, Clone)]
pub struct AnalyzedQuery {
    pub intent: QueryIntent,
    pub diagnostics: Vec<DiagnosticHint>,
    pub focus_files: Vec<String>,
    pub focus_lines: Vec<(String, u32)>,
    pub focus_symbols: Vec<String>,
    /// Terms safe for broad lexical/content search (generics removed when
    /// stronger signals exist).
    pub content_terms: Vec<String>,
    pub generic_type_terms: HashSet<String>,
    pub confidence_note: Option<String>,
}

impl AnalyzedQuery {
    pub fn analyze(query: &str, all_terms: &[String], symbols: &[String]) -> Self {
        let intent = detect_intent(query);
        let diagnostics = extract_diagnostics(query);
        let generic_type_terms = all_terms
            .iter()
            .filter(|t| is_generic_type_token(t))
            .cloned()
            .collect::<HashSet<_>>();

        let mut focus_files = Vec::new();
        let mut focus_lines = Vec::new();
        let mut focus_symbols = symbols.to_vec();

        for diag in &diagnostics {
            if let Some(path) = &diag.file_path {
                push_unique(&mut focus_files, normalize_path_hint(path));
                if let Some(line) = diag.line {
                    let key = (normalize_path_hint(path), line);
                    if !focus_lines.iter().any(|x| x == &key) {
                        focus_lines.push(key);
                    }
                }
            }
            for sym in &diag.symbols {
                push_unique_ci(&mut focus_symbols, sym.clone());
            }
        }

        // Path-like tokens that look like source files in the free text.
        for path in extract_path_hints(query) {
            push_unique(&mut focus_files, path);
        }

        let has_strong_focus = !focus_files.is_empty()
            || !focus_symbols.is_empty()
            || matches!(intent, QueryIntent::DebugCompile);

        let content_terms: Vec<String> = if has_strong_focus {
            all_terms
                .iter()
                .filter(|t| !is_generic_type_token(t) && !is_diagnostic_noise_token(t))
                .cloned()
                .collect()
        } else {
            all_terms
                .iter()
                .filter(|t| !is_diagnostic_noise_token(t))
                .cloned()
                .collect()
        };

        let confidence_note = if diagnostics.is_empty()
            && focus_symbols.is_empty()
            && content_terms.len() <= 1
            && matches!(intent, QueryIntent::General | QueryIntent::Explain)
        {
            Some(
                "Weak query signals — results may be broad. Prefer a symbol name, file path, or paste a compiler diagnostic."
                    .to_string(),
            )
        } else {
            None
        };

        Self {
            intent,
            diagnostics,
            focus_files,
            focus_lines,
            focus_symbols,
            content_terms,
            generic_type_terms,
            confidence_note,
        }
    }

    pub fn path_boost(&self, file_path: &str) -> f64 {
        let mut boost = 0.0;
        let path = file_path.replace('\\', "/");
        let lower = path.to_lowercase();
        for focus in &self.focus_files {
            let focus_n = focus.replace('\\', "/").to_lowercase();
            if lower == focus_n || lower.ends_with(&focus_n) || focus_n.ends_with(&lower) {
                // File match is a strong prior, but line proximity should still
                // dominate within that file for compile diagnostics.
                boost += if matches!(self.intent, QueryIntent::DebugCompile) {
                    55.0
                } else {
                    120.0
                };
            } else if path_basename_eq(&lower, &focus_n) {
                boost += if matches!(self.intent, QueryIntent::DebugCompile) {
                    45.0
                } else {
                    90.0
                };
            } else if shared_meaningful_segments(&lower, &focus_n) >= 2 {
                boost += 20.0;
            }
        }
        // Facility / sibling affinity: same leaf directory as a focus file.
        if boost == 0.0 {
            for focus in &self.focus_files {
                if same_parent_dir(&path, focus) {
                    boost += 18.0;
                    break;
                }
            }
        }
        boost
    }

    pub fn line_boost(&self, file_path: &str, start_line: u32, end_line: u32) -> f64 {
        let path = file_path.replace('\\', "/").to_lowercase();
        let mut best: f64 = 0.0;
        for (focus_path, line) in &self.focus_lines {
            let focus_n = focus_path.replace('\\', "/").to_lowercase();
            if !(path.ends_with(&focus_n)
                || focus_n.ends_with(&path)
                || path_basename_eq(&path, &focus_n))
            {
                continue;
            }
            let end = end_line.max(start_line);
            let score = if *line >= start_line && *line <= end {
                90.0
            } else {
                let dist = if *line < start_line {
                    start_line - *line
                } else {
                    *line - end
                };
                match dist {
                    0..=5 => 70.0,
                    6..=25 => 40.0,
                    26..=80 => 18.0,
                    _ => 0.0,
                }
            };
            best = best.max(score);
        }
        best
    }

    pub fn symbol_boost(&self, name: &str, qualified: &str) -> f64 {
        let name_l = name.to_lowercase();
        let qual_l = qualified.to_lowercase();
        for sym in &self.focus_symbols {
            let s = sym.to_lowercase();
            if name_l == s || qual_l == s || qual_l.ends_with(&format!(".{s}")) {
                return 100.0;
            }
            if name_l.contains(&s) || s.contains(&name_l) {
                return 45.0;
            }
        }
        0.0
    }

    /// True when a content hit only matched generic type tokens.
    pub fn is_generic_only_match(&self, matched_terms: &[String]) -> bool {
        !matched_terms.is_empty()
            && !self.generic_type_terms.is_empty()
            && matched_terms
                .iter()
                .all(|t| self.generic_type_terms.contains(t) || is_generic_type_token(t))
    }

    pub fn intent_kind_bias(&self, kind: crate::types::NodeKind) -> f64 {
        use crate::types::NodeKind;
        match self.intent {
            QueryIntent::DebugCompile => match kind {
                NodeKind::Function | NodeKind::Method => 12.0,
                // Headers often hold the mismatched prototype.
                NodeKind::File => 4.0,
                NodeKind::Variable | NodeKind::Field | NodeKind::Property => -8.0,
                _ => 0.0,
            },
            QueryIntent::Locate => match kind {
                NodeKind::Function
                | NodeKind::Method
                | NodeKind::Class
                | NodeKind::Struct
                | NodeKind::Interface => 8.0,
                _ => 0.0,
            },
            QueryIntent::Explain => match kind {
                NodeKind::Function | NodeKind::Method | NodeKind::Class | NodeKind::Module => 6.0,
                _ => 0.0,
            },
            _ => 0.0,
        }
    }
}

fn detect_intent(query: &str) -> QueryIntent {
    let q = query.to_lowercase();
    let compile_markers = [
        "error:",
        "warning:",
        "note:",
        "incompatible-pointer-types",
        "incompatible pointer",
        "expected '",
        "but argument is of type",
        "conflicting types",
        "undeclared",
        "implicit declaration",
        "no matching function",
        "cannot borrow",
        "mismatched types",
        "type mismatch",
        "prototype",
        "wincompatible",
        "undefined reference",
        "linker",
        "compile",
        "compilation",
    ];
    if compile_markers.iter().any(|m| q.contains(m))
        || (q.contains("expected") && q.contains("but") && q.contains("type"))
    {
        return QueryIntent::DebugCompile;
    }

    let runtime_markers = [
        "segfault",
        "sigsegv",
        "crash",
        "panic",
        "stack trace",
        "core dump",
        "null pointer",
        "use after free",
        "hang",
        "deadlock",
        "race",
        "wrong result",
        "incorrect result",
        "runtime",
    ];
    if runtime_markers.iter().any(|m| q.contains(m)) {
        return QueryIntent::DebugRuntime;
    }

    let locate_markers = [
        "where is",
        "where are",
        "find symbol",
        "find function",
        "locate ",
        "definition of",
        "who defines",
        "which file",
    ];
    if locate_markers.iter().any(|m| q.contains(m)) {
        return QueryIntent::Locate;
    }

    let explain_markers = [
        "how does",
        "how do ",
        "explain",
        "understand",
        "overview",
        "architecture",
        "walk through",
        "walkthrough",
        "what calls",
        "call flow",
        "data flow",
    ];
    if explain_markers.iter().any(|m| q.contains(m)) {
        return QueryIntent::Explain;
    }

    QueryIntent::General
}

fn extract_diagnostics(query: &str) -> Vec<DiagnosticHint> {
    let mut out = Vec::new();
    // path:line:col: or path:line:
    let loc_re = regex::Regex::new(
        r"(?P<path>(?:[A-Za-z]:)?(?:/|\\)?[\w./\\+-]+\.[A-Za-z0-9]+):(?P<line>\d+)(?::(?P<col>\d+))?",
    )
    .unwrap();
    let expected_re =
        regex::Regex::new(r"(?i)expected\s+'([^']+)'\s+but\s+argument\s+is\s+of\s+type\s+'([^']+)'")
            .unwrap();
    let expected_re2 =
        regex::Regex::new(r"(?i)expected\s+`([^`]+)`,\s*found\s+`([^`]+)`").unwrap();
    let note_arg_re = regex::Regex::new(
        r"(?i)note:\s*expected\s+'([^']+)'\s+but\s+argument\s+is\s+of\s+type\s+'([^']+)'",
    )
    .unwrap();

    let mut expected_type = None;
    let mut actual_type = None;
    for re in [&expected_re, &expected_re2, &note_arg_re] {
        if let Some(cap) = re.captures(query) {
            expected_type = Some(cap[1].to_string());
            actual_type = Some(cap[2].to_string());
            break;
        }
    }

    let mut symbols = Vec::new();
    // "passing argument N of 'Foo'" / "in call to `Foo`" / "In function 'Foo'"
    let sym_res = [
        regex::Regex::new(r"(?i)passing argument \d+ of ['`]([A-Za-z_][A-Za-z0-9_]*)['`]")
            .unwrap(),
        regex::Regex::new(r"(?i)in (?:call to|function) ['`]([A-Za-z_][A-Za-z0-9_]*)['`]")
            .unwrap(),
        regex::Regex::new(r"(?i)undefined reference to ['`]([A-Za-z_][A-Za-z0-9_]*)['`]")
            .unwrap(),
        regex::Regex::new(
            r"(?i)(?:undeclared identifier|undeclared)\s+['`]([A-Za-z_][A-Za-z0-9_]*)['`]",
        )
        .unwrap(),
    ];
    for re in &sym_res {
        for cap in re.captures_iter(query) {
            push_unique_ci(&mut symbols, cap[1].to_string());
        }
    }

    let mut seen_paths = HashSet::new();
    for cap in loc_re.captures_iter(query) {
        let path = normalize_path_hint(&cap["path"]);
        if !seen_paths.insert(path.clone()) {
            continue;
        }
        let line = cap.name("line").and_then(|m| m.as_str().parse().ok());
        let column = cap.name("col").and_then(|m| m.as_str().parse().ok());
        out.push(DiagnosticHint {
            file_path: Some(path),
            line,
            column,
            symbols: symbols.clone(),
            expected_type: expected_type.clone(),
            actual_type: actual_type.clone(),
        });
    }

    if out.is_empty()
        && (expected_type.is_some() || !symbols.is_empty() || query_looks_like_diagnostic(query))
    {
        out.push(DiagnosticHint {
            file_path: None,
            line: None,
            column: None,
            symbols,
            expected_type,
            actual_type,
        });
    }

    out
}

fn query_looks_like_diagnostic(query: &str) -> bool {
    let q = query.to_lowercase();
    q.contains("error:") || q.contains("warning:") || q.contains("note: expected")
}

fn extract_path_hints(query: &str) -> Vec<String> {
    // Match path-like tokens ending in common source extensions.
    let re = regex::Regex::new(
        r"(?:^|[\s\[\(\x22\x27`])((?:[\w.-]+/)+[\w.-]+\.(?:c|h|cc|cpp|hpp|rs|py|go|ts|tsx|js|jsx|java|sc|qsc))\b",
    )
    .unwrap();
    let mut out = Vec::new();
    for cap in re.captures_iter(query) {
        push_unique(&mut out, normalize_path_hint(&cap[1]));
    }
    out
}

fn normalize_path_hint(path: &str) -> String {
    path.trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .replace('\\', "/")
}

fn path_basename_eq(a: &str, b: &str) -> bool {
    Path::new(a).file_name() == Path::new(b).file_name()
}

fn same_parent_dir(a: &str, b: &str) -> bool {
    match (Path::new(a).parent(), Path::new(b).parent()) {
        (Some(pa), Some(pb)) => {
            pa.file_name().is_some()
                && pa.file_name() == pb.file_name()
                && pa.file_name().map(|n| n != "/").unwrap_or(false)
        }
        _ => false,
    }
}

fn shared_meaningful_segments(a: &str, b: &str) -> usize {
    let skip = |s: &str| {
        matches!(
            s,
            "" | "."
                | ".."
                | "src"
                | "lib"
                | "include"
                | "hdr"
                | "bin"
                | "build"
                | "target"
                | "ingres"
        )
    };
    let set_a: HashSet<_> = a
        .split('/')
        .filter(|s| !skip(s) && s.len() > 1)
        .collect();
    b.split('/')
        .filter(|s| !skip(s) && s.len() > 1 && set_a.contains(s))
        .count()
}

pub fn is_generic_type_token(token: &str) -> bool {
    matches!(
        token.to_lowercase().as_str(),
        "char"
            | "uchar"
            | "schar"
            | "void"
            | "int"
            | "i4"
            | "i2"
            | "i1"
            | "i8"
            | "u4"
            | "u2"
            | "u8"
            | "long"
            | "short"
            | "float"
            | "double"
            | "f8"
            | "f4"
            | "bool"
            | "boolean"
            | "byte"
            | "bytes"
            | "str"
            | "string"
            | "ptr"
            | "pointer"
            | "status"
            | "size"
            | "size_t"
            | "usize"
            | "isize"
            | "uint"
            | "const"
            | "static"
            | "struct"
            | "union"
            | "enum"
            | "typedef"
            | "unsigned"
            | "signed"
            | "aka"
            | "null"
            | "nullptr"
            | "none"
            | "some"
            | "option"
            | "result"
            | "vec"
            | "vector"
            | "map"
            | "hash"
            | "list"
            | "array"
            | "slice"
            | "ref"
            | "mut"
            | "box"
            | "arc"
            | "rc"
            | "self"
            | "super"
            | "crate"
            | "mod"
            | "impl"
            | "trait"
            | "type"
            | "types"
            | "typeof"
            | "typename"
            | "template"
            | "class"
            | "object"
            | "any"
            | "unknown"
            | "never"
            | "unit"
            | "true"
            | "false"
            | "ok"
            | "err"
            | "error"
            | "warning"
            | "note"
            | "info"
            | "argument"
            | "arguments"
            | "parameter"
            | "parameters"
            | "param"
            | "params"
            | "return"
            | "returns"
            | "expected"
            | "found"
            | "incompatible"
            | "compatible"
            | "mismatch"
            | "mismatched"
            | "passing"
            | "passed"
            | "prototype"
            | "declaration"
            | "definition"
            | "included"
            | "include"
            | "header"
            | "headers"
            | "compiler"
            | "gcc"
            | "clang"
            | "msvc"
    )
}

fn is_diagnostic_noise_token(token: &str) -> bool {
    matches!(
        token.to_lowercase().as_str(),
        "error"
            | "warning"
            | "note"
            | "fatal"
            | "failed"
            | "failure"
            | "line"
            | "column"
            | "col"
            | "file"
            | "from"
            | "included"
            | "incompatible"
            | "pointer"
            | "types"
            | "argument"
            | "expected"
            | "but"
            | "aka"
            | "passing"
            | "wincompatible"
    )
}

fn push_unique(out: &mut Vec<String>, value: String) {
    if !out.iter().any(|x| x == &value) {
        out.push(value);
    }
}

fn push_unique_ci(out: &mut Vec<String>, value: String) {
    if !out.iter().any(|x| x.eq_ignore_ascii_case(&value)) {
        out.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_compile_intent_from_gcc_note() {
        let q = "te.h:109:22: note: expected 'char *' but argument is of type 'const char *'";
        assert_eq!(detect_intent(q), QueryIntent::DebugCompile);
    }

    #[test]
    fn extracts_path_line_and_types() {
        let q = concat!(
            "In file included from /src/ingresx100/ingres/src/testtool/sep/sep/sepostcl.c:16:\n",
            "/src/ingresx100/ingres/src/gl/hdr/hdr/te.h:109:22: note: ",
            "expected 'char *' but argument is of type 'const char *'\n",
            "passing argument 1 of 'TEwrite'"
        );
        let diags = extract_diagnostics(q);
        assert!(diags.len() >= 2);
        assert!(diags.iter().any(|d| {
            d.file_path
                .as_deref()
                .is_some_and(|p| p.ends_with("te.h"))
                && d.line == Some(109)
        }));
        assert!(diags.iter().any(|d| {
            d.file_path
                .as_deref()
                .is_some_and(|p| p.ends_with("sepostcl.c"))
        }));
        let analyzed = AnalyzedQuery::analyze(
            q,
            &["char".into(), "tewrite".into(), "const".into(), "te".into()],
            &["TEwrite".into()],
        );
        assert_eq!(analyzed.intent, QueryIntent::DebugCompile);
        assert!(analyzed
            .focus_symbols
            .iter()
            .any(|s| s.eq_ignore_ascii_case("TEwrite")));
        assert!(!analyzed.content_terms.iter().any(|t| t == "char"));
        assert!(analyzed.content_terms.iter().any(|t| t == "tewrite"));
        let boost = analyzed.path_boost("gl/hdr/hdr/te.h");
        assert!(boost >= 40.0, "path boost was {boost}");
        assert!(analyzed.line_boost("gl/hdr/hdr/te.h", 100, 120) >= 70.0);
    }

    #[test]
    fn generic_only_match_detection() {
        let analyzed = AnalyzedQuery::analyze(
            "expected char * but argument is const char *",
            &["char".into(), "const".into(), "pointer".into()],
            &[],
        );
        assert!(analyzed.is_generic_only_match(&["char".into(), "const".into()]));
        assert!(!analyzed.is_generic_only_match(&["tewrite".into(), "char".into()]));
    }

    #[test]
    fn explain_vs_locate_intent() {
        assert_eq!(
            detect_intent("how does OLpcall work with hostCall"),
            QueryIntent::Explain
        );
        assert_eq!(
            detect_intent("where is hostCall defined"),
            QueryIntent::Locate
        );
    }
}
