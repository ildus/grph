use crate::db::Database;
use crate::errors::Result;
use crate::resolution::builtins::is_builtin;
use crate::resolution::import_resolver::resolve_import;
use crate::types::{Edge, EdgeKind, Language};
use std::path::PathBuf;

pub struct ReferenceResolver {
    db: Database,
    project_root: PathBuf,
}

impl ReferenceResolver {
    pub fn new(db: Database, project_root: PathBuf) -> Self {
        Self { db, project_root }
    }

    /// Resolve all unresolved references after extraction.
    ///
    /// For each unresolved reference, try to find a matching node:
    /// 1. Same-file: look for a node with matching name in the same file
    /// 2. Cross-file: search across all indexed files
    /// 3. If found, update the edge target to the resolved node ID
    /// 4. If not found, leave as unresolved
    pub fn resolve_all(&mut self) -> Result<ResolutionResult> {
        let unresolved = self.db.get_unresolved_refs(10_000)?;
        self.resolve_refs(&unresolved)
    }

    /// Resolve unresolved references emitted by a single file re-index.
    pub fn resolve_file(&mut self, file_path: &str) -> Result<ResolutionResult> {
        let unresolved = self.db.get_unresolved_refs_for_file(file_path, 10_000)?;
        self.resolve_refs(&unresolved)
    }

    fn resolve_refs(
        &mut self,
        unresolved: &[crate::types::UnresolvedRef],
    ) -> Result<ResolutionResult> {
        let total = unresolved.len() as u64;
        let mut resolved = 0u64;
        let mut still_unresolved = 0u64;

        // Disable FK checks during edge updates
        self.db.conn().execute_batch("PRAGMA foreign_keys = OFF")?;

        for uref in unresolved {
            let language = Language::from_str(&uref.language).unwrap_or(Language::Python);
            let name = &uref.reference_name;

            // Skip builtins
            if is_builtin(name, language) {
                if let Some(id) = uref.id {
                    self.db.delete_unresolved_ref(id)?;
                }
                resolved += 1;
                continue;
            }

            // Try same-file resolution first
            let node_id = self
                .db
                .get_node_by_name(name, &uref.file_path)
                .ok()
                .flatten()
                .or_else(|| {
                    // Fallback: search across all files
                    self.db.get_node_by_name_any(name).ok().flatten()
                });

            if let Some(node) = node_id {
                // Update the edge target from name → node ID
                let edge_kind =
                    EdgeKind::from_str(&uref.reference_kind).unwrap_or(EdgeKind::References);
                let updated_edge = Edge {
                    source: uref.from_node_id.clone(),
                    target: node.id.clone(),
                    kind: edge_kind,
                    metadata: Some(serde_json::json!({
                        "resolvedBy": "cross-file",
                    })),
                    line: Some(uref.line),
                    col: Some(uref.col),
                    provenance: Some("tree-sitter+resolved-cross-file".to_string()),
                };

                // Update existing edge (change target from name to ID)
                self.db.conn().execute(
                    "UPDATE edges SET target = ?1, provenance = ?2, metadata = ?3
                     WHERE source = ?4 AND target = ?5 AND kind = ?6",
                    rusqlite::params![
                        updated_edge.target,
                        updated_edge.provenance,
                        updated_edge
                            .metadata
                            .as_ref()
                            .map(|m| serde_json::to_string(m).unwrap_or_default()),
                        updated_edge.source,
                        name, // old target was the name
                        updated_edge.kind.as_str(),
                    ],
                )?;

                if let Some(id) = uref.id {
                    self.db.delete_unresolved_ref(id)?;
                }
                resolved += 1;
            } else {
                still_unresolved += 1;
            }
        }

        self.db.conn().execute_batch("PRAGMA foreign_keys = ON")?;

        Ok(ResolutionResult {
            resolved,
            unresolved: still_unresolved,
            total,
        })
    }

    /// Try to find a symbol by name across the codebase
    pub fn resolve_name(
        &self,
        name: &str,
        file_path: &str,
        language: Language,
    ) -> Option<Vec<String>> {
        if is_builtin(name, language) {
            return Some(Vec::new());
        }

        // Search for exact matches first
        if let Ok(Some(node)) = self.db.get_node_by_name(name, file_path) {
            return Some(vec![node.id]);
        }

        // Search across all files
        if let Ok(nodes) = self.db.search_nodes(name, None, 20) {
            let ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
            if !ids.is_empty() {
                return Some(ids);
            }
        }

        None
    }

    /// Follow an import statement to its source file
    pub fn resolve_import(
        &self,
        import_path: &str,
        from_file: &str,
        language: Language,
    ) -> Option<PathBuf> {
        resolve_import(import_path, from_file, &self.project_root, language)
    }
}

#[derive(Debug, Clone)]
pub struct ResolutionResult {
    pub resolved: u64,
    pub unresolved: u64,
    pub total: u64,
}
