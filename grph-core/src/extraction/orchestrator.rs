use crate::db::Database;
use crate::errors::{GrphError, Result};
use crate::extraction::grammars::{detect_language, detect_language_with_content};
use crate::extraction::languages::extract_for_language;
use crate::extraction::tree_sitter::ExtractionResult;
use crate::types::{
    Edge, EdgeKind, FileRecord, IndexProgress, IndexResult, Language, Node, SyncResult,
    UnresolvedRef,
};
use ignore::WalkBuilder;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_FILE_SIZE: u64 = 1_000_000; // 1MB
const INDEX_WORKER_STACK_SIZE: usize = 16 * 1024 * 1024;

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
        let files = self.scan_files()?;
        let total = files.len() as u64;

        let mut total_nodes = 0u64;
        let mut total_edges = 0u64;
        let mut files_indexed = 0u64;

        self.prune_unscanned_files(&files)?;

        let jobs = jobs.max(1).min(files.len().max(1));
        if jobs == 1 || files.len() <= 1 {
            for (i, file_path) in files.iter().enumerate() {
                let relative_path = relative_path(&self.project_root, file_path);

                progress(IndexProgress {
                    current: i as u64 + 1,
                    total,
                    phase: "parsing".to_string(),
                    current_file: Some(relative_path.clone()),
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

        for i in 0..files.len() {
            match result_rx.recv() {
                Ok(Ok(Some(parsed))) => {
                    progress(IndexProgress {
                        current: i as u64 + 1,
                        total,
                        phase: "storing".to_string(),
                        current_file: Some(parsed.relative_path.clone()),
                    });
                    match self.store_extraction(parsed) {
                        Ok((nodes, edges)) => {
                            total_nodes += nodes as u64;
                            total_edges += edges as u64;
                            files_indexed += 1;
                        }
                        Err(e) => eprintln!("WARN: Failed to store parsed file: {}", e),
                    }
                }
                Ok(Ok(None)) => {
                    progress(IndexProgress {
                        current: i as u64 + 1,
                        total,
                        phase: "skipping".to_string(),
                        current_file: None,
                    });
                }
                Ok(Err((path, e))) => {
                    progress(IndexProgress {
                        current: i as u64 + 1,
                        total,
                        phase: "parsing".to_string(),
                        current_file: Some(path.clone()),
                    });
                    eprintln!("WARN: Failed to parse {}: {}", path, e);
                }
                Err(e) => return Err(GrphError::Extraction(format!("Index worker failed: {e}"))),
            }
        }

        progress(IndexProgress {
            current: total,
            total,
            phase: "complete".to_string(),
            current_file: None,
        });

        Ok(IndexResult {
            files_indexed,
            nodes_created: total_nodes,
            edges_created: total_edges,
        })
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
        let relative_path = parsed.relative_path;
        let language = parsed.language;
        let content = parsed.content;
        let extraction = parsed.extraction;
        let info = parsed.info;
        let node_count = extraction.nodes.len();
        let edge_count = extraction.edges.len();

        // Re-indexing the same file must replace its old graph fragment.
        self.db.delete_file_nodes(&relative_path)?;
        self.db.delete_unresolved_refs_for_file(&relative_path)?;

        // Wrap the entire file update in a single transaction. Without this,
        // SQLite auto-commits after every INSERT — one fsync per row. For a
        // typical file with 50 nodes + 30 edges that's 80 fsyncs instead of 1.
        self.db
            .conn()
            .execute_batch("PRAGMA foreign_keys = OFF; BEGIN TRANSACTION")?;

        for node in &extraction.nodes {
            self.db.insert_node(node)?;
        }

        for edge in &extraction.edges {
            self.db.insert_edge(edge)?;
        }

        // Capture unresolved references: edges whose targets are still bare names
        // (couldn't be resolved to node IDs during local resolution). These will
        // be resolved cross-file by the post-indexing resolution pass.
        let node_ids: HashSet<&str> = extraction.nodes.iter().map(|n| n.id.as_str()).collect();
        for edge in &extraction.edges {
            if !node_ids.contains(edge.target.as_str()) {
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

        self.db
            .conn()
            .execute_batch("COMMIT; PRAGMA foreign_keys = ON")?;

        // Update file record
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

    pub fn hash_content(content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        hex::encode(hasher.finalize())
    }
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
