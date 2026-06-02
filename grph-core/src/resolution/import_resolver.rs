use crate::types::Language;
use std::path::{Path, PathBuf};

/// Resolve an import path to a filesystem path
pub fn resolve_import(
    import_path: &str,
    from_file: &str,
    project_root: &Path,
    language: Language,
) -> Option<PathBuf> {
    match language {
        Language::JavaScript | Language::TypeScript | Language::Tsx | Language::Jsx => {
            resolve_js_import(import_path, from_file, project_root)
        }
        Language::Python => resolve_python_import(import_path, from_file, project_root),
        Language::Go => resolve_go_import(import_path, from_file, project_root),
        Language::Rust => resolve_rust_import(import_path, from_file, project_root),
        Language::C | Language::Cpp | Language::Esqlc => {
            resolve_c_include(import_path, from_file, project_root)
        }
        Language::Shell => resolve_shell_source(import_path, from_file, project_root),
    }
}

fn resolve_shell_source(
    import_path: &str,
    from_file: &str,
    _project_root: &Path,
) -> Option<PathBuf> {
    if import_path.starts_with('.') || import_path.starts_with('/') {
        let from_dir = PathBuf::from(from_file).parent()?.to_path_buf();
        Some(from_dir.join(import_path))
    } else {
        None
    }
}

fn resolve_js_import(import_path: &str, from_file: &str, _project_root: &Path) -> Option<PathBuf> {
    if import_path.starts_with('.') || import_path.starts_with('/') {
        // Relative import
        let from_dir = PathBuf::from(from_file).parent()?.to_path_buf();
        let resolved = from_dir.join(import_path);
        let resolved = resolve_js_extension(&resolved)?;
        Some(resolved)
    } else {
        // Module import - would need node_modules resolution
        // For now, return None
        None
    }
}

fn resolve_python_import(
    import_path: &str,
    from_file: &str,
    _project_root: &Path,
) -> Option<PathBuf> {
    if import_path.starts_with('.') {
        // Relative import
        let from_dir = PathBuf::from(from_file).parent()?.to_path_buf();
        let resolved = from_dir.join(&import_path[1..]);
        let resolved = resolve_py_extension(&resolved)?;
        Some(resolved)
    } else {
        // Absolute import - would need sys.path resolution
        None
    }
}

fn resolve_go_import(import_path: &str, from_file: &str, project_root: &Path) -> Option<PathBuf> {
    if import_path.starts_with('.') || import_path.starts_with('/') {
        let from_dir = PathBuf::from(from_file).parent()?.to_path_buf();
        let resolved = from_dir.join(import_path);
        Some(resolved)
    } else if import_path.starts_with(project_root.to_string_lossy().as_ref()) {
        // Local module
        Some(PathBuf::from(import_path))
    } else {
        // External module - would need GOPATH/module resolution
        None
    }
}

fn resolve_rust_import(
    import_path: &str,
    from_file: &str,
    _project_root: &Path,
) -> Option<PathBuf> {
    if import_path.starts_with("crate::") || import_path.starts_with("crate.") {
        let rest = &import_path[7..];
        let from_dir = PathBuf::from(from_file).parent()?.to_path_buf();
        let resolved = from_dir.join(rest.replace("::", "/"));
        resolve_rs_extension(&resolved)
    } else if import_path.starts_with("super::") || import_path.starts_with("super.") {
        let rest = &import_path[7..];
        let from_dir = PathBuf::from(from_file).parent()?.to_path_buf();
        let resolved = from_dir.join(rest.replace("::", "/"));
        resolve_rs_extension(&resolved)
    } else if import_path.starts_with("self::") || import_path.starts_with("self.") {
        let rest = &import_path[6..];
        let from_dir = PathBuf::from(from_file).parent()?.to_path_buf();
        let resolved = from_dir.join(rest.replace("::", "/"));
        resolve_rs_extension(&resolved)
    } else {
        // External crate - would need Cargo.toml resolution
        None
    }
}

fn resolve_c_include(import_path: &str, from_file: &str, _project_root: &Path) -> Option<PathBuf> {
    if import_path.starts_with('"') {
        // Local include
        let name = import_path.trim_matches('"');
        let from_dir = PathBuf::from(from_file).parent()?.to_path_buf();
        let resolved = from_dir.join(name);
        Some(resolved)
    } else {
        // System include - would need compiler include paths
        None
    }
}

fn resolve_js_extension(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        Some(path.to_path_buf())
    } else if path.with_extension("js").exists() {
        Some(path.with_extension("js"))
    } else if path.with_extension("ts").exists() {
        Some(path.with_extension("ts"))
    } else if path.with_extension("tsx").exists() {
        Some(path.with_extension("tsx"))
    } else if path.join("index.js").exists() {
        Some(path.join("index.js"))
    } else if path.join("index.ts").exists() {
        Some(path.join("index.ts"))
    } else {
        None
    }
}

fn resolve_py_extension(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        Some(path.to_path_buf())
    } else if path.with_extension("py").exists() {
        Some(path.with_extension("py"))
    } else if path.join("__init__.py").exists() {
        Some(path.join("__init__.py"))
    } else {
        None
    }
}

fn resolve_rs_extension(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        Some(path.to_path_buf())
    } else if path.with_extension("rs").exists() {
        Some(path.with_extension("rs"))
    } else {
        None
    }
}
