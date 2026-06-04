use crate::db::connection::Database;
use crate::errors::Result;
use crate::types::{Edge, EdgeKind, FileRecord, Node, NodeKind, UnresolvedRef, UnresolvedRefGroup};
use rusqlite::{params, OptionalExtension};
use std::collections::HashSet;

impl Database {
    // ==================== Node operations ====================

    pub fn insert_node(&self, node: &Node) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO nodes (
                id, kind, name, qualified_name, file_path, language,
                start_line, end_line, start_column, end_column,
                docstring, signature, visibility,
                is_exported, is_async, is_static, is_abstract,
                decorators, type_parameters, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
            params![
                node.id,
                node.kind.as_str(),
                &node.name,
                &node.qualified_name,
                &node.file_path,
                node.language.as_str(),
                node.start_line,
                node.end_line,
                node.start_column,
                node.end_column,
                node.docstring.as_deref(),
                node.signature.as_deref(),
                node.visibility.as_deref(),
                node.is_exported as i32,
                node.is_async as i32,
                node.is_static as i32,
                node.is_abstract as i32,
                node.decorators.as_ref().map(|v| serde_json::to_string(v).ok()).flatten(),
                node.type_parameters.as_ref().map(|v| serde_json::to_string(v).ok()).flatten(),
                node.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn list_nodes_for_ctags(&self) -> Result<Vec<Node>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, kind, name, qualified_name, file_path, language,
                    start_line, end_line, start_column, end_column,
                    docstring, signature, visibility,
                    is_exported, is_async, is_static, is_abstract,
                    decorators, type_parameters, updated_at
             FROM nodes
             ORDER BY name, file_path, start_line",
        )?;

        let nodes = stmt
            .query_map([], Self::row_to_node)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(nodes)
    }

