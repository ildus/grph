use crate::db::Database;
use crate::errors::Result;
use crate::graph::GraphTraverser;
use crate::resolution::builtins::is_builtin;
use crate::types::{Edge, EdgeKind, Language, Node, NodeKind};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Markdown,
    Json,
}

#[derive(Debug, Clone)]
struct ContextOptions {
    max_nodes: usize,
    search_limit: usize,
    traversal_depth: usize,
    max_code_blocks: usize,
    max_code_block_size: usize,
    max_output_chars: usize,
    include_code: bool,
}

impl ContextOptions {
    fn new(max_nodes: u32, include_code: bool) -> Self {
        let max_nodes = max_nodes.max(1) as usize;
        Self {
            max_nodes,
            search_limit: max_nodes.clamp(1, 5),
            traversal_depth: 1,
            max_code_blocks: 5.min(max_nodes),
            max_code_block_size: 1_500,
            max_output_chars: 18_000,
            include_code,
        }
    }

    fn apply_project_size(&mut self, file_count: u64) {
        match file_count {
            0..=499 => {
                self.max_output_chars = 16_000;
                self.max_code_blocks = self.max_code_blocks.min(5);
                self.max_code_block_size = self.max_code_block_size.min(1_400);
            }
            500..=4_999 => {
                self.max_output_chars = 24_000;
                self.max_code_blocks = self.max_code_blocks.max(6).min(self.max_nodes);
                self.max_code_block_size = 1_800;
            }
            _ => {
                self.max_output_chars = 32_000;
                self.max_code_blocks = self.max_code_blocks.max(8).min(self.max_nodes);
                self.max_code_block_size = 2_000;
            }
        }
    }
}

#[derive(Debug, Clone)]
struct ScoredNode {
    node: Node,
    score: f64,
    reason: String,
}

#[derive(Debug, Clone)]
struct CodeBlock {
    node: Node,
    start_line: u32,
    end_line: u32,
    content: String,
    truncated: bool,
}

#[derive(Debug, Clone)]
struct TaskContext {
    query: String,
    entry_points: Vec<ScoredNode>,
    nodes: Vec<ScoredNode>,
    edges: Vec<Edge>,
    code_blocks: Vec<CodeBlock>,
    related_files: Vec<String>,
    call_paths: Vec<Vec<String>>,
    truncated: bool,
}

pub struct ContextBuilder {
    db: Database,
    traverser: GraphTraverser,
    project_root: Option<PathBuf>,
}

impl ContextBuilder {
    pub fn new(db: Database) -> Self {
        Self::new_with_root(db, None::<PathBuf>)
    }

    pub fn new_with_root(db: Database, project_root: Option<impl Into<PathBuf>>) -> Self {
        let traverser = GraphTraverser::new(db.clone());
        Self {
            db,
            traverser,
            project_root: project_root.map(Into::into),
        }
    }

    /// Build bounded, ranked context for an AI agent.
    ///
    /// This intentionally mirrors the mature codegraph pipeline at a smaller
    /// scale: hybrid retrieval, relevance scoring, shallow graph expansion,
    /// compact source snippets, and call-path hints in one primary entry point.
    pub fn build_context(
        &self,
        task_description: &str,
        max_nodes: u32,
        include_code: bool,
        format: OutputFormat,
    ) -> Result<String> {
        let mut opts = ContextOptions::new(max_nodes, include_code);
        if let Ok(file_count) = self.db.count_files() {
            opts.apply_project_size(file_count);
        }
        let context = self.build_task_context(task_description, &opts)?;

        if context.nodes.is_empty() {
            return Ok("No relevant symbols found.".to_string());
        }

        match format {
            OutputFormat::Markdown => Ok(truncate_output(
                self.format_context_markdown(&context),
                opts.max_output_chars,
            )),
            OutputFormat::Json => self.format_context_json(&context),
        }
    }

    fn build_task_context(&self, query: &str, opts: &ContextOptions) -> Result<TaskContext> {
        let terms = extract_search_terms(query);
        let mut entry_points = self.hybrid_search(query, opts)?;
        if entry_points.is_empty() {
            return Ok(TaskContext {
                query: query.to_string(),
                entry_points: Vec::new(),
                nodes: Vec::new(),
                edges: Vec::new(),
                code_blocks: Vec::new(),
                related_files: Vec::new(),
                call_paths: Vec::new(),
                truncated: false,
            });
        }

        entry_points.sort_by(|a, b| b.score.total_cmp(&a.score));
        entry_points = shape_entry_points(entry_points, opts.search_limit, &terms);

        let root_ids: HashSet<String> = entry_points.iter().map(|r| r.node.id.clone()).collect();
        let mut scored: HashMap<String, ScoredNode> = HashMap::new();
        let mut edges = Vec::new();
        let mut seen_edges = HashSet::new();

        for result in &entry_points {
            upsert_scored(&mut scored, result.clone());
        }

        // Ranked graph expansion. Direct calls matter most; references/imports
        // can still reveal framework or wiring context without requiring the
        // agent to know which follow-up tool to call.
        let mut frontier: Vec<ScoredNode> = entry_points.clone();
        for depth in 0..opts.traversal_depth {
            let mut next = Vec::new();
            for current in &frontier {
                let decay = if depth == 0 { 0.72 } else { 0.45 };
                for (node, edge, label) in self.neighbors(&current.node, 5)? {
                    add_edge(&mut edges, &mut seen_edges, edge);
                    let mut score = current.score * decay;
                    if root_ids.contains(&node.id) {
                        score += 25.0;
                    }
                    score += kind_weight(node.kind);
                    if is_test_file(&node.file_path) && !query_mentions_tests(query) {
                        score *= 0.35;
                    }
                    let candidate = ScoredNode {
                        node,
                        score,
                        reason: label,
                    };
                    let id = candidate.node.id.clone();
                    let inserted_or_improved = scored
                        .get(&id)
                        .map(|old| candidate.score > old.score)
                        .unwrap_or(true);
                    if inserted_or_improved {
                        upsert_scored(&mut scored, candidate.clone());
                        next.push(candidate);
                    }
                }
            }
            frontier = next;
        }

        let mut nodes: Vec<ScoredNode> = scored.into_values().collect();
        nodes.sort_by(|a, b| b.score.total_cmp(&a.score));
        let truncated = nodes.len() > opts.max_nodes;
        nodes.truncate(opts.max_nodes);
        apply_file_diversity_cap(&mut nodes, &root_ids, opts.max_nodes);
        apply_non_prod_cap(&mut nodes, &root_ids, opts.max_nodes, query);
        prune_low_information_related_nodes(&mut nodes, &root_ids, &terms);
        self.add_relationship_context(&mut nodes, &root_ids, &terms, opts.max_nodes)?;
        nodes.sort_by(|a, b| b.score.total_cmp(&a.score));
        nodes.truncate(opts.max_nodes);

        let kept_ids: HashSet<String> = nodes.iter().map(|r| r.node.id.clone()).collect();
        edges.retain(|e| edge_between_kept(e, &kept_ids, &nodes));
        self.recover_edges_between_kept(&nodes, &kept_ids, &mut edges, &mut seen_edges)?;

        let mut related_files = Vec::new();
        for node in &nodes {
            if !related_files.iter().any(|p| p == &node.node.file_path) {
                related_files.push(node.node.file_path.clone());
            }
        }

        let code_blocks = if opts.include_code {
            self.extract_code_blocks(&nodes, &edges, &terms, opts)?
        } else {
            Vec::new()
        };
        let call_paths = build_call_paths(&nodes, &edges, &root_ids);

        Ok(TaskContext {
            query: query.to_string(),
            entry_points,
            nodes,
            edges,
            code_blocks,
            related_files,
            call_paths,
            truncated,
        })
    }

