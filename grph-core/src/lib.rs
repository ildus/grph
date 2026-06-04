pub mod compile_commands;
pub mod context;
pub mod ctags;
pub mod db;
pub mod errors;
pub mod extraction;
pub mod graph;
pub mod resolution;
pub mod search;
pub mod types;
pub mod utils;

pub use compile_commands::detect_compile_commands;
pub use context::ContextBuilder;
pub use ctags::CtagsGenerator;
pub use db::Database;
pub use errors::{GrphError, Result};
pub use extraction::ExtractionOrchestrator;
pub use graph::GraphTraverser;
pub use resolution::ReferenceResolver;
pub use search::SearchQuery;
pub use types::{Edge, EdgeKind, FileRecord, GraphStats, Language, Node, NodeKind};

/// Main struct for interacting with Grph
pub struct Grph {
    db: Database,
    project_root: std::path::PathBuf,
}

impl Grph {
    /// Initialize Grph in a project directory
    pub fn init(project_root: &std::path::Path) -> Result<Self> {
        let db = Database::open(project_root)?;
        db.init_schema()?;
        db.enable_wal()?;
        Ok(Self {
            db,
            project_root: project_root.to_path_buf(),
        })
    }

    /// Open an existing Grph database
    pub fn open(project_root: &std::path::Path) -> Result<Self> {
        let db = Database::open(project_root)?;
        Ok(Self {
            db,
            project_root: project_root.to_path_buf(),
        })
    }

    /// Get the database connection
    pub fn db(&self) -> &Database {
        &self.db
    }

    /// Get the project root
    pub fn project_root(&self) -> &std::path::Path {
        &self.project_root
    }

    /// Run extraction/indexing
    pub fn index(
        &mut self,
        progress: impl Fn(extraction::IndexProgress),
    ) -> Result<extraction::IndexResult> {
        self.index_with_jobs(extraction::orchestrator::default_index_jobs(), progress)
    }

    /// Run extraction/indexing with an explicit parsing worker count.
    pub fn index_with_jobs(
        &mut self,
        jobs: usize,
        progress: impl Fn(extraction::IndexProgress),
    ) -> Result<extraction::IndexResult> {
        self.index_with_jobs_and_resolve(jobs, true, progress)
    }

    /// Run extraction/indexing with explicit control over the post-index
    /// cross-file resolver. Large macro-heavy C projects may prefer to skip
    /// resolution during indexing and run `grph sync --resolve` later.
    pub fn index_with_jobs_and_resolve(
        &mut self,
        jobs: usize,
        resolve: bool,
        progress: impl Fn(extraction::IndexProgress),
    ) -> Result<extraction::IndexResult> {
        self.index_with_jobs_resolve_and_compile_commands(jobs, resolve, None, progress)
    }

    /// Run extraction/indexing with an optional compile_commands.json hint for
    /// C-family resolution. This does not restrict the scan; it only records the
    /// compile database for resolver ranking/include lookup and AI context notes.
    pub fn index_with_jobs_resolve_and_compile_commands(
        &mut self,
        jobs: usize,
        resolve: bool,
        compile_commands: Option<&std::path::Path>,
        progress: impl Fn(extraction::IndexProgress),
    ) -> Result<extraction::IndexResult> {
        self.index_with_jobs_force_resolve_and_compile_commands(
            jobs,
            false,
            resolve,
            compile_commands,
            progress,
        )
    }

