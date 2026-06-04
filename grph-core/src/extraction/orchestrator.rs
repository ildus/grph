use crate::db::Database;
use crate::errors::{GrphError, Result};
use crate::extraction::grammars::{detect_language, detect_language_with_content};
use crate::extraction::languages::extract_for_language;
use crate::extraction::tree_sitter::ExtractionResult;
use crate::resolution::builtins::is_builtin;
use crate::types::{
    Edge, EdgeKind, FileRecord, IndexProgress, IndexResult, Language, Node, NodeKind, SyncResult,
    UnresolvedRef,
};
use ignore::WalkBuilder;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_FILE_SIZE: u64 = 20_000_000; // 20MB
const INDEX_WORKER_STACK_SIZE: usize = 16 * 1024 * 1024;
const STORE_BATCH_SIZE: usize = 256;

fn should_skip_path(relative_path: &str) -> bool {
    relative_path.split('/').any(|part| {
        matches!(
            part,
            ".git" | ".grph" | "target" | "node_modules" | "dist" | "build" | ".cache"
        )
    })
}

pub struct ExtractionOrchestrator {
    pub db: Database,
    project_root: PathBuf,
}

impl ExtractionOrchestrator {
    pub fn new(db: Database, project_root: PathBuf) -> Result<Self> {
        Ok(Self { db, project_root })
    }

    /// Walk file tree, respect .gitignore, detect language by extension
    pub fn scan_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();

        let walker = WalkBuilder::new(&self.project_root)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .hidden(false) // we want .gitignore files to be read for their rules
            .standard_filters(true) // respects .gitignore, .ignore, node_modules etc
            // Build output directories are high-noise for code intelligence and
            // can appear in existing indexes if a project was indexed before a
            // .gitignore existed (e.g. Cargo target/debug/build/*/flag_check.c).
            .add_custom_ignore_filename(".grphignore")
            .build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Skip directories
            if !entry.file_type().map_or(false, |ft| ft.is_file()) {
                continue;
            }

            let path = entry.into_path();

            // Skip files > 1MB
            if let Ok(metadata) = fs::metadata(&path) {
                if metadata.len() > MAX_FILE_SIZE {
                    continue;
                }
            }

            let rel_path = path
                .strip_prefix(&self.project_root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if should_skip_path(&rel_path) {
                continue;
            }

            // Only index files with supported languages
            if detect_language(&path).is_some() {
                files.push(path);
            }
        }