    fn hybrid_search(&self, query: &str, opts: &ContextOptions) -> Result<Vec<ScoredNode>> {
        let symbols = extract_symbols_from_query(query);
        let terms = extract_search_terms(query);
        let mut results: HashMap<String, ScoredNode> = HashMap::new();
        let mut exact_ids: HashSet<String> = HashSet::new();

        // Exact symbol matches: highest confidence.
        for symbol in &symbols {
            if let Some(node) = self.db.get_node_by_name_any(symbol)? {
                exact_ids.insert(node.id.clone());
                let score =
                    120.0 + kind_weight(node.kind) + path_relevance(&node.file_path, &terms);
                upsert_scored(
                    &mut results,
                    ScoredNode {
                        node,
                        score,
                        reason: format!("exact `{}`", symbol),
                    },
                );
            }
        }

        // LIKE/FTS-ish search for natural language terms and symbol prefixes.
        let mut search_terms = terms.clone();
        for symbol in &symbols {
            if !search_terms.iter().any(|t| t.eq_ignore_ascii_case(symbol)) {
                search_terms.push(symbol.clone());
            }
            for variant in stem_variants(symbol) {
                if !search_terms
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case(&variant))
                {
                    search_terms.push(variant);
                }
            }
        }

        for term in &search_terms {
            for node in self
                .db
                .search_nodes(term, None, (opts.search_limit * 4) as u32)?
            {
                if !high_value_kind(node.kind) {
                    continue;
                }
                let mut score =
                    55.0 + lexical_score(&node, term) + path_relevance(&node.file_path, &terms);
                if is_test_file(&node.file_path) && !query_mentions_tests(query) {
                    score *= 0.35;
                }
                upsert_scored(
                    &mut results,
                    ScoredNode {
                        node,
                        score,
                        reason: format!("search `{}`", term),
                    },
                );
            }
        }

        // Definition-prefix boost: "rest" should find RestController even when
        // there is no symbol named exactly "rest".
        for term in &search_terms {
            let title = title_case(term);
            if title.len() < 3 {
                continue;
            }
            for node in self
                .db
                .search_nodes(&title, None, (opts.search_limit * 3) as u32)?
            {
                if !definition_kind(node.kind)
                    || !node.name.to_lowercase().starts_with(&title.to_lowercase())
                {
                    continue;
                }
                let score = 75.0
                    + kind_weight(node.kind)
                    + (10.0 - ((node.name.len().saturating_sub(title.len())) as f64 / 3.0))
                        .max(0.0)
                    + path_relevance(&node.file_path, &terms);
                upsert_scored(
                    &mut results,
                    ScoredNode {
                        node,
                        score,
                        reason: format!("definition prefix `{}`", title),
                    },
                );
            }
        }

        // Compound/CamelCase-ish substring matching. FTS/LIKE prefix ranking can
        // miss symbols where query terms appear inside a larger identifier.
        self.add_compound_name_matches(&mut results, &search_terms, &terms, opts, query)?;

        // Content-aware fallback. Agents often ask using product words, error strings,
        // comments, config keys, or literal text that never appears in symbol names.
        // Scan indexed files and promote symbols from files whose source contains
        // multiple query terms, mirroring codegraph's broader retrieval behavior.
        for candidate in self.content_search(query, &terms, opts)? {
            upsert_scored(&mut results, candidate);
        }

