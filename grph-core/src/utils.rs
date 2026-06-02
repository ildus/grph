use std::path::Path;

/// Normalize a file path (replace backslashes, etc.)
pub fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Get the file extension in lowercase
pub fn get_extension(path: &Path) -> Option<String> {
    path.extension()?.to_str().map(|s| s.to_lowercase())
}

/// Check if a path is inside a directory
pub fn is_inside(path: &Path, directory: &Path) -> bool {
    path.starts_with(directory)
}

/// Get the relative path from a base directory
pub fn relative_to(path: &Path, base: &Path) -> Result<String, std::path::StripPrefixError> {
    path.strip_prefix(base)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
}

/// Truncate a string to a maximum length
pub fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

/// Format a byte size as a human-readable string
pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}