        Ok(files)
    }

    /// Full index: scan → parse → store → resolve
    pub fn index_all(&mut self, progress: impl Fn(IndexProgress)) -> Result<IndexResult> {
        self.index_all_with_jobs(default_index_jobs(), progress)
    }

    /// Full index with configurable parallel parsing workers.
    pub fn index_all_with_jobs(
        &mut self,
        jobs: usize,
        progress: impl Fn(IndexProgress),
    ) -> Result<IndexResult> {
        self.index_all_with_jobs_and_force(jobs, false, progress)
    }

    pub fn index_all_with_jobs_and_force(
        &mut self,
        jobs: usize,
        force: bool,
        progress: impl Fn(IndexProgress),
    ) -> Result<IndexResult> {
        let scanned_files = self.scan_files()?;
        if force {
            self.db.clear_graph_for_reindex()?;
        } else {
            self.prune_unscanned_files(&scanned_files)?;
        }
        let files = self.changed_index_files(&scanned_files, force)?;
        let total = files.len() as u64;

        if files.is_empty() {
            progress(IndexProgress {
                current: 0,
                total,
                phase: "complete".to_string(),
                current_file: None,
                parsed: 0,
                stored: 0,
            });
            return Ok(IndexResult {
                files_indexed: 0,
                nodes_created: 0,
                edges_created: 0,
            });
        }

        let defer_node_fts = force || files.len() >= 128;
        if defer_node_fts {
            self.db.disable_node_fts_triggers()?;
        }

        let index_result = self.index_changed_files(files, jobs, total, progress);
        if defer_node_fts {
            if index_result.is_ok() {
                self.db.rebuild_nodes_fts()?;
            }
            self.db.recreate_node_fts_triggers()?;
        }
        index_result
    }

    fn index_changed_files(
        &mut self,
        files: Vec<PathBuf>,
        jobs: usize,
        total: u64,
        progress: impl Fn(IndexProgress),
    ) -> Result<IndexResult> {
        let mut total_nodes = 0u64;
        let mut total_edges = 0u64;
        let mut files_indexed = 0u64;

        let jobs = jobs.max(1).min(files.len().max(1));
        if jobs == 1 || files.len() <= 1 {
            for (i, file_path) in files.iter().enumerate() {
                let relative_path = relative_path(&self.project_root, file_path);

                progress(IndexProgress {
                    current: i as u64 + 1,
                    total,
                    phase: "parsing".to_string(),
                    current_file: Some(relative_path.clone()),
                    parsed: i as u64,
                    stored: files_indexed,
                });

                let language = detect_language(file_path).ok_or_else(|| {
                    GrphError::Extraction(format!("Unsupported language for {:?}", file_path))
                })?;

                match self.parse_and_store(file_path, &relative_path, language) {
                    Ok((nodes, edges)) => {
                        total_nodes += nodes as u64;
                        total_edges += edges as u64;
                        files_indexed += 1;
                    }
                    Err(e) => {
                        eprintln!("WARN: Failed to parse {}: {}", file_path.display(), e);
                    }
                }
            }

            progress(IndexProgress {
                current: total,
                total,
                phase: "complete".to_string(),
                current_file: None,
                parsed: total,
                stored: files_indexed,
            });

            return Ok(IndexResult {
                files_indexed,
                nodes_created: total_nodes,
                edges_created: total_edges,
            });
        }

        let (job_tx, job_rx) = mpsc::channel::<IndexJob>();
        let (result_tx, result_rx) = mpsc::channel::<ParsedFileResult>();
        let job_rx = Arc::new(Mutex::new(job_rx));

        for worker_id in 0..jobs {
            let job_rx = Arc::clone(&job_rx);
            let result_tx = result_tx.clone();
            std::thread::Builder::new()
                .name(format!("grph-index-{worker_id}"))
                .stack_size(index_worker_stack_size())
                .spawn(move || loop {
                    let job = match job_rx.lock().expect("job receiver poisoned").recv() {
                        Ok(job) => job,
                        Err(_) => break,
                    };
                    let result = parse_index_job(job);
                    if result_tx.send(result).is_err() {
                        break;
                    }
                })
                .map_err(|e| GrphError::Extraction(format!("Failed to start index worker: {e}")))?;
        }
        drop(result_tx);

        for file_path in &files {
            let relative_path = relative_path(&self.project_root, file_path);
            detect_language(file_path).ok_or_else(|| {
                GrphError::Extraction(format!("Unsupported language for {:?}", file_path))
            })?;
            job_tx
                .send(IndexJob {
                    file_path: file_path.clone(),
                    relative_path,
                })
                .map_err(|e| GrphError::Extraction(format!("Index worker failed: {e}")))?;
        }
        drop(job_tx);

        let mut store_batch = Vec::with_capacity(STORE_BATCH_SIZE);
        let mut parsed_count = 0u64;
        let mut stored_count = 0u64;
        for i in 0..files.len() {
            match result_rx.recv() {
                Ok(Ok(Some(parsed))) => {
                    parsed_count = i as u64 + 1;
                    progress(IndexProgress {
                        current: parsed_count,
                        total,
                        phase: "parsed".to_string(),
                        current_file: Some(parsed.relative_path.clone()),
                        parsed: parsed_count,
                        stored: stored_count,
                    });
                    store_batch.push(parsed);
                    if store_batch.len() >= STORE_BATCH_SIZE {
                        let batch = std::mem::take(&mut store_batch);
                        match self.store_extractions_batch_with_progress(batch, |path| {
                            stored_count += 1;
                            progress(IndexProgress {
                                current: stored_count,
                                total,
                                phase: "storing".to_string(),
                                current_file: Some(path.to_string()),
                                parsed: parsed_count,
                                stored: stored_count,
                            });
                        }) {
                            Ok((files, nodes, edges)) => {
                                files_indexed += files as u64;
                                total_nodes += nodes as u64;
                                total_edges += edges as u64;
                            }
                            Err(e) => eprintln!("WARN: Failed to store parsed batch: {}", e),
                        }
                    }
                }
                Ok(Ok(None)) => {
                    parsed_count = i as u64 + 1;
                    progress(IndexProgress {
                        current: parsed_count,
                        total,
                        phase: "skipping".to_string(),
                        current_file: None,
                        parsed: parsed_count,
                        stored: stored_count,
                    });
                }
                Ok(Err((path, e))) => {
                    parsed_count = i as u64 + 1;
                    progress(IndexProgress {
                        current: parsed_count,
                        total,
                        phase: "parsing".to_string(),
                        current_file: Some(path.clone()),
                        parsed: parsed_count,
                        stored: stored_count,
                    });
                    eprintln!("WARN: Failed to parse {}: {}", path, e);
                }
                Err(e) => return Err(GrphError::Extraction(format!("Index worker failed: {e}"))),
            }
        }

        if !store_batch.is_empty() {
            match self.store_extractions_batch_with_progress(store_batch, |path| {
                stored_count += 1;
                progress(IndexProgress {
                    current: stored_count,
                    total,
                    phase: "storing".to_string(),
                    current_file: Some(path.to_string()),
                    parsed: parsed_count,
                    stored: stored_count,
                });
            }) {
                Ok((files, nodes, edges)) => {
                    files_indexed += files as u64;
                    total_nodes += nodes as u64;
                    total_edges += edges as u64;
                }
                Err(e) => eprintln!("WARN: Failed to store final parsed batch: {}", e),
            }
        }

        progress(IndexProgress {
            current: total,
            total,
            phase: "complete".to_string(),
            current_file: None,
            parsed: parsed_count,
            stored: stored_count,
        });

        Ok(IndexResult {
            files_indexed,
            nodes_created: total_nodes,
            edges_created: total_edges,
        })
    }

    fn changed_index_files(&self, files: &[PathBuf], force: bool) -> Result<Vec<PathBuf>> {
        if force {
            return Ok(files.to_vec());
        }
        let mut out = Vec::new();
        for file_path in files {
            let rel_path = relative_path(&self.project_root, file_path);
            let info = match source_file_info(file_path) {
                Ok(info) => info,
                Err(e) => {
                    eprintln!("WARN: Failed to stat {}: {}", file_path.display(), e);
                    continue;
                }
            };
            if let Some(existing) = self.db.get_file(&rel_path)? {
                if existing.size == info.size && existing.modified_at >= info.modified_at {
                    continue;
                }
            }
            out.push(file_path.clone());
        }
        Ok(out)
    }

    fn prune_unscanned_files(&self, files: &[PathBuf]) -> Result<()> {
        let current_files: std::collections::HashSet<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(&self.project_root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();

        for file in self.db.list_files(None)? {
            if !current_files.contains(&file.path) || should_skip_path(&file.path) {
                self.db.delete_file_nodes(&file.path)?;
                self.db.delete_file(&file.path)?;
            }
        }
        Ok(())
    }

    /// Incremental sync: detect changed files, re-index only those
    pub fn sync(&mut self) -> Result<SyncResult> {
        let files = self.scan_files()?;
        let mut files_changed = 0u64;
        let mut files_added = 0u64;
        let mut files_deleted = 0u64;
        let mut nodes_created = 0u64;
        let mut edges_created = 0u64;

        let existing_files: std::collections::HashSet<String> = self
            .db
            .list_files(None)?
            .into_iter()
            .map(|f| f.path)
            .collect();

        let current_files: std::collections::HashSet<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(&self.project_root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();

        // Detect deleted files
        for path in existing_files.iter() {
            if !current_files.contains(path) {
                self.db.delete_file_nodes(path)?;
                self.db.delete_file(path)?;
                files_deleted += 1;
            }
        }

        // Process new and changed files only.
        for file_path in &files {
            let rel_path = file_path
                .strip_prefix(&self.project_root)
                .unwrap_or(file_path)
                .to_string_lossy()
                .replace('\\', "/");

            let file_info = match source_file_info(file_path) {
                Ok(info) => info,
                Err(e) => {
                    eprintln!("WARN: Failed to stat {}: {}", file_path.display(), e);
                    continue;
                }
            };

            let existing_file = self.db.get_file(&rel_path)?;
            let is_new = existing_file.is_none();
            if existing_file
                .as_ref()
                .map(|file| {
                    file.size == file_info.size && file.modified_at >= file_info.modified_at
                })
                .unwrap_or(false)
            {
                continue;
            }

            let content = match fs::read(file_path) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(e) => {
                    eprintln!("WARN: Failed to read {}: {}", file_path.display(), e);
                    continue;
                }
            };
            let content_hash = Self::hash_content(&content);
            if existing_file
                .as_ref()
                .map(|file| {
                    file.content_hash == content_hash
                        && file.size == file_info.size
                        && file.modified_at >= file_info.modified_at
                })
                .unwrap_or(false)
            {
                continue;
            }

            let Some(language) = detect_language_for_content(file_path, &content) else {
                continue;
            };
            match self.parse_and_store_content(file_path, &rel_path, language, content) {
                Ok((nodes, edges)) => {
                    nodes_created += nodes as u64;
                    edges_created += edges as u64;
                    if is_new {
                        files_added += 1;
                    } else {
                        files_changed += 1;
                    }
                }
                Err(e) => {
                    eprintln!("WARN: Failed to sync {}: {}", file_path.display(), e);
                }
            }
        }

        Ok(SyncResult {
            files_changed,
            files_added,
            files_deleted,
            nodes_created,
            edges_created,
        })
    }

    /// Incrementally sync one file without scanning the whole project.
    pub fn sync_file(&mut self, file_path: &Path) -> Result<SyncResult> {
        let absolute_path = if file_path.is_absolute() {
            file_path.to_path_buf()
        } else {
            self.project_root.join(file_path)
        };
        let relative_path = absolute_path
            .strip_prefix(&self.project_root)
            .map_err(|_| {
                GrphError::InvalidInput(format!(
                    "file is outside project root: {}",
                    absolute_path.display()
                ))
            })?
            .to_string_lossy()
            .replace('\\', "/");

        if should_skip_path(&relative_path) || !absolute_path.exists() {
            let existed = self.db.get_file(&relative_path)?.is_some();
            self.db.delete_file_nodes(&relative_path)?;
            self.db.delete_unresolved_refs_for_file(&relative_path)?;
            self.db.delete_file(&relative_path)?;
            return Ok(SyncResult {
                files_changed: 0,
                files_added: 0,
                files_deleted: u64::from(existed),
                nodes_created: 0,
                edges_created: 0,
            });
        }

        let file_info = source_file_info(&absolute_path)?;
        let existing_file = self.db.get_file(&relative_path)?;
        let is_new = existing_file.is_none();
        if existing_file
            .as_ref()
            .map(|file| file.size == file_info.size && file.modified_at >= file_info.modified_at)
            .unwrap_or(false)
        {
            return Ok(SyncResult {
                files_changed: 0,
                files_added: 0,
                files_deleted: 0,
                nodes_created: 0,
                edges_created: 0,
            });
        }

        let content = {
            let bytes = fs::read(&absolute_path)?;
            String::from_utf8_lossy(&bytes).into_owned()
        };
        let content_hash = Self::hash_content(&content);
        if existing_file
            .as_ref()
            .map(|file| {
                file.content_hash == content_hash
                    && file.size == file_info.size
                    && file.modified_at >= file_info.modified_at
            })
            .unwrap_or(false)
        {
            return Ok(SyncResult {
                files_changed: 0,
                files_added: 0,
                files_deleted: 0,
                nodes_created: 0,
                edges_created: 0,
            });
        }

        let Some(language) = detect_language_for_content(&absolute_path, &content) else {
            let existed = existing_file.is_some();
            self.db.delete_file_nodes(&relative_path)?;
            self.db.delete_unresolved_refs_for_file(&relative_path)?;
            self.db.delete_file(&relative_path)?;
            return Ok(SyncResult {
                files_changed: 0,
                files_added: 0,
                files_deleted: u64::from(existed),
                nodes_created: 0,
                edges_created: 0,
            });
        };

        let (nodes_created, edges_created) =
            self.parse_and_store_content(&absolute_path, &relative_path, language, content)?;

        Ok(SyncResult {
            files_changed: u64::from(!is_new),
            files_added: u64::from(is_new),
            files_deleted: 0,
            nodes_created: nodes_created as u64,
            edges_created: edges_created as u64,
        })
    }

    fn parse_and_store(
        &mut self,
        file_path: &Path,
        relative_path: &str,
        language: Language,
    ) -> Result<(usize, usize)> {
        let content = {
            let bytes = fs::read(file_path)?;
            String::from_utf8_lossy(&bytes).into_owned()
        };
        // Content-based override for ambiguous .sc extension (could be embedded SQL/C,
        // Scala, SuperCollider — only parse as Esqlc if content has exec sql).
        let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let language = if ext == "sc" {
            match detect_language_with_content(file_path, &content) {
                Some(lang) => lang,
                None => return Ok((0, 0)),
            }
        } else {
            language
        };
        self.parse_and_store_content(file_path, relative_path, language, content)
    }

    fn parse_and_store_content(
        &mut self,
        _file_path: &Path,
        relative_path: &str,
        language: Language,
        content: String,
    ) -> Result<(usize, usize)> {
        let extraction = Self::extract_content(relative_path, language, &content)?;

        self.store_extraction(ParsedFile {
            relative_path: relative_path.to_string(),
            language,
            content,
            extraction,
            info: source_file_info(_file_path)?,
        })
    }

    fn extract_content(
        relative_path: &str,
        language: Language,
        content: &str,
    ) -> Result<ExtractionResult> {
        let mut extraction = match extract_for_language(language, content, relative_path) {
            Ok(result) if !result.nodes.is_empty() => result,
            _ => {
                crate::extraction::languages::extract_with_regex(content, relative_path, language)?
            }
        };
        Self::resolve_local_edge_targets(&mut extraction.edges, &extraction.nodes);
        Ok(extraction)
    }

    fn store_extraction(&mut self, parsed: ParsedFile) -> Result<(usize, usize)> {
        let (_, nodes, edges) = self.store_extractions_batch(vec![parsed])?;
        Ok((nodes, edges))
    }

    fn store_extractions_batch(&mut self, batch: Vec<ParsedFile>) -> Result<(usize, usize, usize)> {
        self.store_extractions_batch_with_progress(batch, |_| {})
    }

    fn store_extractions_batch_with_progress(
        &mut self,
        batch: Vec<ParsedFile>,
        mut on_file_stored: impl FnMut(&str),
    ) -> Result<(usize, usize, usize)> {
        if batch.is_empty() {
            return Ok((0, 0, 0));
        }

        self.db
            .conn()
            .execute_batch("PRAGMA foreign_keys = OFF; BEGIN TRANSACTION")?;

        let mut files = 0usize;
        let mut nodes = 0usize;
        let mut edges = 0usize;
        for parsed in batch {
            let path = parsed.relative_path.clone();
            match self.store_extraction_in_transaction(parsed) {
                Ok((n, e)) => {
                    files += 1;
                    nodes += n;
                    edges += e;
                    on_file_stored(&path);
                }
                Err(err) => {
                    let _ = self
                        .db
                        .conn()
                        .execute_batch("ROLLBACK; PRAGMA foreign_keys = ON");
                    return Err(err);
                }
            }
        }

        self.db
            .conn()
            .execute_batch("COMMIT; PRAGMA foreign_keys = ON")?;
        Ok((files, nodes, edges))
    }

    fn store_extraction_in_transaction(&mut self, parsed: ParsedFile) -> Result<(usize, usize)> {
        let relative_path = parsed.relative_path;
        let language = parsed.language;
        let content = parsed.content;
        let mut extraction = parsed.extraction;
        let info = parsed.info;
        Self::synthesize_generic_callback_edges(
            &mut extraction.edges,
            &extraction.nodes,
            &content,
            language,
        );
        let node_count = extraction.nodes.len();
        let edge_count = extraction.edges.len();

        self.db.delete_file_nodes(&relative_path)?;
        self.db.delete_unresolved_refs_for_file(&relative_path)?;

        for node in &extraction.nodes {
            self.db.insert_node(node)?;
        }

        for edge in &extraction.edges {
            self.db.insert_edge(edge)?;
        }

        let node_ids: HashSet<&str> = extraction.nodes.iter().map(|n| n.id.as_str()).collect();
        for edge in &extraction.edges {
            if !node_ids.contains(edge.target.as_str())
                && should_track_unresolved_reference(&edge.target, edge.kind, language)
            {
                let unresolved = UnresolvedRef {
                    id: None,
                    from_node_id: edge.source.clone(),
                    reference_name: edge.target.clone(),
                    reference_kind: edge.kind.as_str().to_string(),
                    line: edge.line.unwrap_or(0),
                    col: edge.col.unwrap_or(0),
                    candidates: None,
                    file_path: relative_path.clone(),
                    language: language.as_str().to_string(),
                };
                if let Err(e) = self.db.insert_unresolved_ref(&unresolved) {
                    eprintln!("WARN: Failed to insert unresolved ref: {}", e);
                }
            }
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        let file_record = FileRecord {
            path: relative_path,
            content_hash: Self::hash_content(&content),
            language,
            size: info.size,
            modified_at: info.modified_at,
            indexed_at: now,
            node_count: extraction.nodes.len() as u32,
            errors: if extraction.errors.is_empty() {
                None
            } else {
                Some(extraction.errors)
            },
        };

        self.db.insert_file(&file_record)?;
        self.db
            .upsert_file_content_fts(&file_record.path, &content)?;

        Ok((node_count, edge_count))
    }

    /// Resolve edge targets that the regex extractor left as bare names.
    ///
    /// The schema expects edge targets to be node IDs, but regex call detection
    /// naturally sees only the textual callee name. Resolve unambiguous targets
    /// inside the current file before insertion. Ambiguous or external calls are
    /// intentionally left as names for the query fallback/resolution layer.
    fn resolve_local_edge_targets(edges: &mut [Edge], nodes: &[Node]) {
        let node_ids: HashSet<&str> = nodes.iter().map(|node| node.id.as_str()).collect();
        let mut by_name: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut by_qualified_name: HashMap<&str, &str> = HashMap::new();

        for node in nodes {
            by_name
                .entry(node.name.as_str())
                .or_default()
                .push(node.id.as_str());
            by_qualified_name.insert(node.qualified_name.as_str(), node.id.as_str());
        }

        for edge in edges {
            if node_ids.contains(edge.target.as_str()) {
                continue;
            }

            let resolved = by_qualified_name
                .get(edge.target.as_str())
                .copied()
                .or_else(|| match by_name.get(edge.target.as_str()) {
                    Some(ids) if ids.len() == 1 => Some(ids[0]),
                    _ => None,
                });

            if let Some(target_id) = resolved {
                edge.target = target_id.to_string();
                if edge.kind == EdgeKind::Calls || edge.kind == EdgeKind::References {
                    edge.provenance = Some(match edge.provenance.as_deref() {
                        Some(provenance) if provenance.contains("resolved-local") => {
                            provenance.to_string()
                        }
                        Some(provenance) => format!("{}+resolved-local", provenance),
                        None => "resolved-local".to_string(),
                    });
                }
            }
        }
    }

    /// Synthesize non-framework dynamic dispatch edges for common language/runtime
    /// callback patterns. These are deliberately generic (setTimeout, Promise.then,
    /// Array.map/filter, EventEmitter-style .on, Python Thread(target=...), Go
    /// HandleFunc/go/defer, C signal) and marked heuristic so callers can explain
    /// that the edge is inferred rather than a direct lexical call.
    fn synthesize_generic_callback_edges(
        edges: &mut Vec<Edge>,
        nodes: &[Node],
        content: &str,
        language: Language,
    ) {
        let callable_by_name: HashMap<&str, &Node> = nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Function | NodeKind::Method))
            .map(|n| (n.name.as_str(), n))
            .collect();
        if callable_by_name.is_empty() {
            return;
        }
        let file_node = nodes.iter().find(|n| n.kind == NodeKind::File);

        let patterns: &[&str] = match language {
            Language::JavaScript | Language::TypeScript | Language::Jsx | Language::Tsx => &[
                r"\b(?:setTimeout|setInterval|queueMicrotask)\s*\(\s*([A-Za-z_][A-Za-z0-9_]*)",
                r"\.(?:then|catch|finally|map|filter|forEach|some|every|find)\s*\(\s*([A-Za-z_][A-Za-z0-9_]*)",
                r#"\.(?:on|once|addEventListener)\s*\(\s*['"][^'"]+['"]\s*,\s*([A-Za-z_][A-Za-z0-9_]*)"#,
            ],
            Language::Python => &[
                r"\bthreading\.Thread\s*\([^\n)]*target\s*=\s*([A-Za-z_][A-Za-z0-9_]*)",
                r"\batexit\.register\s*\(\s*([A-Za-z_][A-Za-z0-9_]*)",
                r"\b(?:map|filter)\s*\(\s*([A-Za-z_][A-Za-z0-9_]*)",
                r"\bsorted\s*\([^\n)]*key\s*=\s*([A-Za-z_][A-Za-z0-9_]*)",
            ],
            Language::Go => &[
                r"\bhttp\.HandleFunc\s*\(\s*[^,]+,\s*([A-Za-z_][A-Za-z0-9_]*)",
                r"\bgo\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(",
                r"\bdefer\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(",
            ],
            Language::Rust => &[
                r"\.(?:map|filter|for_each|find|any|all)\s*\(\s*([A-Za-z_][A-Za-z0-9_]*)",
                r"\bthread::spawn\s*\(\s*([A-Za-z_][A-Za-z0-9_]*)",
            ],
            Language::C | Language::Cpp => &[
                r"\bsignal\s*\(\s*[^,]+,\s*([A-Za-z_][A-Za-z0-9_]*)",
                r"\bpthread_create\s*\([^\n)]*,\s*[^,]+,\s*([A-Za-z_][A-Za-z0-9_]*)",
            ],
            _ => &[],
        };

        let mut existing = HashSet::new();
        for edge in edges.iter() {
            existing.insert((edge.source.clone(), edge.target.clone(), edge.kind.as_str()));
        }

        for pattern in patterns {
            let Ok(re) = regex::Regex::new(pattern) else {
                continue;
            };
            for caps in re.captures_iter(content) {
                let Some(m) = caps.get(1) else { continue };
                let callback_name = m.as_str();
                let Some(target) = callable_by_name.get(callback_name) else {
                    continue;
                };
                let line = byte_to_line(content, m.start());
                let Some(source) = innermost_node_at_line(nodes, line).or(file_node) else {
                    continue;
                };
                if source.id == target.id {
                    continue;
                }
                if !existing.insert((
                    source.id.clone(),
                    target.id.clone(),
                    EdgeKind::Calls.as_str(),
                )) {
                    continue;
                }
                edges.push(Edge {
                    source: source.id.clone(),
                    target: target.id.clone(),
                    kind: EdgeKind::Calls,
                    metadata: Some(serde_json::json!({
                        "synthesizedBy": "generic-callback",
                        "via": callback_name,
                        "registeredAt": format!("{}:{}", source.file_path, line),
                    })),
                    line: Some(line),
                    col: Some(0),
                    provenance: Some("heuristic".to_string()),
                });
            }
        }
    }

    pub fn hash_content(content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        hex::encode(hasher.finalize())
    }
}