        let mut values: Vec<ScoredNode> = results.into_values().collect();
        apply_colocation_boost(&mut values, &symbols);
        apply_multi_term_boost(&mut values, &terms, &exact_ids);
        values.sort_by(|a, b| b.score.total_cmp(&a.score));
        values.truncate(opts.max_nodes.max(opts.search_limit));
        Ok(values)
    }

    fn add_compound_name_matches(
        &self,
        results: &mut HashMap<String, ScoredNode>,
        search_terms: &[String],
        terms: &[String],
        opts: &ContextOptions,
        query: &str,
    ) -> Result<()> {
        let groups = term_groups(terms);
        if groups.len() < 2 {
            return Ok(());
        }
        let mut by_id: HashMap<String, (Node, HashSet<usize>)> = HashMap::new();
        for term in search_terms.iter().filter(|t| t.len() >= 3).take(16) {
            for node in self.db.search_nodes(term, None, 200)? {
                if !definition_kind(node.kind)
                    && !matches!(node.kind, NodeKind::Function | NodeKind::Method)
                {
                    continue;
                }
                if is_test_file(&node.file_path) && !query_mentions_tests(query) {
                    continue;
                }
                let matched = matched_group_indexes(&node, &groups);
                if matched.is_empty() {
                    continue;
                }
                let entry = by_id
                    .entry(node.id.clone())
                    .or_insert((node, HashSet::new()));
                entry.1.extend(matched);
            }
        }
        let mut candidates: Vec<(Node, usize)> = by_id
            .into_values()
            .filter_map(|(node, groups)| {
                if groups.len() >= 2 {
                    Some((node, groups.len()))
                } else {
                    None
                }
            })
            .collect();
        candidates.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| {
                    path_relevance(&b.0.file_path, terms)
                        .total_cmp(&path_relevance(&a.0.file_path, terms))
                })
                .then_with(|| a.0.name.len().cmp(&b.0.name.len()))
        });
        for (node, count) in candidates.into_iter().take(opts.search_limit) {
            let score = 70.0
                + (count as f64 * 24.0)
                + kind_weight(node.kind)
                + path_relevance(&node.file_path, terms)
                + brevity_bonus(&node.name);
            upsert_scored(
                results,
                ScoredNode {
                    node,
                    score,
                    reason: format!("compound name match ({} terms)", count),
                },
            );
        }
        Ok(())
    }

    fn content_search(
        &self,
        query: &str,
        terms: &[String],
        opts: &ContextOptions,
    ) -> Result<Vec<ScoredNode>> {
        if terms.is_empty() {
            return Ok(Vec::new());
        }

        let mut file_hits: Vec<(String, f64, Vec<String>, Vec<u32>)> = Vec::new();
        for (path, rank) in self
            .db
            .search_file_contents(query, (opts.search_limit * 4) as u32)?
        {
            let source_path = self.resolve_source_path(&path);
            let Ok(content) = std::fs::read_to_string(source_path) else {
                continue;
            };
            let (matched_terms, matched_lines, proximity_score) =
                content_proximity_hits(&content, terms);
            if matched_terms.is_empty() {
                continue;
            }
            let path_score = path_relevance(&path, terms);
            let term_score = (matched_terms.len() as f64) * 22.0;
            file_hits.push((
                path,
                40.0 + rank + path_score + term_score + proximity_score,
                matched_terms,
                matched_lines,
            ));
        }

        // Compatibility fallback for indexes created before files_fts was
        // populated. This keeps old projects useful until the next sync/index.
        if file_hits.is_empty() {
            for file in self.db.list_files(None)? {
                let path_score = path_relevance(&file.path, terms);
                let path = self.resolve_source_path(&file.path);
                let Ok(content) = std::fs::read_to_string(path) else {
                    continue;
                };
                let (matched_terms, matched_lines, proximity_score) =
                    content_proximity_hits(&content, terms);
                if matched_terms.is_empty() {
                    continue;
                }
                let score = path_score + (matched_terms.len() as f64 * 20.0) + proximity_score;
                file_hits.push((file.path, score, matched_terms, matched_lines));
            }
        }

        file_hits.sort_by(|a, b| {
            b.1.total_cmp(&a.1)
                .then_with(|| b.2.len().cmp(&a.2.len()))
                .then_with(|| a.0.cmp(&b.0))
        });
        file_hits.truncate(opts.search_limit * 3);

        let mut out = Vec::new();
        for (file, file_score, matched_terms, matched_lines) in file_hits {
            let mut nodes = self.db.list_nodes_by_file(&file)?;
            nodes.retain(|n| high_value_kind(n.kind));
            let file_nodes = nodes.clone();
            let mut scored_nodes: Vec<(Node, f64, usize)> = nodes
                .into_iter()
                .map(|node| {
                    let node_hits = node_query_overlap(&node, &matched_terms);
                    let distance = distance_to_matched_lines(&node, &matched_lines);
                    let proximity = match distance {
                        Some(0) => 35.0,
                        Some(1..=5) => 24.0,
                        Some(6..=25) => 12.0,
                        Some(_) => 0.0,
                        None => 0.0,
                    };
                    let mut score = 38.0
                        + file_score
                        + kind_weight(node.kind)
                        + implementation_kind_bias(node.kind)
                        + (node_hits as f64 * 18.0)
                        + proximity;
                    if is_test_file(&node.file_path) && !query_mentions_tests(query) {
                        score *= 0.35;
                    }
                    let promoted = promote_leaf_to_enclosing_impl(node, &file_nodes);
                    (promoted, score, node_hits)
                })
                .collect();
            scored_nodes.sort_by(|a, b| {
                b.1.total_cmp(&a.1)
                    .then_with(|| b.2.cmp(&a.2))
                    .then_with(|| a.0.start_line.cmp(&b.0.start_line))
            });

            for (node, node_score, node_hits) in scored_nodes.into_iter().take(5) {
                let reason_terms = format_terms(&matched_terms, 5);
                out.push(ScoredNode {
                    node,
                    score: node_score,
                    reason: format!(
                        "content match (terms: {}; node hits: {})",
                        reason_terms, node_hits
                    ),
                });
            }
        }
        Ok(out)
    }

    fn neighbors(&self, node: &Node, limit: u32) -> Result<Vec<(Node, Edge, String)>> {
        let mut out = Vec::new();
        for (caller, edge) in self.traverser.callers(&node.id, limit)? {
            out.push((caller, edge, "caller".to_string()));
        }
        for (callee, edge) in self.traverser.callees(&node.id, limit)? {
            out.push((callee, edge, "callee".to_string()));
        }
        for (referencer, edge) in self.traverser.references_to(&node.id, limit / 2 + 1)? {
            out.push((referencer, edge, "reference".to_string()));
        }
        Ok(out)
    }

    fn recover_edges_between_kept(
        &self,
        nodes: &[ScoredNode],
        kept_ids: &HashSet<String>,
        edges: &mut Vec<Edge>,
        seen_edges: &mut HashSet<String>,
    ) -> Result<()> {
        for node in nodes.iter().take(80) {
            for edge in self.db.get_edges_for_node(&node.node.id)? {
                if edge_between_kept(&edge, kept_ids, nodes)
                    && matches!(
                        edge.kind,
                        EdgeKind::Calls
                            | EdgeKind::References
                            | EdgeKind::Extends
                            | EdgeKind::Implements
                            | EdgeKind::Overrides
                    )
                {
                    add_edge(edges, seen_edges, edge);
                }
            }
        }
        Ok(())
    }

    fn add_relationship_context(
        &self,
        nodes: &mut Vec<ScoredNode>,
        root_ids: &HashSet<String>,
        terms: &[String],
        max_nodes: usize,
    ) -> Result<()> {
        // Context is most useful when it includes the next implementation hop:
        // non-trivial callers/callees of symbols already selected by retrieval.
        // This is intentionally structural (graph-based), not query/domain
        // heuristic based: it helps any task where the answer is one call away,
        // while filtering ubiquitous libc/header/helper functions that otherwise
        // consume related-symbol and code-block budget.
        // Do not return early when the initial retrieval already filled the
        // node budget.  A large function often has many generic/runtime callees
        // near the top of the extracted call list, while the implementation
        // helper that explains the user's task is just beyond the small default
        // limit.  Collect a wider candidate set below, rank it, and then let the
        // final budget trim replace weaker non-root nodes if a structural
        // neighbor is more informative.
        let existing: HashSet<String> = nodes.iter().map(|n| n.node.id.clone()).collect();
        let roots_first: Vec<ScoredNode> = nodes
            .iter()
            .filter(|n| root_ids.contains(&n.node.id))
            .chain(nodes.iter().filter(|n| !root_ids.contains(&n.node.id)))
            .take(12)
            .cloned()
            .collect();

        let mut additions: HashMap<String, ScoredNode> = HashMap::new();
        for source in roots_first {
            // Pull a generous raw relationship set, then score/filter it
            // ourselves. Database order is not relevance order: in large
            // functions the first few callees are often assertions, error
            // setters, allocators, or runtime helpers, and the domain-relevant
            // helper can be dozens of calls later.
            let relationships = [
                (self.traverser.callees(&source.node.id, 80)?, "callee"),
                (self.traverser.callers(&source.node.id, 40)?, "caller"),
            ];
            for (related, label) in relationships {
                for (node, _edge) in related {
                    if existing.contains(&node.id) || root_ids.contains(&node.id) {
                        continue;
                    }
                    if !high_value_kind(node.kind) || node.kind == NodeKind::File {
                        continue;
                    }
                    if is_low_information_related_symbol(&node, terms) {
                        continue;
                    }
                    let overlap = node_query_overlap(&node, terms) as f64;
                    let same_file = (node.file_path == source.node.file_path) as i32 as f64;
                    let fan_in = self.traverser.callers(&node.id, 25)?.len();
                    let mut score = source.score * 0.58
                        + kind_weight(node.kind)
                        + implementation_kind_bias(node.kind)
                        + overlap * 20.0
                        + same_file * 8.0;
                    score -= relationship_noise_penalty(&node, &source.node, terms, fan_in);
                    if score <= 0.0 {
                        continue;
                    }
                    upsert_scored(
                        &mut additions,
                        ScoredNode {
                            node,
                            score,
                            reason: format!("{} of {}", label, source.node.name),
                        },
                    );
                }
            }
        }

        let mut additions: Vec<ScoredNode> = additions.into_values().collect();
        additions.sort_by(|a, b| b.score.total_cmp(&a.score));
        for addition in additions {
            if !nodes.iter().any(|n| n.node.id == addition.node.id) {
                nodes.push(addition);
            }
        }

        // Keep roots stable, but allow high-value direct relationships to
        // displace lower-scoring retrieval noise. This gives context a reserved
        // "one important hop" budget without increasing output size.
        nodes.sort_by(|a, b| {
            root_ids
                .contains(&b.node.id)
                .cmp(&root_ids.contains(&a.node.id))
                .then_with(|| b.score.total_cmp(&a.score))
                .then_with(|| a.node.start_line.cmp(&b.node.start_line))
        });
        nodes.truncate(max_nodes);
        Ok(())
    }

    fn extract_code_blocks(
        &self,
        nodes: &[ScoredNode],
        edges: &[Edge],
        terms: &[String],
        opts: &ContextOptions,
    ) -> Result<Vec<CodeBlock>> {
        let mut blocks = Vec::new();
        let mut per_file: HashMap<String, usize> = HashMap::new();

        let mut candidates: Vec<&ScoredNode> = nodes
            .iter()
            .filter(|scored| {
                high_value_kind(scored.node.kind) && scored.node.kind != NodeKind::File
            })
            .collect();
        candidates.sort_by(|a, b| {
            code_block_priority(&b.node)
                .cmp(&code_block_priority(&a.node))
                .then_with(|| b.score.total_cmp(&a.score))
                .then_with(|| a.node.start_line.cmp(&b.node.start_line))
        });

        let selected_ids: HashSet<&str> = nodes.iter().map(|n| n.node.id.as_str()).collect();
        let selected_names: HashSet<&str> = nodes.iter().map(|n| n.node.name.as_str()).collect();

        for scored in candidates {
            if blocks.len() >= opts.max_code_blocks {
                break;
            }
            if is_generic_utility_symbol(&scored.node)
                && node_query_overlap(&scored.node, terms) == 0
            {
                continue;
            }
            let file_count = per_file.entry(scored.node.file_path.clone()).or_default();
            if *file_count >= 2 {
                continue;
            }
            let focus_lines =
                call_site_focus_lines(&scored.node, edges, &selected_ids, &selected_names);
            if let Ok((start_line, source)) = self.read_source_slice_relevant(
                &scored.node,
                terms,
                &focus_lines,
                opts.max_code_block_size,
            ) {
                let (content, truncated) = limit_code_block(source, opts.max_code_block_size);
                if content.trim().is_empty() {
                    continue;
                }
                let line_count = content.lines().count().max(1) as u32;
                blocks.push(CodeBlock {
                    node: scored.node.clone(),
                    start_line,
                    end_line: start_line + line_count - 1,
                    content,
                    truncated,
                });
                *file_count += 1;
            }
        }
        Ok(blocks)
    }

    fn format_context_markdown(&self, context: &TaskContext) -> String {
        let mut md = String::new();
        md.push_str("## Code Context\n\n");
        md.push_str(&format!("**Query:** {}\n\n", context.query));
        md.push_str(&format!(
            "**Summary:** {} entry points, {} related symbols, {} edges, {} files{}\n\n",
            context.entry_points.len(),
            context
                .nodes
                .len()
                .saturating_sub(context.entry_points.len()),
            context.edges.len(),
            context.related_files.len(),
            if context.truncated {
                " (truncated to budget)"
            } else {
                ""
            }
        ));

        md.push_str("### Entry Points\n\n");
        for entry in &context.entry_points {
            md.push_str(&format_node_line(entry, true));
        }
        md.push('\n');

        let root_ids: HashSet<&str> = context
            .entry_points
            .iter()
            .map(|r| r.node.id.as_str())
            .collect();
        let related: Vec<&ScoredNode> = context
            .nodes
            .iter()
            .filter(|n| !root_ids.contains(n.node.id.as_str()))
            .take(12)
            .collect();
        if !related.is_empty() {
            md.push_str("### Related Symbols\n\n");
            let mut by_file: HashMap<&str, Vec<&ScoredNode>> = HashMap::new();
            for node in related {
                by_file.entry(&node.node.file_path).or_default().push(node);
            }
            let mut files: Vec<_> = by_file.into_iter().collect();
            files.sort_by(|a, b| a.0.cmp(b.0));
            for (file, nodes) in files {
                let names = nodes
                    .iter()
                    .map(|n| {
                        format!(
                            "{}:{} [{}]",
                            n.node.name,
                            n.node.start_line,
                            compact_reason(&n.reason)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                md.push_str(&format!("- {}: {}\n", file, names));
            }
            md.push('\n');
        }

        if !context.call_paths.is_empty() {
            md.push_str("### Call Paths\n\n");
            md.push_str("Execution flow among key symbols found in this context:\n\n");
            for path in &context.call_paths {
                md.push_str(&format!("- {}\n", path.join(" → ")));
            }
            md.push_str("\n_These paths come from indexed `calls` edges; dynamic dispatch may stop a chain early._\n\n");
        }

        if !context.related_files.is_empty() {
            md.push_str("### Related Files\n\n");
            for file in context.related_files.iter().take(10) {
                md.push_str(&format!("- {}\n", file));
            }
            if context.related_files.len() > 10 {
                md.push_str(&format!(
                    "- ... and {} more\n",
                    context.related_files.len() - 10
                ));
            }
            md.push('\n');
        }

        if !context.code_blocks.is_empty() {
            md.push_str("### Code\n\n");
            for block in &context.code_blocks {
                md.push_str(&format!(
                    "#### {} ({})\n\n```{}\n{}\n```\n",
                    block.node.name,
                    format_file_and_line(&block.node.file_path, block.start_line),
                    block.node.language.as_str(),
                    block.content
                ));
                if block.truncated {
                    md.push_str("_Snippet truncated to context budget._\n");
                }
                md.push('\n');
            }
        }

        md.push_str("---\nUse `grph_node`/`grph_explore` for targeted follow-up source if a listed symbol needs more detail.\n");
        md
    }

    fn format_context_json(&self, context: &TaskContext) -> Result<String> {
        let nodes = context
            .nodes
            .iter()
            .map(|s| {
                json!({
                    "id": s.node.id,
                    "name": s.node.name,
                    "kind": s.node.kind.as_str(),
                    "filePath": s.node.file_path,
                    "startLine": s.node.start_line,
                    "endLine": s.node.end_line,
                    "signature": s.node.signature,
                    "score": s.score,
                    "reason": s.reason,
                })
            })
            .collect::<Vec<_>>();
        let entry_points = context
            .entry_points
            .iter()
            .map(|s| s.node.id.clone())
            .collect::<Vec<_>>();
        let edges = context
            .edges
            .iter()
            .map(|e| {
                json!({
                    "source": e.source,
                    "target": e.target,
                    "kind": e.kind.as_str(),
                    "line": e.line,
                    "provenance": e.provenance,
                })
            })
            .collect::<Vec<_>>();
        let code_blocks = context
            .code_blocks
            .iter()
            .map(|b| {
                json!({
                    "nodeId": b.node.id,
                    "nodeName": b.node.name,
                    "filePath": b.node.file_path,
                    "startLine": b.start_line,
                    "endLine": b.end_line,
                    "language": b.node.language.as_str(),
                    "content": b.content,
                    "truncated": b.truncated,
                })
            })
            .collect::<Vec<_>>();
        Ok(serde_json::to_string_pretty(&json!({
            "query": context.query,
            "summary": {
                "entryPointCount": context.entry_points.len(),
                "nodeCount": context.nodes.len(),
                "edgeCount": context.edges.len(),
                "fileCount": context.related_files.len(),
                "codeBlockCount": context.code_blocks.len(),
                "truncated": context.truncated,
            },
            "entryPoints": entry_points,
            "nodes": nodes,
            "edges": edges,
            "relatedFiles": context.related_files,
            "callPaths": context.call_paths,
            "codeBlocks": code_blocks,
        }))?)
    }

    fn read_source_slice_relevant(
        &self,
        node: &Node,
        terms: &[String],
        focus_lines: &[u32],
        max_chars: usize,
    ) -> Result<(u32, String)> {
        let path = self.resolve_source_path(&node.file_path);
        let content = std::fs::read_to_string(&path)?;
        let lines: Vec<&str> = content.lines().collect();
        if lines.is_empty() {
            return Ok((1, String::new()));
        }

        let start = node.start_line.max(1);
        let inferred = infer_block_end(&lines, node.start_line, node.language);
        let end = node
            .end_line
            .max(inferred)
            .min(lines.len() as u32)
            .max(start);
        let start_idx = (start - 1) as usize;
        let end_idx = end as usize;
        let full = lines[start_idx..end_idx].join("\n");
        if full.len() <= max_chars || (terms.is_empty() && focus_lines.is_empty()) {
            return Ok((start, full));
        }

        let lower_terms: Vec<String> = terms.iter().map(|t| t.to_lowercase()).collect();
        let mut best_line = None;
        let mut best_score = 0usize;
        for idx in start_idx..end_idx {
            let lower = lines[idx].to_lowercase();
            let hits = lower_terms
                .iter()
                .filter(|term| lower.contains(term.as_str()))
                .count();
            let line_no = (idx + 1) as u32;
            let focus_score = focus_lines
                .iter()
                .filter_map(|focus| {
                    focus
                        .checked_sub(line_no)
                        .or_else(|| line_no.checked_sub(*focus))
                })
                .filter(|distance| *distance <= 2)
                .map(|distance| 8usize.saturating_sub(distance as usize * 2))
                .max()
                .unwrap_or(0);
            let score = hits * 10 + focus_score;
            if score > best_score {
                best_score = score;
                best_line = Some(idx);
            }
        }
        let Some(best) = best_line else {
            return Ok((start, full));
        };

        let mut window_start = best.saturating_sub(10).max(start_idx);
        let mut window_end = (best + 11).min(end_idx);
        loop {
            let slice = lines[window_start..window_end].join("\n");
            if slice.len() <= max_chars || window_end <= window_start + 4 {
                return Ok(((window_start + 1) as u32, slice));
            }
            if best - window_start > window_end.saturating_sub(best + 1) {
                window_start += 1;
            } else {
                window_end -= 1;
            }
        }
    }

    fn resolve_source_path(&self, file_path: &str) -> PathBuf {
        let path = Path::new(file_path);
        if path.is_absolute() {
            path.to_path_buf()
        } else if let Some(root) = &self.project_root {
            root.join(path)
        } else {
            path.to_path_buf()
        }
    }
}

fn upsert_scored(map: &mut HashMap<String, ScoredNode>, candidate: ScoredNode) {
    match map.get_mut(&candidate.node.id) {
        Some(existing) if candidate.score > existing.score => *existing = candidate,
        None => {
            map.insert(candidate.node.id.clone(), candidate);
        }
        _ => {}
    }
}

fn add_edge(edges: &mut Vec<Edge>, seen: &mut HashSet<String>, edge: Edge) {
    let key = format!(
        "{}>{}:{}:{:?}:{:?}",
        edge.source,
        edge.target,
        edge.kind.as_str(),
        edge.line,
        edge.col
    );
    if seen.insert(key) {
        edges.push(edge);
    }
}

fn content_proximity_hits(content: &str, terms: &[String]) -> (Vec<String>, Vec<u32>, f64) {
    // Prefer files where query terms appear near each other.  A whole-file
    // unique-term count makes huge files look relevant when one generic term is
    // mentioned in one subsystem and another term thousands of lines away.  A
    // sliding window better matches what agents need: the symbols close to the
    // task-specific prose/comment/code.
    const WINDOW: usize = 80;
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() || terms.is_empty() {
        return (Vec::new(), Vec::new(), 0.0);
    }

    let mut best_terms = Vec::new();
    let mut best_lines = Vec::new();
    let mut best_density = 0usize;

    for start in 0..lines.len() {
        let end = (start + WINDOW).min(lines.len());
        let slice = lines[start..end]
            .join(
                "
",
            )
            .to_lowercase();
        let mut window_terms = Vec::new();
        for term in terms {
            if slice.contains(term.as_str()) && !window_terms.iter().any(|t| t == term) {
                window_terms.push(term.clone());
            }
        }
        if window_terms.len() < best_terms.len() {
            continue;
        }

        let mut hit_lines = Vec::new();
        let mut density = 0usize;
        for (idx, line) in lines[start..end].iter().enumerate() {
            let lower = line.to_lowercase();
            let hits = window_terms
                .iter()
                .filter(|term| lower.contains(term.as_str()))
                .count();
            if hits > 0 {
                hit_lines.push((start + idx + 1) as u32);
                density += hits;
            }
        }

        if window_terms.len() > best_terms.len() || density > best_density {
            best_terms = window_terms;
            best_lines = hit_lines;
            best_density = density;
        }
    }

    // If no compact window contains much, fall back to whole-file hits but with
    // no proximity bonus.  This preserves recall while preventing far-apart
    // matches from outranking local TODOs and implementation code.
    if best_terms.is_empty() {
        let matched_terms = content_term_hits(content, terms);
        let matched_lines = content_matched_lines(content, &matched_terms);
        return (matched_terms, matched_lines, 0.0);
    }

    let proximity_score = (best_terms.len().saturating_sub(1) as f64 * 18.0)
        + (best_density.saturating_sub(best_terms.len()) as f64 * 3.0);
    (best_terms, best_lines, proximity_score)
}

fn content_term_hits(content: &str, terms: &[String]) -> Vec<String> {
    let lower = content.to_lowercase();
    let mut out = Vec::new();
    for term in terms {
        if lower.contains(term.as_str()) && !out.iter().any(|t| t == term) {
            out.push(term.clone());
        }
    }
    out
}

fn content_matched_lines(content: &str, terms: &[String]) -> Vec<u32> {
    let mut out = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let lower = line.to_lowercase();
        if terms.iter().any(|term| lower.contains(term.as_str())) {
            out.push((idx + 1) as u32);
        }
    }
    out
}

fn node_query_overlap(node: &Node, terms: &[String]) -> usize {
    let haystack = format!(
        "{} {} {} {}",
        node.name,
        node.qualified_name,
        node.file_path,
        node.signature.as_deref().unwrap_or("")
    )
    .to_lowercase();
    terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .count()
}

fn distance_to_matched_lines(node: &Node, lines: &[u32]) -> Option<u32> {
    if lines.is_empty() {
        return None;
    }
    let start = node.start_line;
    let end = node.end_line.max(node.start_line);
    lines
        .iter()
        .map(|line| {
            if *line >= start && *line <= end {
                0
            } else if *line < start {
                start - *line
            } else {
                *line - end
            }
        })
        .min()
}

fn format_terms(terms: &[String], limit: usize) -> String {
    let mut shown = terms
        .iter()
        .take(limit)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if terms.len() > limit {
        shown.push_str(", ...");
    }
    shown
}

fn is_generic_utility_symbol(node: &Node) -> bool {
    is_builtin(&node.name, node.language)
}

fn extract_symbols_from_query(query: &str) -> Vec<String> {
    let mut symbols = HashSet::new();
    let re =
        regex::Regex::new(r"\b([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)\b").unwrap();
    for cap in re.captures_iter(query) {
        let token = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
        if token.len() < 3 || common_word(token) {
            continue;
        }
        if token.contains('.') {
            symbols.insert(token.to_string());
            for part in token.split('.') {
                if part.len() >= 3 && !common_word(part) {
                    symbols.insert(part.to_string());
                }
            }
        } else if looks_identifier_like(token) {
            symbols.insert(token.to_string());
        }
    }
    let mut out: Vec<String> = symbols.into_iter().collect();
    out.sort();
    out
}

fn extract_search_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut push = |term: String| {
        let term = term.trim().to_lowercase();
        if term.len() < 3 || common_word(&term) {
            return;
        }
        if !terms.iter().any(|t| t == &term) {
            terms.push(term.clone());
        }
        for variant in stem_variants(&term) {
            if !terms.iter().any(|t| t == &variant) && !common_word(&variant) {
                terms.push(variant);
            }
        }
    };

    let identifier_re =
        regex::Regex::new(r"\b[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*\b").unwrap();
    for m in identifier_re.find_iter(query) {
        let token = m.as_str();
        if token.len() >= 3 && looks_identifier_like(token) && !common_word(token) {
            push(token.replace('.', "_").to_lowercase());
        }
        for part in split_identifier_terms(token) {
            push(part);
        }
    }

    for raw in query.split(|c: char| !c.is_alphanumeric() && c != '_') {
        push(raw.to_string());
    }
    terms
}