    pub fn index_with_jobs_force_resolve_and_compile_commands(
        &mut self,
        jobs: usize,
        force: bool,
        resolve: bool,
        compile_commands: Option<&std::path::Path>,
        progress: impl Fn(extraction::IndexProgress),
    ) -> Result<extraction::IndexResult> {
        if let Some(path) = compile_commands {
            let stored = if path.is_absolute() {
                path.strip_prefix(&self.project_root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .replace('\\', "/")
            } else {
                path.to_string_lossy().replace('\\', "/")
            };
            self.db
                .set_project_metadata("index.compile_commands.path", &stored)?;
        } else {
            self.db
                .delete_project_metadata("index.compile_commands.path")?;
        }

        let mut orchestrator =
            ExtractionOrchestrator::new(self.db.clone(), self.project_root.clone())?;
        let result = orchestrator.index_all_with_jobs_and_force(jobs, force, progress)?;

        if resolve {
            self.resolve_cross_file_refs_if_needed();
        }

        Ok(result)
    }

    /// Sync changed files
    pub fn sync(&mut self) -> Result<extraction::SyncResult> {
        let mut orchestrator =
            ExtractionOrchestrator::new(self.db.clone(), self.project_root.clone())?;
        let result = orchestrator.sync()?;

        // Run cross-file reference resolution only if files actually changed.
        if result.files_changed > 0 || result.files_added > 0 || result.files_deleted > 0 {
            self.resolve_cross_file_refs_if_needed();
        }

        Ok(result)
    }

    /// Sync one file and resolve only references emitted by that file.
    pub fn sync_file(&mut self, file_path: &std::path::Path) -> Result<extraction::SyncResult> {
        let mut orchestrator =
            ExtractionOrchestrator::new(self.db.clone(), self.project_root.clone())?;
        let result = orchestrator.sync_file(file_path)?;

        if result.files_changed > 0 || result.files_added > 0 {
            let relative_path = if file_path.is_absolute() {
                file_path
                    .strip_prefix(&self.project_root)
                    .unwrap_or(file_path)
                    .to_string_lossy()
                    .replace('\\', "/")
            } else {
                file_path.to_string_lossy().replace('\\', "/")
            };
            let mut resolver =
                resolution::ReferenceResolver::new(self.db.clone(), self.project_root.clone());
            let resolved = resolver.resolve_file(&relative_path)?;
            if resolved.resolved > 0 || resolved.unresolved > 0 {
                eprintln!(
                    "Resolved {}/{} file references across {}/{} groups ({} refs unresolved in batch, {} total remaining)",
                    resolved.resolved,
                    resolved.total,
                    resolved.resolved_groups,
                    resolved.total_groups,
                    resolved.unresolved,
                    resolved.remaining
                );
            }
        }

        Ok(result)
    }

    /// Resolve cross-file references — only prints if there's work to do.
    fn resolve_cross_file_refs_if_needed(&self) {
        let unresolved_count = match self.db.count_pending_unresolved_refs() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("WARN: Failed to count unresolved refs: {}", e);
                return;
            }
        };
        if unresolved_count == 0 {
            return;
        }
        match self.resolve_pending_refs(100_000) {
            Ok(result) => print_resolution_result("cross-file", &result),
            Err(e) => eprintln!("WARN: Cross-file reference resolution failed: {}", e),
        }
    }

    /// Resolve pending cross-file references without re-indexing.
    pub fn resolve_pending_refs(&self, group_limit: u32) -> Result<resolution::ResolutionResult> {
        let mut resolver =
            resolution::ReferenceResolver::new(self.db.clone(), self.project_root.clone());
        resolver.resolve_all_with_limit(group_limit)
    }

    /// Resolve pending references for one file without re-indexing it.
    pub fn resolve_pending_refs_for_file(
        &self,
        file_path: &std::path::Path,
        group_limit: u32,
    ) -> Result<resolution::ResolutionResult> {
        let relative_path = if file_path.is_absolute() {
            file_path
                .strip_prefix(&self.project_root)
                .unwrap_or(file_path)
                .to_string_lossy()
                .replace('\\', "/")
        } else {
            file_path.to_string_lossy().replace('\\', "/")
        };
        let mut resolver =
            resolution::ReferenceResolver::new(self.db.clone(), self.project_root.clone());
        resolver.resolve_file_with_limit(&relative_path, group_limit)
    }

    /// Build context for an AI task
    pub fn build_context(&self, task: &str, max_nodes: u32, include_code: bool) -> Result<String> {
        let builder =
            ContextBuilder::new_with_root(self.db.clone(), Some(self.project_root.clone()));
        let mut context = builder.build_context(
            task,
            max_nodes,
            include_code,
            context::OutputFormat::Markdown,
        )?;
        if let Ok(Some(path)) = self.db.get_project_metadata("index.compile_commands.path") {
            context = format!(
                "> Index note: compile_commands.json hint '{}' was used for C-family resolution. The repository scan was not restricted; resolver preferences may reflect that build/platform configuration.

{}",
                path, context
            );
        } else if let Some(path) = compile_commands::detect_compile_commands(&self.project_root) {
            context = format!(
                "> Index note: detected '{}'. It was not used for this index. Run 'grph index --compile-commands {}' to use it as a C-family resolver hint.

{}",
                path.display(), path.display(), context
            );
        }
        Ok(context)
    }

    /// Search for nodes
    pub fn search(
        &self,
        query: &str,
        kind: Option<types::NodeKind>,
        limit: u32,
    ) -> Result<Vec<Node>> {
        self.db.search_nodes(query, kind, limit)
    }

    /// Get graph traverser
    pub fn traverser(&self) -> GraphTraverser {
        GraphTraverser::new(self.db.clone())
    }

    /// Get stats
    pub fn stats(&self) -> Result<GraphStats> {
        self.db.get_stats()
    }

    /// Generate a Universal Ctags-compatible tags file from the current index.
    pub fn generate_ctags(&self, path: &std::path::Path) -> Result<usize> {
        CtagsGenerator::new(self.db.clone()).generate_to_file(path)
    }
}

fn print_resolution_result(label: &str, result: &resolution::ResolutionResult) {
    if result.total == 0 && result.remaining == 0 {
        return;
    }
    eprintln!(
        "Resolved {}/{} {} references across {}/{} groups ({} refs unresolved in batch, {} total remaining)",
        result.resolved,
        result.total,
        label,
        result.resolved_groups,
        result.total_groups,
        result.unresolved,
        result.remaining
    );
}
