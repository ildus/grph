use crate::types::{Language, Node};

/// Match a reference name against candidate nodes
pub fn match_reference(name: &str, candidates: &[Node], _language: Language) -> Vec<String> {
    // Exact match first
    for node in candidates {
        if node.name == name {
            return vec![node.id.clone()];
        }
    }

    // Case-insensitive match
    let lower_name = name.to_lowercase();
    let mut case_insensitive: Vec<&Node> = Vec::new();
    for node in candidates {
        if node.name.to_lowercase() == lower_name {
            case_insensitive.push(node);
        }
    }
    if !case_insensitive.is_empty() {
        return case_insensitive.iter().map(|n| n.id.clone()).collect();
    }

    // Suffix match (e.g., "UserService" matches "services/UserService.ts")
    let mut suffix_matches: Vec<&Node> = Vec::new();
    for node in candidates {
        if let Some(idx) = node.qualified_name.rfind('/') {
            let suffix = &node.qualified_name[idx + 1..];
            if suffix.contains(name) || name.contains(suffix) {
                suffix_matches.push(node);
            }
        }
    }
    if !suffix_matches.is_empty() {
        return suffix_matches.iter().map(|n| n.id.clone()).collect();
    }

    // No matches
    Vec::new()
}