fn split_identifier_terms(token: &str) -> Vec<String> {
    let mut normalized = String::new();
    let chars: Vec<char> = token.chars().collect();
    for (i, ch) in chars.iter().enumerate() {
        if *ch == '_' || *ch == '.' || *ch == '-' {
            normalized.push(' ');
            continue;
        }
        if i > 0 {
            let prev = chars[i - 1];
            let next = chars.get(i + 1).copied();
            if ch.is_uppercase()
                && (prev.is_lowercase()
                    || prev.is_ascii_digit()
                    || (prev.is_uppercase() && next.map(|n| n.is_lowercase()).unwrap_or(false)))
            {
                normalized.push(' ');
            }
        }
        normalized.push(*ch);
    }
    normalized
        .split_whitespace()
        .map(|s| s.to_lowercase())
        .filter(|s| s.len() >= 3 && !common_word(s))
        .collect()
}

fn brevity_bonus(name: &str) -> f64 {
    (10.0 - (name.len() as f64 / 6.0)).max(0.0)
}

fn compact_reason(reason: &str) -> String {
    let reason = reason.trim();
    if reason.len() <= 48 {
        reason.to_string()
    } else {
        format!("{}…", &reason[..47])
    }
}

fn endpoint_matches_node(endpoint: &str, node: &Node) -> bool {
    endpoint == node.id || endpoint == node.name || endpoint == node.qualified_name
}