fn should_track_unresolved_reference(target: &str, kind: EdgeKind, language: Language) -> bool {
    if target.is_empty() || is_builtin(target, language) {
        return false;
    }

    // Shell extraction can see variable expansions as command-like references.
    // They are runtime data, not navigable symbols.
    if language == Language::Shell && is_shell_variable_expansion(target) {
        return false;
    }

    // C-family projects often have hundreds of thousands of macro/type-looking
    // tokens captured as call/reference edges: ERx(...), _T(...), ASSERT(...),
    // DB_FAILURE_MACRO, DWORD, etc. They are usually preprocessor constants or
    // external SDK macros, not project symbols an agent can navigate to. Keeping
    // every occurrence in unresolved_refs makes post-index resolution O(noise).
    if matches!(language, Language::C | Language::Cpp | Language::Esqlc)
        && matches!(
            kind,
            EdgeKind::Calls | EdgeKind::References | EdgeKind::Instantiates
        )
        && is_c_macro_like_external(target)
    {
        return false;
    }

    true
}

fn is_shell_variable_expansion(name: &str) -> bool {
    let trimmed = name.trim();
    trimmed.starts_with('$')
        || (trimmed.starts_with("${") && trimmed.ends_with('}'))
        || trimmed.starts_with("$(")
}

