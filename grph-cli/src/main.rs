use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "grph",
    version,
    about = "Semantic code intelligence — index and query your codebase"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize Grph in a project
    Init {
        /// Also run initial indexing
        #[arg(short = 'i', long)]
        index: bool,
        /// Number of parallel parsing workers
        #[arg(short = 'j', long)]
        jobs: Option<usize>,
        /// Project path
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Build the knowledge graph index
    Index {
        /// Project path
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Force re-indexing
        #[arg(long)]
        force: bool,
        /// Quiet mode
        #[arg(long)]
        quiet: bool,
        /// Number of parallel parsing workers
        #[arg(short = 'j', long)]
        jobs: Option<usize>,
    },

    /// Incrementally sync changed files
    Sync {
        /// Project path
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Sync only this file
        #[arg(long)]
        file: Option<PathBuf>,
    },

    /// Show index statistics
    Status {
        /// Project path
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Generate Universal Ctags-compatible tags file from the index
    Ctags {
        /// Project path
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Output path for tags file
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Search symbols by name
    Query {
        /// Search query
        query: String,
        /// Filter by kind
        #[arg(long)]
        kind: Option<String>,
        /// Limit results
        #[arg(long)]
        limit: Option<u32>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Project path
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Show file structure from the index
    Files {
        /// Project path
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long)]
        format: Option<String>,
        /// Filter by pattern
        #[arg(long)]
        filter: Option<String>,
        /// Max depth
        #[arg(long)]
        max_depth: Option<u32>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Build AI context for a task
    Context {
        /// Task description
        task: String,
        /// Output format
        #[arg(long)]
        format: Option<String>,
        /// Max nodes
        #[arg(long)]
        max_nodes: Option<u32>,
        /// Project path
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Find what calls a symbol
    Callers {
        /// Symbol name
        symbol: String,
        /// Limit results
        #[arg(long)]
        limit: Option<u32>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Project path
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Find what a symbol calls
    Callees {
        /// Symbol name
        symbol: String,
        /// Limit results
        #[arg(long)]
        limit: Option<u32>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Project path
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Analyze impact radius of changing a symbol
    Impact {
        /// Symbol name
        symbol: String,
        /// Depth
        #[arg(long)]
        depth: Option<u32>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Project path
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Trace call path between two symbols
    Trace {
        /// Starting symbol
        from: String,
        /// Target symbol
        to: String,
        /// Project path
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Explore multiple related symbols
    Explore {
        /// Search query
        query: String,
        /// Max files
        #[arg(long)]
        max_files: Option<u32>,
        /// Project path
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Start an integration server
    Serve {
        /// Run as MCP server
        #[arg(long)]
        mcp: bool,
        /// Run as LSP server
        #[arg(long)]
        lsp: bool,
        /// Project path
        #[arg(short = 'p', long, default_value = ".")]
        path: PathBuf,
    },

    /// Remove Grph from a project
    Uninit {
        /// Project path
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Force removal
        #[arg(long)]
        force: bool,
    },
}

fn main() -> grph_core::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { index, jobs, path } => {
            let path = resolve_path(&path);
            let grph = grph_core::Grph::init(&path)?;

            if index {
                println!("Starting initial index...");
                let mut grph = grph; // make mutable for indexing
                let result = grph.index_with_jobs(index_jobs(jobs), |progress| {
                    if progress.phase == "complete" {
                        eprintln!(); // final newline
                    } else {
                        use std::io::{self, Write};
                        let _ = write!(
                            io::stderr(),
                            "\r\x1b[K[{}/{}] {}: {}",
                            progress.current,
                            progress.total,
                            progress.phase,
                            progress.current_file.as_deref().unwrap_or("")
                        );
                        let _ = io::stderr().flush();
                    }
                })?;
                println!(
                    "Indexed {} files, {} nodes, {} edges",
                    result.files_indexed, result.nodes_created, result.edges_created
                );
            } else {
                println!("Initialized. Run `grph index` to build the index.");
            }
        }
        Commands::Index {
            path,
            force: _,
            quiet,
            jobs,
        } => {
            let path = resolve_path(&path);
            let mut grph = grph_core::Grph::open(&path)?;

            if !quiet {
                println!("Indexing...");
            }

            let result = grph.index_with_jobs(index_jobs(jobs), |progress| {
                if !quiet {
                    if progress.phase == "complete" {
                        eprintln!(); // final newline
                    } else {
                        use std::io::{self, Write};
                        let _ = write!(
                            io::stderr(),
                            "\r\x1b[K[{}/{}] {}: {}",
                            progress.current,
                            progress.total,
                            progress.phase,
                            progress.current_file.as_deref().unwrap_or("")
                        );
                        let _ = io::stderr().flush();
                    }
                }
            })?;

            println!(
                "Indexed {} files, {} nodes, {} edges",
                result.files_indexed, result.nodes_created, result.edges_created
            );
        }
        Commands::Sync { path, file } => {
            let path = resolve_path(&path);
            let mut grph = grph_core::Grph::open(&path)?;
            let result = if let Some(file) = file {
                let file = if file.is_absolute() {
                    file
                } else {
                    path.join(file)
                };
                grph.sync_file(&file)?
            } else {
                grph.sync()?
            };
            println!(
                "Synced: {} changed, {} added, {} deleted",
                result.files_changed, result.files_added, result.files_deleted
            );
        }
        Commands::Status { path } => {
            let path = resolve_path(&path);
            let grph = grph_core::Grph::open(&path)?;
            let stats = grph.stats()?;
            println!("Files: {}", stats.total_files);
            println!("Nodes: {}", stats.total_nodes);
            println!("Edges: {}", stats.total_edges);
        }
        Commands::Ctags { path, output } => {
            let path = resolve_path(&path);
            let grph = grph_core::Grph::open(&path)?;
            let output = output
                .map(|p| if p.is_absolute() { p } else { path.join(p) })
                .unwrap_or_else(|| path.join("tags"));
            let count = grph.generate_ctags(&output)?;
            println!("Wrote {} tags to {}", count, output.display());
        }
        Commands::Query {
            query,
            kind,
            limit,
            json,
            path,
        } => {
            let path = resolve_path(&path);
            let grph = grph_core::Grph::open(&path)?;
            let kind_enum = kind.as_ref().and_then(|k| grph_core::NodeKind::from_str(k));
            let nodes = grph.search(&query, kind_enum, limit.unwrap_or(10))?;

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&nodes).map_err(grph_core::GrphError::Json)?
                );
            } else {
                println!("Search Results for \"{}\":\n", query);
                for node in &nodes {
                    println!("{:<11} {}", node.kind.as_str(), node.name);
                    println!("  {}:{}", node.file_path, node.start_line);
                    if let Some(signature) = display_signature(node) {
                        println!("  {}", signature);
                    }
                    println!();
                }
            }
        }
        Commands::Files { path, .. } => {
            let path = resolve_path(&path);
            let grph = grph_core::Grph::open(&path)?;
            let files = grph.db().list_files(None)?;
            for file in &files {
                println!("{}", file.path);
            }
        }
        Commands::Context {
            task,
            format: _,
            max_nodes,
            path,
        } => {
            let path = resolve_path(&path);
            let grph = grph_core::Grph::open(&path)?;
            let context = grph.build_context(&task, max_nodes.unwrap_or(20), true)?;
            println!("{}", context);
        }
        Commands::Callers {
            symbol,
            limit,
            json: _,
            path,
        } => {
            let path = resolve_path(&path);
            let grph = grph_core::Grph::open(&path)?;
            let nodes = grph.search(&symbol, None, 10)?;
            if nodes.is_empty() {
                eprintln!("Symbol not found: {}", symbol);
                std::process::exit(1);
            }
            let limit = limit.unwrap_or(DEFAULT_GRAPH_LIMIT);
            let traverser = grph.traverser();
            let callers = traverser.callers(&nodes[0].id, limit)?;
            println!("Callers of \"{}\" ({}):\n", symbol, callers.len());
            for (node, edge) in &callers {
                println!(
                    "{:<10} {}\n  {}:{}",
                    node.kind.as_str(),
                    node.name,
                    node.file_path,
                    edge.line.unwrap_or(node.start_line),
                );
            }
            if callers.len() >= limit as usize {
                eprintln!("Showing first {limit} callers. Use --limit to show more.");
            }
        }
        Commands::Callees {
            symbol,
            limit,
            json: _,
            path,
        } => {
            let path = resolve_path(&path);
            let grph = grph_core::Grph::open(&path)?;
            let nodes = grph.search(&symbol, None, 10)?;
            if nodes.is_empty() {
                eprintln!("Symbol not found: {}", symbol);
                std::process::exit(1);
            }
            let limit = limit.unwrap_or(DEFAULT_GRAPH_LIMIT);
            let traverser = grph.traverser();
            let callees = traverser.callees(&nodes[0].id, limit)?;
            println!("Callees of \"{}\" ({}):\n", symbol, callees.len());
            for (node, _edge) in &callees {
                println!(
                    "{:<10} {}\n  {}:{}",
                    node.kind.as_str(),
                    node.name,
                    node.file_path,
                    node.start_line,
                );
            }
            if callees.len() >= limit as usize {
                eprintln!("Showing first {limit} callees. Use --limit to show more.");
            }
        }
        Commands::Impact {
            symbol,
            depth,
            json: _,
            path,
        } => {
            let path = resolve_path(&path);
            let grph = grph_core::Grph::open(&path)?;
            let nodes = grph.search(&symbol, None, 10)?;
            if nodes.is_empty() {
                eprintln!("Symbol not found: {}", symbol);
                std::process::exit(1);
            }
            let traverser = grph.traverser();
            let impact = traverser.impact_radius(&nodes[0].id, depth.unwrap_or(2))?;
            println!("Impact radius of {} (depth {}):", symbol, impact.depth);
            println!("  Nodes: {}", impact.nodes.len());
            println!("  Edges: {}", impact.edges.len());
        }
        Commands::Trace { from, to, path } => {
            let path = resolve_path(&path);
            let grph = grph_core::Grph::open(&path)?;
            let from_nodes = grph.search(&from, None, 5)?;
            let to_nodes = grph.search(&to, None, 5)?;
            if from_nodes.is_empty() || to_nodes.is_empty() {
                eprintln!("Symbol not found: from={}, to={}", from, to);
                std::process::exit(1);
            }
            let traverser = grph.traverser();
            match traverser.shortest_path(&from_nodes[0].id, &to_nodes[0].id)? {
                Some(path_hops) => {
                    println!("Path from {} to {}:", from, to);
                    for hop in path_hops {
                        let label = grph
                            .db()
                            .get_node_by_id(&hop.node_id)?
                            .map(|node| node.name)
                            .unwrap_or(hop.node_id);
                        println!("  → {}", label);
                    }
                }
                None => {
                    println!("No path found from {} to {}", from, to);
                }
            }
        }
        Commands::Explore {
            query,
            max_files,
            path,
        } => {
            let path = resolve_path(&path);
            let grph = grph_core::Grph::open(&path)?;
            let nodes = grph.search(&query, None, (max_files.unwrap_or(12) * 5) as u32)?;
            let mut by_file: std::collections::HashMap<String, Vec<&grph_core::Node>> =
                std::collections::HashMap::new();
            for node in &nodes {
                by_file
                    .entry(node.file_path.clone())
                    .or_default()
                    .push(node);
            }
            for (file, file_nodes) in by_file.iter().take(max_files.unwrap_or(12) as usize) {
                println!("### {}", file);
                for node in file_nodes.iter().take(20) {
                    println!("  {} (line {})", node.name, node.start_line);
                }
                println!();
                for node in file_nodes.iter().take(3) {
                    if let Some((start_line, snippet)) = source_snippet(&path, node) {
                        println!("#### {} ({}:{})", node.name, node.file_path, start_line);
                        println!("```{}", node.language.as_str());
                        println!("{}", snippet);
                        println!(
                            "```
"
                        );
                    }
                }
            }
        }
        Commands::Serve { mcp, lsp, path } => {
            if mcp {
                let path = resolve_path(&path);
                eprintln!("Starting MCP server at {:?}...", path);
                grph_mcp::transport::serve_stdio(path).map_err(grph_core::GrphError::Io)?;
            } else if lsp {
                let path = resolve_path(&path);
                eprintln!("Starting LSP server at {:?}...", path);
                grph_lsp::run_lsp_server(path).map_err(grph_core::GrphError::Io)?;
            } else {
                eprintln!("Use `grph serve --mcp` or `grph serve --lsp`");
                std::process::exit(1);
            }
        }
        Commands::Uninit { path, force: _ } => {
            let path = resolve_path(&path).join(".grph");
            if path.exists() {
                std::fs::remove_dir_all(&path).map_err(grph_core::GrphError::Io)?;
                println!("Removed .grph directory");
            } else {
                println!("No .grph directory found");
            }
        }
    }

    Ok(())
}

fn resolve_path(path: &PathBuf) -> PathBuf {
    if path.is_absolute() {
        path.clone()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    }
}

fn index_jobs(jobs: Option<usize>) -> usize {
    jobs.filter(|jobs| *jobs > 0)
        .unwrap_or_else(grph_core::extraction::orchestrator::default_index_jobs)
}

const DEFAULT_GRAPH_LIMIT: u32 = 1000;

fn display_signature(node: &grph_core::Node) -> Option<String> {
    let signature = node.signature.as_ref()?.trim();
    if signature.is_empty() {
        return None;
    }

    let signature = trim_trailing_body_marker(signature);
    if matches!(
        node.kind,
        grph_core::NodeKind::Function | grph_core::NodeKind::Method
    ) && node.language == grph_core::Language::Rust
    {
        if let Some(compact) = rust_signature_without_decl_prefix(&signature, &node.name) {
            return Some(compact);
        }
    }

    Some(signature)
}

fn trim_trailing_body_marker(signature: &str) -> String {
    signature
        .trim()
        .trim_end_matches('{')
        .trim_end()
        .to_string()
}

fn rust_signature_without_decl_prefix(signature: &str, name: &str) -> Option<String> {
    let fn_pos = signature.find("fn ")?;
    let after_fn = signature[fn_pos + 3..].trim_start();
    let after_name = after_fn.strip_prefix(name)?.trim_start();
    if after_name.starts_with('(') {
        Some(after_name.to_string())
    } else {
        None
    }
}

fn source_snippet(project_root: &std::path::Path, node: &grph_core::Node) -> Option<(u32, String)> {
    let source_path = if std::path::Path::new(&node.file_path).is_absolute() {
        std::path::PathBuf::from(&node.file_path)
    } else {
        project_root.join(&node.file_path)
    };
    let content = std::fs::read_to_string(source_path).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return None;
    }

    let start = node.start_line.max(1);
    let end = infer_snippet_end(&lines, node.start_line, node.language).min(lines.len() as u32);

    Some((start, lines[(start - 1) as usize..end as usize].join("\n")))
}

fn infer_snippet_end(lines: &[&str], start_line: u32, language: grph_core::Language) -> u32 {
    let start_idx = start_line.saturating_sub(1) as usize;
    if start_idx >= lines.len() {
        return start_line;
    }

    if language == grph_core::Language::Python {
        let base_indent = line_indent(lines[start_idx]);
        let mut last = start_idx;
        for (idx, line) in lines.iter().enumerate().skip(start_idx + 1) {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                last = idx;
                continue;
            }
            if line_indent(line) <= base_indent {
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

fn line_indent(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).count()
}