fn endpoint_selected(
    endpoint: &str,
    selected_ids: &HashSet<&str>,
    selected_names: &HashSet<&str>,
) -> bool {
    selected_ids.contains(endpoint) || selected_names.contains(endpoint)
}

fn call_site_focus_lines(
    node: &Node,
    edges: &[Edge],
    selected_ids: &HashSet<&str>,
    selected_names: &HashSet<&str>,
) -> Vec<u32> {
    let mut out = Vec::new();
    for edge in edges {
        if !matches!(edge.kind, EdgeKind::Calls | EdgeKind::References) {
            continue;
        }
        if !endpoint_matches_node(&edge.source, node)
            || !endpoint_selected(&edge.target, selected_ids, selected_names)
        {
            continue;
        }
        let Some(line) = edge.line else {
            continue;
        };
        if line >= node.start_line
            && line <= node.end_line.max(node.start_line)
            && !out.contains(&line)
        {
            out.push(line);
        }
    }
    out
}

fn relationship_noise_penalty(node: &Node, source: &Node, terms: &[String], fan_in: usize) -> f64 {
    if node_query_overlap(node, terms) > 0 || node.file_path == source.file_path {
        return 0.0;
    }
    let mut penalty = 0.0;
    if generic_infrastructure_helper_name(&node.name) {
        penalty += 90.0;
    }
    if fan_in >= 20 {
        penalty += 55.0;
    } else if fan_in >= 10 {
        penalty += 25.0;
    }
    penalty
}

fn generic_infrastructure_helper_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    let parts = split_identifier_terms(&lower);
    let has = |needle: &str| parts.iter().any(|p| p == needle) || lower.contains(needle);
    has("alloc")
        || has("dealloc")
        || has("malloc")
        || has("calloc")
        || has("free")
        || has("lock")
        || has("unlock")
        || has("mutex")
        || has("stamp")
        || has("timestamp")
}

fn weak_entry_point_candidate(scored: &ScoredNode, terms: &[String]) -> bool {
    let reason = scored.reason.to_lowercase();
    reason.contains("single-term dampened") && node_query_overlap(&scored.node, terms) < 2
}