fn is_c_macro_like_external(name: &str) -> bool {
    let trimmed = name.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
    if trimmed.len() <= 1 {
        return true;
    }
    if trimmed.ends_with("_MACRO") {
        return true;
    }
    if trimmed.starts_with('_')
        && trimmed
            .chars()
            .skip(1)
            .all(|c| c == '_' || c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return true;
    }
    let has_alpha = trimmed.chars().any(|c| c.is_ascii_alphabetic());
    let has_lower = trimmed.chars().any(|c| c.is_ascii_lowercase());
    let has_upper = trimmed.chars().any(|c| c.is_ascii_uppercase());
    has_alpha && has_upper && !has_lower
}

fn byte_to_line(content: &str, byte_offset: usize) -> u32 {
    content[..byte_offset.min(content.len())]
        .bytes()
        .filter(|b| *b == b'\n')
        .count() as u32
        + 1
}

fn innermost_node_at_line<'a>(nodes: &'a [Node], line: u32) -> Option<&'a Node> {
    nodes
        .iter()
        .filter(|n| n.start_line <= line && n.end_line.max(n.start_line) >= line)
        .filter(|n| matches!(n.kind, NodeKind::Function | NodeKind::Method))
        .min_by_key(|n| n.end_line.saturating_sub(n.start_line))
        .or_else(|| {
            nodes
                .iter()
                .filter(|n| n.start_line <= line && n.end_line.max(n.start_line) >= line)
                .min_by_key(|n| n.end_line.saturating_sub(n.start_line))
        })
}

