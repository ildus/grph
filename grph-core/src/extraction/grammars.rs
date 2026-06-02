use crate::types::Language;
use std::path::Path;

/// Detect language from file extension alone (no content inspection).
pub fn detect_language(file_path: &Path) -> Option<Language> {
    let ext = file_path.extension()?.to_str()?;
    match ext {
        "c" => Some(Language::C),
        "h" => Some(Language::C),
        "cpp" | "cc" | "cxx" | "c++" => Some(Language::Cpp),
        "hpp" | "hxx" | "h++" => Some(Language::Cpp),
        "py" | "pyw" | "pyi" => Some(Language::Python),
        "js" | "mjs" | "cjs" => Some(Language::JavaScript),
        "jsx" => Some(Language::Jsx),
        "ts" => Some(Language::TypeScript),
        "tsx" => Some(Language::Tsx),
        "go" => Some(Language::Go),
        "rs" => Some(Language::Rust),
        "sh" | "bash" => Some(Language::Shell),
        "qsc" | "qsh" => Some(Language::Esqlc),
        // .sc could be Scala or embedded SQL/C — content detection resolves it
        "sc" => Some(Language::Esqlc),
        _ => None,
    }
}

/// Content-based override for files with ambiguous extensions.
///
/// Called after extension detection to resolve:
/// - .sc  → `Esqlc` if the source contains `exec sql` (otherwise: None — skip)
/// - .sh/.bash are always `Shell`.
pub fn detect_language_with_content(file_path: &Path, source: &str) -> Option<Language> {
    let ext = file_path.extension()?.to_str()?;

    match ext {
        "sc" => {
            if looks_like_esqlc(source) {
                Some(Language::Esqlc)
            } else {
                None // Scala/SuperCollider/etc — skip
            }
        }
        _ => None,
    }
}

/// Heuristic: does a `.sc` file contain embedded SQL/C constructs?
fn looks_like_esqlc(source: &str) -> bool {
    let sample = &source[..source.len().min(8192)];
    sample
        .lines()
        .any(|line| line.trim_start().to_lowercase().starts_with("exec sql"))
}