fn code_block_priority(node: &Node) -> i32 {
    match node.kind {
        NodeKind::Function | NodeKind::Method => 4,
        NodeKind::Class | NodeKind::Struct | NodeKind::Interface | NodeKind::Trait => 3,
        NodeKind::Enum | NodeKind::TypeAlias => 2,
        NodeKind::Constant => 1,
        NodeKind::Variable | NodeKind::Field | NodeKind::Property => 0,
        _ => 1,
    }
}

fn apply_file_diversity_cap(
    nodes: &mut Vec<ScoredNode>,
    root_ids: &HashSet<String>,
    max_nodes: usize,
) {
    let max_per_file = 5.max((max_nodes as f64 * 0.20).ceil() as usize);
    let mut counts: HashMap<String, usize> = HashMap::new();
    nodes.retain(|node| {
        let count = counts.entry(node.node.file_path.clone()).or_default();
        if *count < max_per_file || root_ids.contains(&node.node.id) {
            *count += 1;
            true
        } else {
            false
        }
    });
}

fn apply_non_prod_cap(
    nodes: &mut Vec<ScoredNode>,
    root_ids: &HashSet<String>,
    max_nodes: usize,
    query: &str,
) {
    if query_mentions_tests(query) {
        return;
    }
    let max_non_prod = 3.max((max_nodes as f64 * 0.15).ceil() as usize);
    let mut count = 0usize;
    nodes.retain(|node| {
        if !is_non_prod_file(&node.node.file_path) || root_ids.contains(&node.node.id) {
            return true;
        }
        if count < max_non_prod {
            count += 1;
            true
        } else {
            false
        }
    });
}

fn edge_between_kept(edge: &Edge, kept_ids: &HashSet<String>, nodes: &[ScoredNode]) -> bool {
    if kept_ids.contains(&edge.source) && kept_ids.contains(&edge.target) {
        return true;
    }
    let name_to_id: HashMap<&str, &str> = nodes
        .iter()
        .map(|n| (n.node.name.as_str(), n.node.id.as_str()))
        .collect();
    let source_kept = kept_ids.contains(&edge.source)
        || name_to_id
            .get(edge.source.as_str())
            .map(|id| kept_ids.contains(*id))
            .unwrap_or(false);
    let target_kept = kept_ids.contains(&edge.target)
        || name_to_id
            .get(edge.target.as_str())
            .map(|id| kept_ids.contains(*id))
            .unwrap_or(false);
    source_kept && target_kept
}

fn prune_low_information_related_nodes(
    nodes: &mut Vec<ScoredNode>,
    root_ids: &HashSet<String>,
    terms: &[String],
) {
    nodes.retain(|node| {
        root_ids.contains(&node.node.id) || !is_low_information_related_symbol(&node.node, terms)
    });
}

fn is_low_information_related_symbol(node: &Node, terms: &[String]) -> bool {
    if node_query_overlap(node, terms) > 0 {
        return false;
    }
    if is_generic_infrastructure_symbol(node) {
        return true;
    }
    let lower_path = node.file_path.to_lowercase();
    if (lower_path.contains("/hdr/") || lower_path.ends_with(".h"))
        && matches!(
            node.kind,
            NodeKind::Function | NodeKind::Variable | NodeKind::Constant | NodeKind::Field
        )
    {
        return true;
    }
    false
}

fn is_generic_infrastructure_symbol(node: &Node) -> bool {
    is_generic_utility_symbol(node)
        || common_runtime_helper_name(&node.name)
        || generic_infrastructure_helper_name(&node.name)
}

fn common_runtime_helper_name(name: &str) -> bool {
    matches!(
        name,
        "abort"
            | "atoi"
            | "calloc"
            | "close"
            | "dup"
            | "dup2"
            | "exit"
            | "free"
            | "fprintf"
            | "fputs"
            | "getenv"
            | "longjmp"
            | "malloc"
            | "memcpy"
            | "memmove"
            | "memset"
            | "open"
            | "printf"
            | "read"
            | "realloc"
            | "setenv"
            | "setjmp"
            | "snprintf"
            | "sprintf"
            | "strcat"
            | "strcmp"
            | "strcpy"
            | "strlen"
            | "strncmp"
            | "write"
            | "CVal"
            | "CVan"
            | "MEfree"
            | "NMgtAt"
            | "STcompare"
            | "STcopy"
            | "STncmp"
    )
}

fn promote_leaf_to_enclosing_impl(node: Node, file_nodes: &[Node]) -> Node {
    if !matches!(
        node.kind,
        NodeKind::Variable | NodeKind::Field | NodeKind::Property | NodeKind::Parameter
    ) {
        return node;
    }
    file_nodes
        .iter()
        .filter(|candidate| {
            matches!(
                candidate.kind,
                NodeKind::Function
                    | NodeKind::Method
                    | NodeKind::Class
                    | NodeKind::Struct
                    | NodeKind::Interface
                    | NodeKind::Trait
            ) && candidate.start_line <= node.start_line
                && candidate.end_line.max(candidate.start_line) >= node.start_line
                && candidate.id != node.id
        })
        .min_by_key(|candidate| candidate.end_line.saturating_sub(candidate.start_line))
        .cloned()
        .unwrap_or(node)
}