struct IndexJob {
    file_path: PathBuf,
    relative_path: String,
}

struct ParsedFile {
    relative_path: String,
    language: Language,
    content: String,
    extraction: ExtractionResult,
    info: SourceFileInfo,
}

#[derive(Clone, Copy)]
struct SourceFileInfo {
    size: u64,
    modified_at: i64,
}

type ParsedFileResult = std::result::Result<Option<ParsedFile>, (String, GrphError)>;

fn parse_index_job(job: IndexJob) -> ParsedFileResult {
    let info = source_file_info(&job.file_path).map_err(|e| (job.relative_path.clone(), e))?;
    let content = fs::read(&job.file_path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .map_err(|e| (job.relative_path.clone(), GrphError::Io(e)))?;

    let Some(language) = detect_language_for_content(&job.file_path, &content) else {
        return Ok(None);
    };

    let extraction =
        ExtractionOrchestrator::extract_content(&job.relative_path, language, &content)
            .map_err(|e| (job.relative_path.clone(), e))?;

    Ok(Some(ParsedFile {
        relative_path: job.relative_path,
        language,
        content,
        extraction,
        info,
    }))
}

fn relative_path(project_root: &Path, file_path: &Path) -> String {
    file_path
        .strip_prefix(project_root)
        .unwrap_or(file_path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn detect_language_for_content(file_path: &Path, content: &str) -> Option<Language> {
    if file_path.extension().and_then(|e| e.to_str()) == Some("sc") {
        detect_language_with_content(file_path, content)
    } else {
        detect_language(file_path)
    }
}

fn source_file_info(path: &Path) -> Result<SourceFileInfo> {
    let metadata = path.metadata()?;
    let modified_at = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(|e| GrphError::InvalidInput(format!("invalid file mtime: {e}")))?
        .as_millis() as i64;
    Ok(SourceFileInfo {
        size: metadata.len(),
        modified_at,
    })
}

pub fn default_index_jobs() -> usize {
    std::thread::available_parallelism()
        .map(|cpus| (cpus.get() / 2).max(1))
        .unwrap_or(1)
}

fn index_worker_stack_size() -> usize {
    std::env::var("GRPH_INDEX_WORKER_STACK_BYTES")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|size| *size >= 1024 * 1024)
        .unwrap_or(INDEX_WORKER_STACK_SIZE)
}