    pub fn get_node_by_id(&self, id: &str) -> Result<Option<Node>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, kind, name, qualified_name, file_path, language,
                    start_line, end_line, start_column, end_column,
                    docstring, signature, visibility,
                    is_exported, is_async, is_static, is_abstract,
                    decorators, type_parameters, updated_at
             FROM nodes WHERE id = ?1",
        )?;

        let node = stmt.query_row(params![id], Self::row_to_node).optional()?;
        Ok(node)
    }

    pub fn list_nodes_by_file(&self, file_path: &str) -> Result<Vec<Node>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, kind, name, qualified_name, file_path, language,
                    start_line, end_line, start_column, end_column,
                    docstring, signature, visibility,
                    is_exported, is_async, is_static, is_abstract,
                    decorators, type_parameters, updated_at
             FROM nodes
             WHERE file_path = ?1
             ORDER BY start_line, start_column, end_line DESC",
        )?;

        let nodes = stmt
            .query_map(params![file_path], Self::row_to_node)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(nodes)
    }

    pub fn get_node_at_position(
        &self,
        file_path: &str,
        line: u32,
        column: u32,
    ) -> Result<Option<Node>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, kind, name, qualified_name, file_path, language,
                    start_line, end_line, start_column, end_column,
                    docstring, signature, visibility,
                    is_exported, is_async, is_static, is_abstract,
                    decorators, type_parameters, updated_at
             FROM nodes
             WHERE file_path = ?1
               AND start_line <= ?2
               AND end_line >= ?2
               AND (start_line < ?2 OR start_column <= ?3)
               AND (end_line > ?2 OR end_column >= ?3)
             ORDER BY (end_line - start_line) ASC, start_line DESC, start_column DESC
             LIMIT 1",
        )?;

        let node = stmt
            .query_row(params![file_path, line, column], Self::row_to_node)
            .optional()?;
        Ok(node)
    }

    pub fn get_nodes_by_kind(&self, kind: NodeKind, limit: u32) -> Result<Vec<Node>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, kind, name, qualified_name, file_path, language,
                    start_line, end_line, start_column, end_column,
                    docstring, signature, visibility,
                    is_exported, is_async, is_static, is_abstract,
                    decorators, type_parameters, updated_at
             FROM nodes WHERE kind = ?1 LIMIT ?2",
        )?;

        let nodes = stmt
            .query_map(params![kind.as_str(), limit], Self::row_to_node)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(nodes)
    }

    pub fn find_uncalled_functions(&self, limit: u32) -> Result<Vec<Node>> {
        let mut stmt = self.conn().prepare(
            "SELECT n.id, n.kind, n.name, n.qualified_name, n.file_path, n.language,
                    n.start_line, n.end_line, n.start_column, n.end_column,
                    n.docstring, n.signature, n.visibility,
                    n.is_exported, n.is_async, n.is_static, n.is_abstract,
                    n.decorators, n.type_parameters, n.updated_at
             FROM nodes n
             WHERE n.kind = 'function'
               AND NOT EXISTS (
                   SELECT 1
                   FROM edges e
                   WHERE e.target = n.id
                     AND e.kind = 'calls'
               )
               AND NOT EXISTS (
                   SELECT 1
                   FROM edges e
                   WHERE e.target = n.name
                     AND e.kind = 'calls'
               )
             ORDER BY n.file_path, n.start_line
             LIMIT ?1",
        )?;

        let nodes = stmt
            .query_map(params![limit], Self::row_to_node)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(nodes)
    }

    pub fn search_nodes(
        &self,
        query: &str,
        kind: Option<NodeKind>,
        limit: u32,
    ) -> Result<Vec<Node>> {
        // Use ranked LIKE-based search (FTS5 virtual table has issues with content sync).
        self.search_nodes_like(query, kind, limit)
    }

    pub fn search_nodes_fts(
        &self,
        query: &str,
        kind: Option<NodeKind>,
        limit: u32,
    ) -> Result<Vec<Node>> {
        let sql = if let Some(_k) = kind {
            format!(
                "SELECT n.id, n.kind, n.name, n.qualified_name, n.file_path, n.language,
                        n.start_line, n.end_line, n.start_column, n.end_column,
                        n.docstring, n.signature, n.visibility,
                        n.is_exported, n.is_async, n.is_static, n.is_abstract,
                        n.decorators, n.type_parameters, n.updated_at
                 FROM nodes n
                 INNER JOIN nodes_fts fts ON n.id = fts.id
                 WHERE nodes_fts MATCH ?1
                   AND n.kind = ?2
                 ORDER BY bm25(fts) LIMIT ?3",
            )
        } else {
            format!(
                "SELECT n.id, n.kind, n.name, n.qualified_name, n.file_path, n.language,
                        n.start_line, n.end_line, n.start_column, n.end_column,
                        n.docstring, n.signature, n.visibility,
                        n.is_exported, n.is_async, n.is_static, n.is_abstract,
                        n.decorators, n.type_parameters, n.updated_at
                 FROM nodes n
                 INNER JOIN nodes_fts fts ON n.id = fts.id
                 WHERE nodes_fts MATCH ?1
                 ORDER BY bm25(fts) LIMIT ?2",
            )
        };

        let mut stmt = self.conn().prepare(&sql)?;
        let nodes = if kind.is_some() {
            stmt.query_map(
                params![query, kind.unwrap().as_str(), limit],
                Self::row_to_node,
            )?
        } else {
            stmt.query_map(params![query, limit], Self::row_to_node)?
        }
        .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(nodes)
    }

    pub fn search_nodes_like(
        &self,
        query: &str,
        kind: Option<NodeKind>,
        limit: u32,
    ) -> Result<Vec<Node>> {
        let sql = if let Some(_k) = kind {
            "SELECT id, kind, name, qualified_name, file_path, language,
                    start_line, end_line, start_column, end_column,
                    docstring, signature, visibility,
                    is_exported, is_async, is_static, is_abstract,
                    decorators, type_parameters, updated_at
             FROM nodes
             WHERE name LIKE ?1 AND kind = ?2
             ORDER BY
               CASE
                 WHEN name = ?3 THEN 0
                 WHEN name LIKE ?1 THEN 1
                 ELSE 2
               END,
               length(name), file_path
             LIMIT ?4"
        } else {
            "SELECT id, kind, name, qualified_name, file_path, language,
                    start_line, end_line, start_column, end_column,
                    docstring, signature, visibility,
                    is_exported, is_async, is_static, is_abstract,
                    decorators, type_parameters, updated_at
             FROM nodes
             WHERE name LIKE ?1
             ORDER BY
               CASE
                 WHEN name = ?2 THEN 0
                 WHEN name LIKE ?1 THEN 1
                 ELSE 2
               END,
               length(name), file_path
             LIMIT ?3"
        };

        let pattern = wildcard_query_to_like(query);
        let rank_query = wildcard_query_rank_term(query);
        let mut stmt = self.conn().prepare(sql)?;
        let nodes = if kind.is_some() {
            stmt.query_map(
                params![pattern, kind.unwrap().as_str(), rank_query, limit],
                Self::row_to_node,
            )?
        } else {
            stmt.query_map(params![pattern, rank_query, limit], Self::row_to_node)?
        }
        .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(nodes)
    }

    pub fn get_node_by_name(&self, name: &str, file_path: &str) -> Result<Option<Node>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, kind, name, qualified_name, file_path, language,
                    start_line, end_line, start_column, end_column,
                    docstring, signature, visibility,
                    is_exported, is_async, is_static, is_abstract,
                    decorators, type_parameters, updated_at
             FROM nodes
             WHERE name = ?1 AND file_path = ?2
             LIMIT 1",
        )?;

        let node = stmt
            .query_row(params![name, file_path], Self::row_to_node)
            .optional()?;
        Ok(node)
    }

    pub fn get_node_by_name_any(&self, name: &str) -> Result<Option<Node>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, kind, name, qualified_name, file_path, language,
                    start_line, end_line, start_column, end_column,
                    docstring, signature, visibility,
                    is_exported, is_async, is_static, is_abstract,
                    decorators, type_parameters, updated_at
             FROM nodes
             WHERE name = ?1 OR qualified_name = ?1
             ORDER BY CASE WHEN name = ?1 THEN 0 ELSE 1 END, length(qualified_name)
             LIMIT 1",
        )?;

        let node = stmt
            .query_row(params![name], Self::row_to_node)
            .optional()?;
        Ok(node)
    }

    pub fn list_node_names(&self, query: &str, limit: u32) -> Result<Vec<String>> {
        let mut stmt = self
            .conn()
            .prepare("SELECT DISTINCT name FROM nodes WHERE name LIKE ?1 ORDER BY name LIMIT ?2")?;

        let pattern = format!("{}%", query);
        let names: Vec<String> = stmt
            .query_map(params![pattern, limit], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(names)
    }

    pub fn count_nodes(&self) -> Result<u64> {
        let count: u64 = self
            .conn()
            .query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0))?;
        Ok(count)
    }

    pub fn count_nodes_by_kind(&self, kind: NodeKind) -> Result<u64> {
        let count: u64 = self.conn().query_row(
            "SELECT COUNT(*) FROM nodes WHERE kind = ?1",
            [kind.as_str()],
            |r| r.get(0),
        )?;
        Ok(count)
    }

    // ==================== Edge operations ====================

    pub fn insert_edge(&self, edge: &Edge) -> Result<()> {
        self.conn().execute(
            "INSERT INTO edges (source, target, kind, metadata, line, col, provenance)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &edge.source,
                &edge.target,
                edge.kind.as_str(),
                edge.metadata
                    .as_ref()
                    .map(|v| serde_json::to_string(v).ok())
                    .flatten(),
                edge.line.map(|l| l as i64),
                edge.col.map(|c| c as i64),
                edge.provenance.as_deref(),
            ],
        )?;
        Ok(())
    }

    pub fn batch_insert_edges(&self, edges: &[Edge]) -> Result<()> {
        self.conn().execute_batch("BEGIN TRANSACTION")?;
        for edge in edges {
            self.insert_edge(edge)?;
        }
        self.conn().execute_batch("COMMIT")?;
        Ok(())
    }

    pub fn find_callers(&self, node_id: &str, limit: u32) -> Result<Vec<(Node, Edge)>> {
        let mut results = self.find_callers_by_id(node_id, limit)?;

        // Some edges are resolved to node IDs while others can still point at the
        // bare callee name. Return both forms instead of stopping at the first hit.
        if let Some(node) = self.get_node_by_id(node_id)? {
            results.extend(self.find_callers_by_name(&node.name, limit)?);
        }

        dedupe_edge_results(&mut results);
        results.truncate(limit as usize);
        Ok(results)
    }

    pub fn find_references_to(&self, node_id: &str, limit: u32) -> Result<Vec<(Node, Edge)>> {
        let mut results = self.find_references_to_target(node_id, limit)?;

        if let Some(node) = self.get_node_by_id(node_id)? {
            results.extend(self.find_references_to_target(&node.name, limit)?);
        }

        dedupe_edge_results(&mut results);
        results.truncate(limit as usize);
        Ok(results)
    }

    fn find_references_to_target(&self, target: &str, limit: u32) -> Result<Vec<(Node, Edge)>> {
        let stmt = self.conn().prepare(
            "SELECT n.id, n.kind, n.name, n.qualified_name, n.file_path, n.language,
                    n.start_line, n.end_line, n.start_column, n.end_column,
                    n.docstring, n.signature, n.visibility,
                    n.is_exported, n.is_async, n.is_static, n.is_abstract,
                    n.decorators, n.type_parameters, n.updated_at,
                    e.source, e.target, e.kind, e.metadata, e.line, e.col, e.provenance
             FROM edges e
             JOIN nodes n ON e.source = n.id
             WHERE e.target = ?1
               AND e.kind IN ('references', 'imports', 'instantiates')
             LIMIT ?2",
        )?;
        self.collect_edge_results(stmt, target, limit)
    }

    fn find_callers_by_id(&self, node_id: &str, limit: u32) -> Result<Vec<(Node, Edge)>> {
        let stmt = self.conn().prepare(
            "SELECT n.id, n.kind, n.name, n.qualified_name, n.file_path, n.language,
                    n.start_line, n.end_line, n.start_column, n.end_column,
                    n.docstring, n.signature, n.visibility,
                    n.is_exported, n.is_async, n.is_static, n.is_abstract,
                    n.decorators, n.type_parameters, n.updated_at,
                    e.source, e.target, e.kind, e.metadata, e.line, e.col, e.provenance
             FROM edges e
             JOIN nodes n ON e.source = n.id
             WHERE e.target = ?1 AND e.kind = 'calls'
             LIMIT ?2",
        )?;
        self.collect_edge_results(stmt, node_id, limit)
    }

    fn find_callers_by_name(&self, name: &str, limit: u32) -> Result<Vec<(Node, Edge)>> {
        let stmt = self.conn().prepare(
            "SELECT n.id, n.kind, n.name, n.qualified_name, n.file_path, n.language,
                    n.start_line, n.end_line, n.start_column, n.end_column,
                    n.docstring, n.signature, n.visibility,
                    n.is_exported, n.is_async, n.is_static, n.is_abstract,
                    n.decorators, n.type_parameters, n.updated_at,
                    e.source, e.target, e.kind, e.metadata, e.line, e.col, e.provenance
             FROM edges e
             JOIN nodes n ON e.source = n.id
             WHERE e.target = ?1 AND e.kind = 'calls'
             LIMIT ?2",
        )?;
        self.collect_edge_results(stmt, name, limit)
    }

    pub fn find_callees(&self, node_id: &str, limit: u32) -> Result<Vec<(Node, Edge)>> {
        let mut results = self.find_callees_by_id(node_id, limit)?;

        if let Some(node) = self.get_node_by_id(node_id)? {
            results.extend(self.find_callees_by_name(&node.name, limit)?);
        }

        dedupe_edge_results(&mut results);
        results.truncate(limit as usize);
        Ok(results)
    }

    fn find_callees_by_id(&self, node_id: &str, limit: u32) -> Result<Vec<(Node, Edge)>> {
        let stmt = self.conn().prepare(
            "SELECT n.id, n.kind, n.name, n.qualified_name, n.file_path, n.language,
                    n.start_line, n.end_line, n.start_column, n.end_column,
                    n.docstring, n.signature, n.visibility,
                    n.is_exported, n.is_async, n.is_static, n.is_abstract,
                    n.decorators, n.type_parameters, n.updated_at,
                    e.source, e.target, e.kind, e.metadata, e.line, e.col, e.provenance
             FROM edges e
             JOIN nodes n ON e.target = n.id OR e.target = n.name
             WHERE e.source = ?1 AND e.kind = 'calls'
             LIMIT ?2",
        )?;
        self.collect_edge_results(stmt, node_id, limit)
    }

    fn find_callees_by_name(&self, name: &str, limit: u32) -> Result<Vec<(Node, Edge)>> {
        let stmt = self.conn().prepare(
            "SELECT n.id, n.kind, n.name, n.qualified_name, n.file_path, n.language,
                    n.start_line, n.end_line, n.start_column, n.end_column,
                    n.docstring, n.signature, n.visibility,
                    n.is_exported, n.is_async, n.is_static, n.is_abstract,
                    n.decorators, n.type_parameters, n.updated_at,
                    e.source, e.target, e.kind, e.metadata, e.line, e.col, e.provenance
             FROM edges e
             JOIN nodes source_node ON e.source = source_node.id
             JOIN nodes n ON e.target = n.id OR e.target = n.name
             WHERE source_node.name = ?1 AND e.kind = 'calls'
             LIMIT ?2",
        )?;
        self.collect_edge_results(stmt, name, limit)
    }

    fn collect_edge_results<T: rusqlite::types::ToSql>(
        &self,
        mut stmt: rusqlite::Statement,
        param: T,
        limit: u32,
    ) -> Result<Vec<(Node, Edge)>> {
        let results: Vec<(Node, Edge)> = stmt
            .query_map(params![param, limit], |row| {
                let node = Self::row_to_node(row)?;
                let source: String = row.get(20)?;
                let target: String = row.get(21)?;
                let kind_str: String = row.get(22)?;
                let metadata: Option<String> = row.get(23)?;
                let line: Option<i64> = row.get(24)?;
                let col: Option<i64> = row.get(25)?;
                let provenance: Option<String> = row.get(26)?;

                let kind = EdgeKind::from_str(&kind_str).unwrap_or(EdgeKind::References);
                let edge = Edge {
                    source,
                    target,
                    kind,
                    metadata: metadata.and_then(|m| serde_json::from_str(&m).ok()),
                    line: line.map(|l| l as u32),
                    col: col.map(|c| c as u32),
                    provenance,
                };
                Ok((node, edge))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(results)
    }

    pub fn get_edges_for_node(&self, node_id: &str) -> Result<Vec<Edge>> {
        // Try by node ID first
        let edges = self.get_edges_for_id(node_id)?;
        if !edges.is_empty() {
            return Ok(edges);
        }
        // Fallback: search by name
        if let Ok(Some(node)) = self.get_node_by_id(node_id) {
            return self.get_edges_for_name(&node.name);
        }
        Ok(Vec::new())
    }

    fn get_edges_for_id(&self, node_id: &str) -> Result<Vec<Edge>> {
        let stmt = self.conn().prepare(
            "SELECT source, target, kind, metadata, line, col, provenance
             FROM edges WHERE source = ?1 OR target = ?1",
        )?;
        self.map_edges(stmt, node_id)
    }

    fn get_edges_for_name(&self, name: &str) -> Result<Vec<Edge>> {
        let stmt = self.conn().prepare(
            "SELECT source, target, kind, metadata, line, col, provenance
             FROM edges WHERE source = ?1 OR target = ?1",
        )?;
        self.map_edges(stmt, name)
    }

    fn map_edges<T: rusqlite::types::ToSql>(
        &self,
        mut stmt: rusqlite::Statement,
        param: T,
    ) -> Result<Vec<Edge>> {
        let edges: Vec<Edge> = stmt
            .query_map(params![param], |row| {
                let source: String = row.get(0)?;
                let target: String = row.get(1)?;
                let kind_str: String = row.get(2)?;
                let metadata: Option<String> = row.get(3)?;
                let line: Option<i64> = row.get(4)?;
                let col: Option<i64> = row.get(5)?;
                let provenance: Option<String> = row.get(6)?;

                let kind = EdgeKind::from_str(&kind_str).unwrap_or(EdgeKind::References);
                Ok(Edge {
                    source,
                    target,
                    kind,
                    metadata: metadata.and_then(|m| serde_json::from_str(&m).ok()),
                    line: line.map(|l| l as u32),
                    col: col.map(|c| c as u32),
                    provenance,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(edges)
    }

    pub fn get_edges_by_kind(&self, kind: EdgeKind) -> Result<Vec<Edge>> {
        let mut stmt = self.conn().prepare(
            "SELECT source, target, kind, metadata, line, col, provenance
             FROM edges WHERE kind = ?1",
        )?;

        let edges: Vec<Edge> = stmt
            .query_map(params![kind.as_str()], |row| {
                let source: String = row.get(0)?;
                let target: String = row.get(1)?;
                let kind_str: String = row.get(2)?;
                let metadata: Option<String> = row.get(3)?;
                let line: Option<i64> = row.get(4)?;
                let col: Option<i64> = row.get(5)?;
                let provenance: Option<String> = row.get(6)?;

                let kind = EdgeKind::from_str(&kind_str).unwrap_or(EdgeKind::References);
                Ok(Edge {
                    source,
                    target,
                    kind,
                    metadata: metadata.and_then(|m| serde_json::from_str(&m).ok()),
                    line: line.map(|l| l as u32),
                    col: col.map(|c| c as u32),
                    provenance,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(edges)
    }

    pub fn count_edges(&self) -> Result<u64> {
        let count: u64 = self
            .conn()
            .query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))?;
        Ok(count)
    }

    pub fn count_edges_by_kind(&self, kind: EdgeKind) -> Result<u64> {
        let count: u64 = self.conn().query_row(
            "SELECT COUNT(*) FROM edges WHERE kind = ?1",
            [kind.as_str()],
            |r| r.get(0),
        )?;
        Ok(count)
    }

    // ==================== File operations ====================

    pub fn insert_file(&self, file: &FileRecord) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO files (path, content_hash, language, size, modified_at, indexed_at, node_count, errors)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                &file.path,
                &file.content_hash,
                file.language.as_str(),
                file.size,
                file.modified_at,
                file.indexed_at,
                file.node_count,
                file.errors.as_ref().map(|v| serde_json::to_string(v).ok()).flatten(),
            ],
        )?;
        Ok(())
    }

    pub fn delete_file(&self, path: &str) -> Result<()> {
        self.delete_file_content_fts(path)?;
        self.conn()
            .execute("DELETE FROM files WHERE path = ?1", params![path])?;
        Ok(())
    }

    pub fn get_file(&self, path: &str) -> Result<Option<FileRecord>> {
        let mut stmt = self.conn().prepare(
            "SELECT path, content_hash, language, size, modified_at, indexed_at, node_count, errors
             FROM files WHERE path = ?1",
        )?;

        let file = stmt
            .query_row(params![path], Self::row_to_file)
            .optional()?;
        Ok(file)
    }

    pub fn list_files(&self, language: Option<Language>) -> Result<Vec<FileRecord>> {
        let sql = if let Some(_lang) = language {
            "SELECT path, content_hash, language, size, modified_at, indexed_at, node_count, errors
             FROM files WHERE language = ?1"
        } else {
            "SELECT path, content_hash, language, size, modified_at, indexed_at, node_count, errors
             FROM files"
        };

        let mut stmt = self.conn().prepare(sql)?;
        let files = if language.is_some() {
            stmt.query_map(params![language.unwrap().as_str()], Self::row_to_file)?
        } else {
            stmt.query_map([], Self::row_to_file)?
        }
        .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(files)
    }

    pub fn get_files_by_language(&self) -> Result<Vec<(String, u64)>> {
        let mut stmt = self
            .conn()
            .prepare("SELECT language, COUNT(*) FROM files GROUP BY language")?;

        let result: Vec<(String, u64)> = stmt
            .query_map([], |row| {
                let lang: String = row.get(0)?;
                let count: u64 = row.get(1)?;
                Ok((lang, count))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(result)
    }

    pub fn get_recent_files(&self, since: i64) -> Result<Vec<FileRecord>> {
        let mut stmt = self.conn().prepare(
            "SELECT path, content_hash, language, size, modified_at, indexed_at, node_count, errors
             FROM files WHERE modified_at > ?1",
        )?;

        let files: Vec<FileRecord> = stmt
            .query_map(params![since], Self::row_to_file)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(files)
    }

    pub fn count_files(&self) -> Result<u64> {
        let count: u64 = self
            .conn()
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
        Ok(count)
    }

    pub fn upsert_file_content_fts(&self, path: &str, content: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM files_fts WHERE path = ?1", params![path])?;
        self.conn().execute(
            "INSERT INTO files_fts(path, content) VALUES (?1, ?2)",
            params![path, content],
        )?;
        Ok(())
    }

    pub fn delete_file_content_fts(&self, path: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM files_fts WHERE path = ?1", params![path])?;
        Ok(())
    }

    pub fn search_file_contents(&self, query: &str, limit: u32) -> Result<Vec<(String, f64)>> {
        let sanitized = fts_query_terms(query);
        if sanitized.is_empty() {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn().prepare(
            "SELECT path, bm25(files_fts) AS rank
             FROM files_fts
             WHERE files_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![sanitized, limit], |row| {
            let path: String = row.get(0)?;
            let rank: f64 = row.get(1)?;
            // bm25 is lower-is-better, convert to positive score.
            Ok((path, -rank))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn get_all_node_names(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn().prepare("SELECT DISTINCT name FROM nodes")?;
        let names = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(names)
    }

    pub fn get_nodes_by_name_all(&self, name: &str, limit: u32) -> Result<Vec<Node>> {
        // This is called once per unresolved reference during post-index
        // resolution. Keep it strictly index-friendly: an earlier suffix LIKE
        // (`qualified_name LIKE %.name`) forced a full scan of large `nodes`
        // tables for every unresolved ref and made big C codebases appear to
        // hang after parsing. `name` already carries the simple symbol name we
        // need for call/reference resolution; exact qualified-name lookup is
        // retained for callers that pass a fully qualified symbol.
        let mut stmt = self.conn().prepare(
            "SELECT id, kind, name, qualified_name, file_path, language,
                    start_line, end_line, start_column, end_column,
                    docstring, signature, visibility,
                    is_exported, is_async, is_static, is_abstract,
                    decorators, type_parameters, updated_at
             FROM nodes
             WHERE name = ?1 OR qualified_name = ?1
             ORDER BY CASE WHEN name = ?1 THEN 0 ELSE 1 END,
                      length(qualified_name), file_path
             LIMIT ?2",
        )?;
        let nodes = stmt
            .query_map(params![name, limit], Self::row_to_node)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(nodes)
    }

    // ==================== Stats & Metadata ====================

    pub fn get_stats(&self) -> Result<crate::types::GraphStats> {
        let nodes_by_kind: serde_json::Value = {
            let mut stmt = self
                .conn()
                .prepare("SELECT kind, COUNT(*) FROM nodes GROUP BY kind")?;
            let rows: Vec<(String, u64)> = stmt
                .query_map([], |row| {
                    let k: String = row.get(0)?;
                    let c: u64 = row.get(1)?;
                    Ok((k, c))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let map: serde_json::Map<String, serde_json::Value> = rows
                .into_iter()
                .map(|(k, c)| (k, serde_json::Value::from(c)))
                .collect();
            serde_json::Value::Object(map)
        };

        let nodes_by_language: serde_json::Value = {
            let mut stmt = self
                .conn()
                .prepare("SELECT language, COUNT(*) FROM nodes GROUP BY language")?;
            let rows: Vec<(String, u64)> = stmt
                .query_map([], |row| {
                    let k: String = row.get(0)?;
                    let c: u64 = row.get(1)?;
                    Ok((k, c))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let map: serde_json::Map<String, serde_json::Value> = rows
                .into_iter()
                .map(|(k, c)| (k, serde_json::Value::from(c)))
                .collect();
            serde_json::Value::Object(map)
        };

        let edges_by_kind: serde_json::Value = {
            let mut stmt = self
                .conn()
                .prepare("SELECT kind, COUNT(*) FROM edges GROUP BY kind")?;
            let rows: Vec<(String, u64)> = stmt
                .query_map([], |row| {
                    let k: String = row.get(0)?;
                    let c: u64 = row.get(1)?;
                    Ok((k, c))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let map: serde_json::Map<String, serde_json::Value> = rows
                .into_iter()
                .map(|(k, c)| (k, serde_json::Value::from(c)))
                .collect();
            serde_json::Value::Object(map)
        };

        let total_files = self.count_files()?;
        let total_nodes = self.count_nodes()?;
        let total_edges = self.count_edges()?;

        Ok(crate::types::GraphStats {
            nodes_by_kind,
            nodes_by_language,
            edges_by_kind,
            total_files,
            total_nodes,
            total_edges,
        })
    }

    // ==================== Unresolved References ====================

    pub fn insert_unresolved_ref(&self, unresolved: &UnresolvedRef) -> Result<()> {
        self.conn().execute(
            "INSERT INTO unresolved_refs (from_node_id, reference_name, reference_kind, line, col, candidates, file_path, language)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                unresolved.from_node_id,
                unresolved.reference_name,
                unresolved.reference_kind,
                unresolved.line,
                unresolved.col,
                unresolved
                    .candidates
                    .as_ref()
                    .map(|c| serde_json::to_string(c).unwrap_or_default()),
                unresolved.file_path,
                unresolved.language,
            ],
        )?;
        Ok(())
    }

    pub fn delete_unresolved_refs_for_file(&self, file_path: &str) -> Result<()> {
        self.conn().execute(
            "DELETE FROM unresolved_refs WHERE file_path = ?1",
            params![file_path],
        )?;
        Ok(())
    }

    pub fn get_unresolved_refs(&self, limit: u32) -> Result<Vec<UnresolvedRef>> {
        let mut stmt = self.conn().prepare(
            "SELECT rowid, from_node_id, reference_name, reference_kind, line, col, candidates, file_path, language
             FROM unresolved_refs
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok(UnresolvedRef {
                id: Some(row.get(0)?),
                from_node_id: row.get(1)?,
                reference_name: row.get(2)?,
                reference_kind: row.get(3)?,
                line: row.get(4)?,
                col: row.get(5)?,
                candidates: {
                    let s: Option<String> = row.get(6)?;
                    s.and_then(|s| serde_json::from_str(&s).ok())
                },
                file_path: row.get(7)?,
                language: row.get(8)?,
            })
        })?;
        let mut refs = Vec::new();
        for row in rows {
            refs.push(row?);
        }
        Ok(refs)
    }

    pub fn get_unresolved_ref_groups(&self, limit: u32) -> Result<Vec<UnresolvedRefGroup>> {
        let mut stmt = self.conn().prepare(
            "SELECT reference_name, reference_kind, file_path, language, COUNT(*) AS c
             FROM unresolved_refs
             WHERE resolution_status IS NULL
             GROUP BY reference_name, reference_kind, file_path, language
             ORDER BY c DESC
             LIMIT ?1",
        )?;
        let groups = stmt
            .query_map(params![limit], |row| {
                Ok(UnresolvedRefGroup {
                    reference_name: row.get(0)?,
                    reference_kind: row.get(1)?,
                    file_path: row.get(2)?,
                    language: row.get(3)?,
                    count: row.get::<_, i64>(4)? as u64,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(groups)
    }

    pub fn get_unresolved_ref_groups_for_file(
        &self,
        file_path: &str,
        limit: u32,
    ) -> Result<Vec<UnresolvedRefGroup>> {
        let mut stmt = self.conn().prepare(
            "SELECT reference_name, reference_kind, file_path, language, COUNT(*) AS c
             FROM unresolved_refs
             WHERE file_path = ?1 AND resolution_status IS NULL
             GROUP BY reference_name, reference_kind, file_path, language
             ORDER BY c DESC
             LIMIT ?2",
        )?;
        let groups = stmt
            .query_map(params![file_path, limit], |row| {
                Ok(UnresolvedRefGroup {
                    reference_name: row.get(0)?,
                    reference_kind: row.get(1)?,
                    file_path: row.get(2)?,
                    language: row.get(3)?,
                    count: row.get::<_, i64>(4)? as u64,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(groups)
    }

    pub fn update_edges_for_unresolved_group(
        &self,
        group: &UnresolvedRefGroup,
        target_node_id: &str,
        provenance: &str,
        metadata: &serde_json::Value,
    ) -> Result<usize> {
        let metadata = serde_json::to_string(metadata).unwrap_or_default();
        let changed = self.conn().execute(
            "UPDATE edges
             SET target = ?1, provenance = ?2, metadata = ?3
             WHERE target = ?4
               AND kind = ?5
               AND source IN (SELECT id FROM nodes WHERE file_path = ?6)",
            params![
                target_node_id,
                provenance,
                metadata,
                group.reference_name,
                group.reference_kind,
                group.file_path,
            ],
        )?;
        Ok(changed)
    }

    pub fn delete_unresolved_ref_group(&self, group: &UnresolvedRefGroup) -> Result<usize> {
        let changed = self.conn().execute(
            "DELETE FROM unresolved_refs
             WHERE reference_name = ?1
               AND reference_kind = ?2
               AND file_path = ?3
               AND language = ?4",
            params![
                group.reference_name,
                group.reference_kind,
                group.file_path,
                group.language,
            ],
        )?;
        Ok(changed)
    }

    pub fn mark_unresolved_ref_group_attempted(
        &self,
        group: &UnresolvedRefGroup,
        reason: &str,
    ) -> Result<usize> {
        let changed = self.conn().execute(
            "UPDATE unresolved_refs
             SET resolution_status = 'unresolved',
                 resolution_reason = ?1,
                 resolution_attempted_at = CAST(strftime('%s', 'now') AS INTEGER)
             WHERE reference_name = ?2
               AND reference_kind = ?3
               AND file_path = ?4
               AND language = ?5
               AND resolution_status IS NULL",
            params![
                reason,
                group.reference_name,
                group.reference_kind,
                group.file_path,
                group.language,
            ],
        )?;
        Ok(changed)
    }

    pub fn get_unresolved_refs_for_file(
        &self,
        file_path: &str,
        limit: u32,
    ) -> Result<Vec<UnresolvedRef>> {
        let mut stmt = self.conn().prepare(
            "SELECT rowid, from_node_id, reference_name, reference_kind, line, col, candidates, file_path, language
             FROM unresolved_refs
             WHERE file_path = ?1
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![file_path, limit], |row| {
            Ok(UnresolvedRef {
                id: Some(row.get(0)?),
                from_node_id: row.get(1)?,
                reference_name: row.get(2)?,
                reference_kind: row.get(3)?,
                line: row.get(4)?,
                col: row.get(5)?,
                candidates: {
                    let s: Option<String> = row.get(6)?;
                    s.and_then(|s| serde_json::from_str(&s).ok())
                },
                file_path: row.get(7)?,
                language: row.get(8)?,
            })
        })?;
        let mut refs = Vec::new();
        for row in rows {
            refs.push(row?);
        }
        Ok(refs)
    }

    pub fn get_unresolved_refs_by_name(
        &self,
        reference_name: &str,
        limit: u32,
    ) -> Result<Vec<UnresolvedRef>> {
        let mut stmt = self.conn().prepare(
            "SELECT rowid, from_node_id, reference_name, reference_kind, line, col, candidates, file_path, language
             FROM unresolved_refs
             WHERE reference_name = ?1
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![reference_name, limit], |row| {
            Ok(UnresolvedRef {
                id: Some(row.get(0)?),
                from_node_id: row.get(1)?,
                reference_name: row.get(2)?,
                reference_kind: row.get(3)?,
                line: row.get(4)?,
                col: row.get(5)?,
                candidates: {
                    let s: Option<String> = row.get(6)?;
                    s.and_then(|s| serde_json::from_str(&s).ok())
                },
                file_path: row.get(7)?,
                language: row.get(8)?,
            })
        })?;
        let mut refs = Vec::new();
        for row in rows {
            refs.push(row?);
        }
        Ok(refs)
    }

    pub fn count_unresolved_refs(&self) -> Result<u64> {
        let count: u64 =
            self.conn()
                .query_row("SELECT COUNT(*) FROM unresolved_refs", [], |r| r.get(0))?;
        Ok(count)
    }

    pub fn count_pending_unresolved_refs(&self) -> Result<u64> {
        let count: u64 = self.conn().query_row(
            "SELECT COUNT(*) FROM unresolved_refs WHERE resolution_status IS NULL",
            [],
            |r| r.get(0),
        )?;
        Ok(count)
    }

    pub fn delete_unresolved_ref(&self, id: i64) -> Result<()> {
        self.conn()
            .execute("DELETE FROM unresolved_refs WHERE rowid = ?1", params![id])?;
        Ok(())
    }

    pub fn delete_file_nodes(&self, file_path: &str) -> Result<()> {
        // Edges can point at node IDs, but regex extraction currently stores
        // unresolved call targets as names. Delete edges whose source/target is
        // one of this file's node IDs before deleting nodes, otherwise re-index
        // and sync create duplicate outgoing edges.
        self.conn().execute(
            "DELETE FROM edges
             WHERE source IN (SELECT id FROM nodes WHERE file_path = ?1)
                OR target IN (SELECT id FROM nodes WHERE file_path = ?1)",
            params![file_path],
        )?;
        self.conn()
            .execute("DELETE FROM nodes WHERE file_path = ?1", params![file_path])?;
        self.delete_file_content_fts(file_path)?;
        Ok(())
    }

    pub fn get_file_node_count(&self, file_path: &str) -> Result<u32> {
        let count: u32 = self.conn().query_row(
            "SELECT COUNT(*) FROM nodes WHERE file_path = ?1",
            params![file_path],
            |r| r.get(0),
        )?;
        Ok(count)
    }

    pub fn set_project_metadata(&self, key: &str, value: &str) -> Result<()> {
        use std::time::{SystemTime, UNIX_EPOCH};
        let updated_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        self.conn().execute(
            "INSERT INTO project_metadata (key, value, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, updated_at],
        )?;
        Ok(())
    }

    pub fn get_project_metadata(&self, key: &str) -> Result<Option<String>> {
        self.conn()
            .query_row(
                "SELECT value FROM project_metadata WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn delete_project_metadata(&self, key: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM project_metadata WHERE key = ?1", params![key])?;
        Ok(())
    }

    pub fn clear_graph_for_reindex(&self) -> Result<()> {
        self.conn().execute_batch(
            "PRAGMA foreign_keys = OFF;
             DELETE FROM edges;
             DELETE FROM unresolved_refs;
             DELETE FROM nodes;
             DELETE FROM files;
             DELETE FROM files_fts;
             DELETE FROM nodes_fts;
             PRAGMA foreign_keys = ON;",
        )?;
        Ok(())
    }

    pub fn disable_node_fts_triggers(&self) -> Result<()> {
        self.conn().execute_batch(
            "DROP TRIGGER IF EXISTS nodes_ai;
             DROP TRIGGER IF EXISTS nodes_ad;
             DROP TRIGGER IF EXISTS nodes_au;",
        )?;
        Ok(())
    }

    pub fn recreate_node_fts_triggers(&self) -> Result<()> {
        self.conn().execute_batch(
            "CREATE TRIGGER IF NOT EXISTS nodes_ai AFTER INSERT ON nodes BEGIN
                 INSERT INTO nodes_fts(rowid, id, name, qualified_name, docstring, signature)
                 VALUES (NEW.rowid, NEW.id, NEW.name, NEW.qualified_name, NEW.docstring, NEW.signature);
             END;
             CREATE TRIGGER IF NOT EXISTS nodes_ad AFTER DELETE ON nodes BEGIN
                 INSERT INTO nodes_fts(nodes_fts, rowid, id, name, qualified_name, docstring, signature)
                 VALUES ('delete', OLD.rowid, OLD.id, OLD.name, OLD.qualified_name, OLD.docstring, OLD.signature);
             END;
             CREATE TRIGGER IF NOT EXISTS nodes_au AFTER UPDATE ON nodes BEGIN
                 INSERT INTO nodes_fts(nodes_fts, rowid, id, name, qualified_name, docstring, signature)
                 VALUES ('delete', OLD.rowid, OLD.id, OLD.name, OLD.qualified_name, OLD.docstring, OLD.signature);
                 INSERT INTO nodes_fts(rowid, id, name, qualified_name, docstring, signature)
                 VALUES (NEW.rowid, NEW.id, NEW.name, NEW.qualified_name, NEW.docstring, NEW.signature);
             END;",
        )?;
        Ok(())
    }

    pub fn rebuild_nodes_fts(&self) -> Result<()> {
        self.conn().execute_batch(
            "DELETE FROM nodes_fts;
             INSERT INTO nodes_fts(rowid, id, name, qualified_name, docstring, signature)
             SELECT rowid, id, name, qualified_name, docstring, signature FROM nodes;",
        )?;
        Ok(())
    }

    // ==================== Helpers ====================

    fn row_to_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<Node> {
        let kind_str: String = row.get(1)?;
        let language_str: String = row.get(5)?;
        Ok(Node {
            id: row.get(0)?,
            kind: NodeKind::from_str(&kind_str).unwrap_or(NodeKind::Variable),
            name: row.get(2)?,
            qualified_name: row.get(3)?,
            file_path: row.get(4)?,
            language: Language::from_str(&language_str).unwrap_or(Language::Python),
            start_line: row.get(6)?,
            end_line: row.get(7)?,
            start_column: row.get(8)?,
            end_column: row.get(9)?,
            docstring: row.get(10)?,
            signature: row.get(11)?,
            visibility: row.get(12)?,
            is_exported: {
                let v: i32 = row.get(13)?;
                v != 0
            },
            is_async: {
                let v: i32 = row.get(14)?;
                v != 0
            },
            is_static: {
                let v: i32 = row.get(15)?;
                v != 0
            },
            is_abstract: {
                let v: i32 = row.get(16)?;
                v != 0
            },
            decorators: {
                let s: Option<String> = row.get(17)?;
                s.and_then(|s| serde_json::from_str(&s).ok())
            },
            type_parameters: {
                let s: Option<String> = row.get(18)?;
                s.and_then(|s| serde_json::from_str(&s).ok())
            },
            updated_at: row.get(19)?,
        })
    }

    fn row_to_file(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileRecord> {
        let language_str: String = row.get(2)?;
        let errors_str: Option<String> = row.get(7)?;
        Ok(FileRecord {
            path: row.get(0)?,
            content_hash: row.get(1)?,
            language: Language::from_str(&language_str).unwrap_or(Language::Python),
            size: row.get(3)?,
            modified_at: row.get(4)?,
            indexed_at: row.get(5)?,
            node_count: row.get(6)?,
            errors: errors_str.and_then(|s| serde_json::from_str(&s).ok()),
        })
    }
}

use crate::types::Language;

fn wildcard_query_to_like(query: &str) -> String {
    let mut pattern = String::new();
    let mut saw_wildcard = false;
    for ch in query.chars() {
        match ch {
            '*' => {
                pattern.push('%');
                saw_wildcard = true;
            }
            '?' => {
                pattern.push('_');
                saw_wildcard = true;
            }
            '%' | '_' => {
                pattern.push(ch);
                saw_wildcard = true;
            }
            _ => pattern.push(ch),
        }
    }
    if !saw_wildcard {
        pattern.push('%');
    }
    pattern
}

fn wildcard_query_rank_term(query: &str) -> String {
    query.trim_end_matches(['*', '?', '%', '_']).to_string()
}

fn fts_query_terms(query: &str) -> String {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .map(str::trim)
        .filter(|t| t.len() >= 2)
        .map(|t| t.replace('"', ""))
        .filter(|t| !t.is_empty())
        .collect();
    terms.join(" OR ")
}

fn dedupe_edge_results(results: &mut Vec<(Node, Edge)>) {
    let mut seen = HashSet::new();
    results.retain(|(node, edge)| {
        seen.insert((
            node.id.clone(),
            edge.source.clone(),
            edge.target.clone(),
            edge.kind.as_str(),
            edge.line,
            edge.col,
        ))
    });
    // Collapse duplicate rows that originate from the same unresolved edge
    // matching multiple nodes with the same name (e.g. a header macro defined
    // twice). Key by (call-site, callee-name) so distinct call sites remain
    // separate while multiple definitions of the same name collapse to one.
    let mut seen_site = HashSet::new();
    results.retain(|(node, edge)| {
        seen_site.insert((
            edge.source.clone(),
            edge.line,
            edge.col,
            node.name.clone(),
            edge.kind.as_str(),
        ))
    });
}