fn shape_entry_points(
    mut candidates: Vec<ScoredNode>,
    limit: usize,
    terms: &[String],
) -> Vec<ScoredNode> {
    // Codegraph-style root hygiene: entry points are scarce. Prefer
    // implementation-bearing symbols, avoid variables when their enclosing
    // function is present, and avoid letting one file consume all roots. Weak
    // single-term content hits are useful supporting context, but should only
    // become roots after stronger exact/compound/multi-term candidates.
    candidates.sort_by(|a, b| {
        let a_weak = weak_entry_point_candidate(a, terms);
        let b_weak = weak_entry_point_candidate(b, terms);
        a_weak
            .cmp(&b_weak)
            .then_with(|| entry_point_priority(&b.node).cmp(&entry_point_priority(&a.node)))
            .then_with(|| b.score.total_cmp(&a.score))
            .then_with(|| a.node.start_line.cmp(&b.node.start_line))
    });
    let max_per_file = 2usize.max(limit / 3);
    let mut per_file: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for candidate in candidates.iter() {
        if out.len() >= limit {
            break;
        }
        if !seen.insert(candidate.node.id.clone()) {
            continue;
        }
        if entry_point_priority(&candidate.node) <= 0
            && candidates.iter().any(|other| {
                other.node.file_path == candidate.node.file_path
                    && entry_point_priority(&other.node) > 0
                    && other.node.start_line <= candidate.node.start_line
                    && other.node.end_line.max(other.node.start_line) >= candidate.node.start_line
            })
        {
            continue;
        }
        let count = per_file
            .entry(candidate.node.file_path.clone())
            .or_default();
        if *count >= max_per_file {
            continue;
        }
        *count += 1;
        out.push(candidate.clone());
    }
    if out.len() < limit {
        for candidate in candidates {
            if out.len() >= limit {
                break;
            }
            if !out.iter().any(|kept| kept.node.id == candidate.node.id) {
                out.push(candidate);
            }
        }
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out
}

fn entry_point_priority(node: &Node) -> i32 {
    match node.kind {
        NodeKind::Function | NodeKind::Method => 4,
        NodeKind::Class | NodeKind::Struct | NodeKind::Interface | NodeKind::Trait => 3,
        NodeKind::Enum | NodeKind::TypeAlias | NodeKind::Module | NodeKind::Namespace => 2,
        NodeKind::Constant => 1,
        NodeKind::Variable | NodeKind::Field | NodeKind::Property => 0,
        _ => 1,
    }
}

fn common_word(word: &str) -> bool {
    matches!(
        word.to_lowercase().as_str(),
        "the"
            | "and"
            | "for"
            | "with"
            | "from"
            | "this"
            | "that"
            | "have"
            | "been"
            | "will"
            | "would"
            | "could"
            | "should"
            | "does"
            | "done"
            | "make"
            | "made"
            | "use"
            | "used"
            | "using"
            | "work"
            | "works"
            | "find"
            | "found"
            | "show"
            | "call"
            | "called"
            | "calling"
            | "get"
            | "set"
            | "add"
            | "all"
            | "any"
            | "how"
            | "what"
            | "when"
            | "where"
            | "which"
            | "who"
            | "why"
            | "not"
            | "but"
            | "are"
            | "was"
            | "were"
            | "has"
            | "had"
            | "its"
            | "can"
            | "did"
            | "may"
            | "also"
            | "into"
            | "than"
            | "then"
            | "them"
            | "each"
            | "other"
            | "some"
            | "such"
            | "only"
            | "same"
            | "about"
            | "after"
            | "before"
            | "between"
            | "through"
            | "during"
            | "without"
            | "again"
            | "further"
            | "once"
            | "here"
            | "there"
            | "both"
            | "just"
            | "more"
            | "most"
            | "very"
            | "being"
            | "having"
            | "doing"
            | "system"
            | "need"
            | "needs"
            | "want"
            | "wants"
            | "like"
            | "look"
            | "change"
            | "changes"
            | "changed"
            | "changing"
            | "layer"
            | "handle"
            | "handles"
            | "handling"
            | "incoming"
            | "outgoing"
            | "data"
            | "flow"
            | "flows"
            | "level"
            | "levels"
            | "request"
            | "requests"
            | "response"
            | "responses"
            | "implement"
            | "implements"
            | "implementation"
            | "interface"
            | "interfaces"
            | "class"
            | "classes"
            | "method"
            | "methods"
            | "trigger"
            | "triggers"
            | "affected"
            | "affect"
            | "affects"
            | "else"
            | "code"
            | "return"
            | "returns"
            | "take"
            | "takes"
            | "check"
            | "create"
            | "read"
            | "write"
            | "start"
            | "stop"
            | "run"
            | "runs"
            | "running"
            | "fix"
            | "update"
            | "task"
            | "bug"
            | "feature"
            | "file"
            | "files"
            | "function"
            | "type"
            | "failing"
            | "failed"
            | "silently"
            | "decide"
            | "decides"
            | "happen"
            | "happens"
            | "implemented"
            | "robust"
            | "mode"
    )
}

fn looks_identifier_like(token: &str) -> bool {
    // Be conservative here: every identifier-like token is treated as an
    // exact-symbol candidate and can trigger strong colocation boosts.  Plain
    // lower-case prose words such as "child", "timeout", "sync", or "mode"
    // are better handled by lexical/content search; otherwise broad natural
    // language implementation queries pull in unrelated symbols that merely
    // share generic names.  Real symbol hints usually contain punctuation,
    // underscores, CamelCase, or all-caps constants.
    token.contains('_')
        || token.contains('.')
        || token.chars().any(|c| c.is_uppercase())
        || token
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

fn stem_variants(term: &str) -> Vec<String> {
    let t = term.to_lowercase();
    let mut out: HashSet<String> = HashSet::new();

    if t.ends_with("ing") && t.len() > 5 {
        let base = &t[..t.len() - 3];
        out.insert(base.to_string());
        out.insert(format!("{}e", base));
        if has_doubled_final(base) {
            out.insert(base[..base.len() - 1].to_string());
        }
    }
    if (t.ends_with("tion") || t.ends_with("sion")) && t.len() > 5 {
        out.insert(t[..t.len() - 3].to_string());
    }
    if t.ends_with("ment") && t.len() > 6 {
        out.insert(t[..t.len() - 4].to_string());
    }
    if t.ends_with("ies") && t.len() > 4 {
        out.insert(format!("{}y", &t[..t.len() - 3]));
    } else if t.ends_with("es") && t.len() > 4 {
        out.insert(t[..t.len() - 2].to_string());
    } else if t.ends_with('s') && !t.ends_with("ss") && t.len() > 4 {
        out.insert(t[..t.len() - 1].to_string());
    }
    if t.ends_with("ed") && !t.ends_with("eed") && t.len() > 4 {
        out.insert(t[..t.len() - 1].to_string());
        out.insert(t[..t.len() - 2].to_string());
        if t.ends_with("ied") && t.len() > 5 {
            out.insert(format!("{}y", &t[..t.len() - 3]));
        }
    }
    if t.ends_with("er") && t.len() > 4 {
        let base = &t[..t.len() - 2];
        out.insert(base.to_string());
        out.insert(format!("{}e", base));
        if has_doubled_final(base) {
            out.insert(base[..base.len() - 1].to_string());
        }
    }

    let mut v: Vec<String> = out
        .into_iter()
        .filter(|variant| variant.len() >= 3 && variant != &t && !common_word(variant))
        .collect();
    v.sort();
    v
}

fn has_doubled_final(s: &str) -> bool {
    let mut chars = s.chars().rev();
    matches!((chars.next(), chars.next()), (Some(a), Some(b)) if a == b)
}

fn title_case(term: &str) -> String {
    let mut chars = term.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

fn lexical_score(node: &Node, term: &str) -> f64 {
    let name = node.name.to_lowercase();
    let qn = node.qualified_name.to_lowercase();
    let term = term.to_lowercase();
    let mut score = kind_weight(node.kind);
    if name == term {
        score += 45.0;
    } else if name.starts_with(&term) {
        score += 30.0;
    } else if name.contains(&term) {
        score += 18.0;
    } else if qn.contains(&term) {
        score += 10.0;
    }
    score
}

fn path_relevance(path: &str, terms: &[String]) -> f64 {
    let path = path.to_lowercase();
    terms.iter().filter(|t| path.contains(t.as_str())).count() as f64 * 8.0
}

fn apply_colocation_boost(nodes: &mut [ScoredNode], symbols: &[String]) {
    if symbols.len() < 2 {
        return;
    }
    let mut files: HashMap<String, HashSet<String>> = HashMap::new();
    for node in nodes.iter() {
        for symbol in symbols {
            if node.node.name.eq_ignore_ascii_case(symbol)
                || node
                    .node
                    .qualified_name
                    .to_lowercase()
                    .contains(&symbol.to_lowercase())
            {
                files
                    .entry(node.node.file_path.clone())
                    .or_default()
                    .insert(symbol.to_lowercase());
            }
        }
    }
    for node in nodes.iter_mut() {
        let count = files
            .get(&node.node.file_path)
            .map(|s| s.len())
            .unwrap_or(0);
        if count > 1 {
            node.score += ((count - 1) * 20) as f64;
            node.reason.push_str(" + colocated");
        }
    }
}

fn apply_multi_term_boost(nodes: &mut [ScoredNode], terms: &[String], exact_ids: &HashSet<String>) {
    let groups = term_groups(terms);
    if groups.len() < 2 {
        return;
    }
    for node in nodes.iter_mut() {
        let hits = matched_group_indexes(&node.node, &groups).len();
        if hits >= 2 {
            node.score *= 1.0 + (hits as f64 * 0.35);
            node.reason.push_str(" + multi-term");
        } else if hits <= 1 && !exact_ids.contains(&node.node.id) {
            node.score *= 0.62;
            node.reason.push_str(" + single-term dampened");
        }
    }
}

fn term_groups(terms: &[String]) -> Vec<Vec<String>> {
    let mut sorted: Vec<String> = terms.iter().cloned().collect();
    sorted.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    let mut assigned: HashSet<String> = HashSet::new();
    let mut groups = Vec::new();
    for term in &sorted {
        if assigned.contains(term) {
            continue;
        }
        let mut group = vec![term.clone()];
        assigned.insert(term.clone());
        for other in &sorted {
            if assigned.contains(other) {
                continue;
            }
            if term.contains(other) || other.contains(term) {
                group.push(other.clone());
                assigned.insert(other.clone());
            }
        }
        groups.push(group);
    }
    groups
}

fn matched_group_indexes(node: &Node, groups: &[Vec<String>]) -> HashSet<usize> {
    let name_terms = split_identifier_terms(&node.name).join(" ");
    let haystack = format!(
        "{} {} {} {} {}",
        node.name,
        node.qualified_name,
        name_terms,
        node.file_path,
        node.signature.as_deref().unwrap_or("")
    )
    .to_lowercase();
    let dirs: Vec<String> = Path::new(&node.file_path)
        .parent()
        .map(|p| {
            p.components()
                .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
                .collect()
        })
        .unwrap_or_default();
    let mut hits = HashSet::new();
    for (idx, group) in groups.iter().enumerate() {
        if group
            .iter()
            .any(|term| haystack.contains(term.as_str()) || dirs.iter().any(|seg| seg == term))
        {
            hits.insert(idx);
        }
    }
    hits
}

fn high_value_kind(kind: NodeKind) -> bool {
    !matches!(
        kind,
        NodeKind::Import | NodeKind::Export | NodeKind::Parameter
    )
}

fn definition_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Class
            | NodeKind::Struct
            | NodeKind::Interface
            | NodeKind::Trait
            | NodeKind::Protocol
            | NodeKind::Enum
            | NodeKind::TypeAlias
            | NodeKind::Component
            | NodeKind::Module
            | NodeKind::Namespace
    )
}

fn implementation_kind_bias(kind: NodeKind) -> f64 {
    match kind {
        NodeKind::Function | NodeKind::Method => 18.0,
        NodeKind::Class | NodeKind::Struct | NodeKind::Interface | NodeKind::Trait => 8.0,
        NodeKind::Variable | NodeKind::Field | NodeKind::Property | NodeKind::Parameter => -10.0,
        _ => 0.0,
    }
}

fn kind_weight(kind: NodeKind) -> f64 {
    match kind {
        NodeKind::Function | NodeKind::Method => 18.0,
        NodeKind::Class
        | NodeKind::Struct
        | NodeKind::Interface
        | NodeKind::Trait
        | NodeKind::Protocol => 15.0,
        NodeKind::Component => 14.0,
        NodeKind::Enum | NodeKind::TypeAlias => 12.0,
        NodeKind::Module | NodeKind::Namespace => 8.0,
        NodeKind::Variable | NodeKind::Constant | NodeKind::Property | NodeKind::Field => 5.0,
        NodeKind::File => 3.0,
        _ => 0.0,
    }
}

fn is_test_file(path: &str) -> bool {
    let p = path.to_lowercase();
    let lower_name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
        .to_lowercase();
    p.contains("/test/")
        || p.contains("/tests/")
        || p.contains("/__tests__/")
        || p.contains("/spec/")
        || p.contains("/specs/")
        || p.contains("/testlib/")
        || p.contains("/testing/")
        || lower_name.starts_with("test_")
        || lower_name.starts_with("test.")
        || lower_name.ends_with("_test.go")
        || lower_name.ends_with(".test.ts")
        || lower_name.ends_with(".test.js")
        || lower_name.ends_with(".spec.ts")
        || lower_name.ends_with(".spec.js")
        || regex::Regex::new(r"[._-](test|tests|spec|specs)\.[a-z0-9]+$")
            .unwrap()
            .is_match(&lower_name)
}

fn is_non_prod_file(path: &str) -> bool {
    let p = path.to_lowercase();
    is_test_file(path)
        || p.contains("/example/")
        || p.contains("/examples/")
        || p.contains("/sample/")
        || p.contains("/samples/")
        || p.contains("/fixture/")
        || p.contains("/fixtures/")
        || p.contains("/mock/")
        || p.contains("/mocks/")
}

fn query_mentions_tests(query: &str) -> bool {
    let q = query.to_lowercase();
    q.contains("test") || q.contains("spec")
}

fn build_call_paths(
    nodes: &[ScoredNode],
    edges: &[Edge],
    root_ids: &HashSet<String>,
) -> Vec<Vec<String>> {
    let id_to_name: HashMap<String, String> = nodes
        .iter()
        .map(|n| (n.node.id.clone(), n.node.name.clone()))
        .collect();
    let name_to_id: HashMap<String, String> = nodes
        .iter()
        .map(|n| (n.node.name.clone(), n.node.id.clone()))
        .collect();
    let kept: HashSet<String> = id_to_name.keys().cloned().collect();
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();

    for edge in edges.iter().filter(|e| e.kind == EdgeKind::Calls) {
        let source = normalize_endpoint(&edge.source, &kept, &name_to_id);
        let target = normalize_endpoint(&edge.target, &kept, &name_to_id);
        if let (Some(source), Some(target)) = (source, target) {
            adj.entry(source).or_default().push(target);
        }
    }

    let mut chains: Vec<Vec<String>> = Vec::new();
    let starts: Vec<String> = root_ids
        .iter()
        .filter(|id| adj.contains_key(*id))
        .take(5)
        .cloned()
        .collect();
    for start in starts {
        let mut seen = HashSet::new();
        seen.insert(start.clone());
        dfs_call_paths(&start, &adj, &mut seen, vec![start.clone()], &mut chains);
    }

    chains.retain(|c| c.len() >= 3 && c.iter().filter(|id| root_ids.contains(*id)).count() >= 2);
    chains.sort_by(|a, b| b.len().cmp(&a.len()));
    chains.truncate(3);
    let mut seen_paths = HashSet::new();
    chains
        .into_iter()
        .map(|chain| {
            chain
                .into_iter()
                .filter_map(|id| id_to_name.get(&id).cloned())
                .collect::<Vec<_>>()
        })
        .filter(|chain: &Vec<String>| chain.len() >= 3)
        .filter(|chain| seen_paths.insert(chain.join(" -> ")))
        .collect()
}

fn normalize_endpoint(
    endpoint: &str,
    kept: &HashSet<String>,
    name_to_id: &HashMap<String, String>,
) -> Option<String> {
    if kept.contains(endpoint) {
        Some(endpoint.to_string())
    } else {
        name_to_id.get(endpoint).cloned()
    }
}

fn dfs_call_paths(
    current: &str,
    adj: &HashMap<String, Vec<String>>,
    seen: &mut HashSet<String>,
    path: Vec<String>,
    chains: &mut Vec<Vec<String>>,
) {
    if path.len() >= 6 {
        chains.push(path);
        return;
    }
    let Some(next) = adj.get(current) else {
        chains.push(path);
        return;
    };
    for target in next.iter().take(4) {
        if seen.insert(target.clone()) {
            let mut p = path.clone();
            p.push(target.clone());
            dfs_call_paths(target, adj, seen, p, chains);
            seen.remove(target);
        }
    }
}

fn format_node_line(scored: &ScoredNode, include_reason: bool) -> String {
    let node = &scored.node;
    let mut line = format!(
        "- **{}** ({}) - {}",
        node.name,
        node.kind.as_str(),
        format_file_and_line(&node.file_path, node.start_line)
    );
    if include_reason {
        line.push_str(&format!(" _score {:.1}, {}_", scored.score, scored.reason));
    }
    line.push('\n');
    if let Some(sig) = &node.signature {
        line.push_str(&format!("  `{}`\n", sig.replace('\n', " ")));
    }
    line
}

fn limit_code_block(source: String, max_chars: usize) -> (String, bool) {
    if source.len() <= max_chars {
        return (source, false);
    }
    let mut out = String::new();
    for line in source.lines() {
        if out.len() + line.len() + 1 > max_chars {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("\n// ... truncated ...");
    (out, true)
}

fn truncate_output(mut output: String, max_chars: usize) -> String {
    if output.len() <= max_chars {
        return output;
    }
    output.truncate(max_chars);
    output.push_str("\n\n... output truncated to context budget ...\n");
    output
}

fn infer_block_end(lines: &[&str], start_line: u32, language: Language) -> u32 {
    let start_idx = start_line.saturating_sub(1) as usize;
    if start_idx >= lines.len() {
        return start_line;
    }

    if language == Language::Python {
        let base_indent = indentation(lines[start_idx]);
        let mut last = start_idx;
        for (idx, line) in lines.iter().enumerate().skip(start_idx + 1) {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                last = idx;
                continue;
            }
            if indentation(line) <= base_indent {
                break;
            }
            last = idx;
        }
        return (last + 1) as u32;
    }

    let mut brace_depth: i32 = 0;
    let mut saw_open = false;
    for (idx, line) in lines.iter().enumerate().skip(start_idx) {
        for ch in line.chars() {
            match ch {
                '{' => {
                    brace_depth += 1;
                    saw_open = true;
                }
                '}' => brace_depth -= 1,
                _ => {}
            }
        }
        if saw_open && brace_depth <= 0 {
            return (idx + 1) as u32;
        }
        if !saw_open && idx > start_idx && line.trim_end().ends_with(';') {
            return (idx + 1) as u32;
        }
    }

    start_line.min(lines.len() as u32)
}

fn indentation(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).count()
}

fn format_file_and_line(file_path: &str, line: u32) -> String {
    format!("{}:{}", file_path, line)
}
