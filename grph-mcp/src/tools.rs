use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

pub fn text_response(text: impl Into<String>) -> Value {
    json!({"content": [{"type": "text", "text": text.into()}]})
}

fn json_response(value: Value) -> Value {
    text_response(serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()))
}

pub fn mcp_error(message: impl Into<String>) -> Value {
    json!({
        "isError": true,
        "content": [{"type": "text", "text": message.into()}]
    })
}

fn arg_str<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

fn arg_u32(args: &Map<String, Value>, key: &str, default: u32) -> u32 {
    args.get(key)
        .and_then(|v| v.as_u64())
        .map(|v| v.min(u32::MAX as u64) as u32)
        .unwrap_or(default)
}

fn arg_bool(args: &Map<String, Value>, key: &str) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn open(project_root: &PathBuf) -> grph_core::Result<grph_core::Grph> {
    if !project_root.join(".grph").join("grph.db").exists() {
        return Err(grph_core::GrphError::NotInitialized);
    }
    grph_core::Grph::open(project_root)
}

fn search_first(
    grph: &grph_core::Grph,
    symbol: &str,
) -> grph_core::Result<Option<grph_core::Node>> {
    if let Some(node) = grph.db().get_node_by_name_any(symbol)? {
        return Ok(Some(node));
    }
    Ok(grph.search(symbol, None, 10)?.into_iter().next())
}

pub fn handle_search(args: &Map<String, Value>, project_root: &PathBuf) -> Value {
    let query = match arg_str(args, "query") {
        Some(query) => query,
        None => return mcp_error("Missing required argument: query"),
    };
    let limit = arg_u32(args, "limit", 20);
    let kind = arg_str(args, "kind").and_then(grph_core::NodeKind::from_str);

    match open(project_root).and_then(|grph| grph.search(query, kind, limit)) {
        Ok(nodes) if arg_bool(args, "json") => json_response(json!(nodes)),
        Ok(nodes) => {
            let text = nodes
                .iter()
                .map(|node| {
                    format!(
                        "{}:{} {} ({})",
                        node.file_path,
                        node.start_line,
                        node.name,
                        node.kind.as_str()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            text_response(if text.is_empty() {
                "No symbols found.".to_string()
            } else {
                text
            })
        }
        Err(e) => mcp_error(format!("Search failed: {}", e)),
    }
}

pub fn handle_context(args: &Map<String, Value>, project_root: &PathBuf) -> Value {
    let task = match arg_str(args, "task") {
        Some(task) => task,
        None => return mcp_error("Missing required argument: task"),
    };
    let max_nodes = arg_u32(args, "max_nodes", 20);

    match open(project_root).and_then(|grph| grph.build_context(task, max_nodes, true)) {
        Ok(context) => text_response(context),
        Err(e) => mcp_error(format!("Context build failed: {}", e)),
    }
}

pub fn handle_callers(args: &Map<String, Value>, project_root: &PathBuf) -> Value {
    let symbol = match arg_str(args, "symbol") {
        Some(symbol) => symbol,
        None => return mcp_error("Missing required argument: symbol"),
    };
    let limit = arg_u32(args, "limit", 20);

    match open(project_root).and_then(|grph| {
        let node = search_first(&grph, symbol)?;
        let Some(node) = node else {
            return Ok(Vec::new());
        };
        grph.traverser().callers(&node.id, limit)
    }) {
        Ok(callers) => text_response(if callers.is_empty() {
            format!("No callers found for {}.", symbol)
        } else {
            callers
                .iter()
                .map(|(node, edge)| format!("{} → {} ({})", node.name, symbol, edge.kind.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        }),
        Err(e) => mcp_error(format!("Callers lookup failed: {}", e)),
    }
}

pub fn handle_uncalled(args: &Map<String, Value>, project_root: &PathBuf) -> Value {
    let limit = arg_u32(args, "limit", 20);

    match open(project_root).and_then(|grph| grph.db().find_uncalled_functions(limit)) {
        Ok(nodes) if arg_bool(args, "json") => json_response(json!(nodes)),
        Ok(nodes) => {
            let text = nodes
                .iter()
                .map(|node| format!("{}:{} {}", node.file_path, node.start_line, node.name))
                .collect::<Vec<_>>()
                .join("\n");
            text_response(if text.is_empty() {
                "No uncalled functions found.".to_string()
            } else {
                text
            })
        }
        Err(e) => mcp_error(format!("Uncalled functions lookup failed: {}", e)),
    }
}

pub fn handle_callees(args: &Map<String, Value>, project_root: &PathBuf) -> Value {
    let symbol = match arg_str(args, "symbol") {
        Some(symbol) => symbol,
        None => return mcp_error("Missing required argument: symbol"),
    };
    let limit = arg_u32(args, "limit", 20);

    match open(project_root).and_then(|grph| {
        let node = search_first(&grph, symbol)?;
        let Some(node) = node else {
            return Ok(Vec::new());
        };
        grph.traverser().callees(&node.id, limit)
    }) {
        Ok(callees) => text_response(if callees.is_empty() {
            format!("No callees found for {}.", symbol)
        } else {
            callees
                .iter()
                .map(|(node, edge)| format!("{} → {} ({})", symbol, node.name, edge.kind.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        }),
        Err(e) => mcp_error(format!("Callees lookup failed: {}", e)),
    }
}

pub fn handle_impact(args: &Map<String, Value>, project_root: &PathBuf) -> Value {
    let symbol = match arg_str(args, "symbol") {
        Some(symbol) => symbol,
        None => return mcp_error("Missing required argument: symbol"),
    };
    let depth = arg_u32(args, "depth", 2);

    match open(project_root).and_then(|grph| {
        let node = search_first(&grph, symbol)?;
        let Some(node) = node else {
            return Err(grph_core::GrphError::SymbolNotFound(format!(
                "Symbol not found: {}",
                symbol
            )));
        };
        grph.traverser().impact_radius(&node.id, depth)
    }) {
        Ok(impact) => text_response(format_impact(symbol, &impact)),
        Err(e) => mcp_error(format!("Impact analysis failed: {}", e)),
    }
}

fn format_impact(symbol: &str, impact: &grph_core::graph::traversal::ImpactResult) -> String {
    let mut out = format!(
        "## Impact: \"{}\" affects {} symbols (depth {})\n\nEdges: {}\n\n",
        symbol,
        impact.nodes.len(),
        impact.depth,
        impact.edges.len()
    );

    let mut by_file = std::collections::BTreeMap::<String, Vec<&grph_core::Node>>::new();
    for node in &impact.nodes {
        by_file
            .entry(node.file_path.clone())
            .or_default()
            .push(node);
    }

    for (file, mut nodes) in by_file {
        nodes.sort_by_key(|node| node.start_line);
        out.push_str(&format!("**{}:**\n", file));
        out.push_str(
            &nodes
                .iter()
                .map(|node| format!("{}:{}", node.name, node.start_line))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push_str("\n\n");
    }

    out.trim_end().to_string()
}

pub fn handle_node(args: &Map<String, Value>, project_root: &PathBuf) -> Value {
    let symbol = match arg_str(args, "symbol") {
        Some(symbol) => symbol,
        None => return mcp_error("Missing required argument: symbol"),
    };
    let include_code = arg_bool(args, "includeCode") || arg_bool(args, "include_code");

    match open(project_root).and_then(|grph| {
        let Some(node) = search_first(&grph, symbol)? else {
            return Ok(None);
        };
        Ok(Some(format_node_detail(&grph, &node, include_code)?))
    }) {
        Ok(Some(text)) => text_response(text),
        Ok(None) => mcp_error(format!("Symbol not found: {}", symbol)),
        Err(e) => mcp_error(format!("Node lookup failed: {}", e)),
    }
}

fn format_node_detail(
    grph: &grph_core::Grph,
    node: &grph_core::Node,
    include_code: bool,
) -> grph_core::Result<String> {
    let mut out = String::new();
    out.push_str(&format!(
        "## {} ({})\n\n{}:{}\n\n",
        node.name,
        node.kind.as_str(),
        node.file_path,
        node.start_line
    ));
    if let Some(signature) = node.signature.as_ref().filter(|s| !s.trim().is_empty()) {
        out.push_str(&format!("`{}`\n\n", signature.replace('\n', " ")));
    }
    if let Some(doc) = node.docstring.as_ref().filter(|s| !s.trim().is_empty()) {
        out.push_str(doc.trim());
        out.push_str("\n\n");
    }

    let callers = grph.traverser().callers(&node.id, 10)?;
    let callees = grph.traverser().callees(&node.id, 10)?;
    if !callers.is_empty() || !callees.is_empty() {
        out.push_str("### Relationships\n\n");
        if !callers.is_empty() {
            out.push_str("**Callers:** ");
            out.push_str(
                &callers
                    .iter()
                    .map(|(n, _)| format!("{}:{}", n.name, n.start_line))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            out.push_str("\n");
        }
        if !callees.is_empty() {
            out.push_str("**Callees:** ");
            out.push_str(
                &callees
                    .iter()
                    .map(|(n, _)| format!("{}:{}", n.name, n.start_line))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            out.push_str("\n");
        }
        out.push_str("\n");
    }

    if include_code {
        if is_container_node(node.kind) {
            let members = grph.db().list_nodes_by_file(&node.file_path)?;
            let mut members: Vec<_> = members
                .into_iter()
                .filter(|m| {
                    m.id != node.id
                        && m.start_line >= node.start_line
                        && m.end_line <= node.end_line.max(node.start_line)
                        && !matches!(
                            m.kind,
                            grph_core::NodeKind::Import | grph_core::NodeKind::File
                        )
                })
                .collect();
            members.sort_by_key(|m| (m.start_line, m.end_line));
            out.push_str("### Structure\n\n");
            if members.is_empty() {
                out.push_str("No indexed members found.\n\n");
            } else {
                for member in members.into_iter().take(80) {
                    let sig = member
                        .signature
                        .as_ref()
                        .map(|s| format!(" — `{}`", s.replace('\n', " ")))
                        .unwrap_or_default();
                    out.push_str(&format!(
                        "- {} ({}) line {}{}\n",
                        member.name,
                        member.kind.as_str(),
                        member.start_line,
                        sig
                    ));
                }
                out.push_str("\n");
            }
        } else if let Some((start, source, truncated)) =
            node_source(grph.project_root(), node, 4_000)
        {
            out.push_str("### Source\n\n");
            out.push_str(&format!(
                "```{}\n{}\n```\n",
                node.language.as_str(),
                number_text_lines(&source, start)
            ));
            if truncated {
                out.push_str("_Source truncated to node budget._\n");
            }
        }
    } else {
        out.push_str(
            "_Pass `includeCode: true` for source; containers return a structure outline._\n",
        );
    }

    Ok(out)
}

fn is_container_node(kind: grph_core::NodeKind) -> bool {
    matches!(
        kind,
        grph_core::NodeKind::Class
            | grph_core::NodeKind::Struct
            | grph_core::NodeKind::Interface
            | grph_core::NodeKind::Trait
            | grph_core::NodeKind::Protocol
            | grph_core::NodeKind::Enum
            | grph_core::NodeKind::Namespace
            | grph_core::NodeKind::Module
            | grph_core::NodeKind::Component
    )
}

fn node_source(
    project_root: &Path,
    node: &grph_core::Node,
    max_chars: usize,
) -> Option<(u32, String, bool)> {
    let path = resolve_source_path(project_root, &node.file_path);
    let content = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let start = node.start_line.max(1);
    let end = node.end_line.max(start).min(lines.len() as u32);
    let mut source = lines[(start - 1) as usize..end as usize].join("\n");
    let truncated = source.len() > max_chars;
    if truncated {
        source.truncate(max_chars);
        source.push_str("\n⋮ trimmed ⋮");
    }
    Some((start, source, truncated))
}

fn number_text_lines(source: &str, start_line: u32) -> String {
    let mut out = String::new();
    for (idx, line) in source.lines().enumerate() {
        out.push_str(&format!("{}\t{}\n", start_line + idx as u32, line));
    }
    out.trim_end().to_string()
}

pub fn handle_status(_args: &Map<String, Value>, project_root: &PathBuf) -> Value {
    match open(project_root).and_then(|grph| grph.stats()) {
        Ok(stats) => text_response(format!(
            "Files: {}\nNodes: {}\nEdges: {}",
            stats.total_files, stats.total_nodes, stats.total_edges
        )),
        Err(e) => mcp_error(format!("Status failed: {}", e)),
    }
}

pub fn handle_files(args: &Map<String, Value>, project_root: &PathBuf) -> Value {
    match open(project_root).and_then(|grph| grph.db().list_files(None)) {
        Ok(files) if arg_bool(args, "json") => json_response(json!(files)),
        Ok(files) => text_response(
            files
                .into_iter()
                .map(|file| file.path)
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        Err(e) => mcp_error(format!("Files lookup failed: {}", e)),
    }
}

pub fn handle_trace(args: &Map<String, Value>, project_root: &PathBuf) -> Value {
    let from = match arg_str(args, "from") {
        Some(from) => from,
        None => return mcp_error("Missing required argument: from"),
    };
    let to = match arg_str(args, "to") {
        Some(to) => to,
        None => return mcp_error("Missing required argument: to"),
    };

    match open(project_root).and_then(|grph| {
        let Some(from_node) = search_first(&grph, from)? else {
            return Err(grph_core::GrphError::SymbolNotFound(format!(
                "Symbol not found: {}",
                from
            )));
        };
        let Some(to_node) = search_first(&grph, to)? else {
            return Err(grph_core::GrphError::SymbolNotFound(format!(
                "Symbol not found: {}",
                to
            )));
        };
        let path = grph.traverser().shortest_path(&from_node.id, &to_node.id)?;
        let labels = path
            .unwrap_or_default()
            .into_iter()
            .map(|hop| {
                grph.db()
                    .get_node_by_id(&hop.node_id)
                    .ok()
                    .flatten()
                    .map(|node| node.name)
                    .unwrap_or(hop.node_id)
            })
            .collect::<Vec<_>>();
        Ok(labels)
    }) {
        Ok(labels) if labels.is_empty() => {
            text_response(format!("No path found from {} to {}", from, to))
        }
        Ok(labels) => text_response(format!(
            "Path from {} to {}:\n  → {}",
            from,
            to,
            labels.join("\n  → ")
        )),
        Err(e) => mcp_error(format!("Trace failed: {}", e)),
    }
}

pub fn handle_explore(args: &Map<String, Value>, project_root: &PathBuf) -> Value {
    let query = match arg_str(args, "query") {
        Some(query) => query,
        None => return mcp_error("Missing required argument: query"),
    };

    match open(project_root)
        .and_then(|grph| format_explore(&grph, query, arg_u32(args, "max_files", 0)))
    {
        Ok(output) => text_response(output),
        Err(e) => mcp_error(format!("Explore failed: {}", e)),
    }
}

fn format_explore(
    grph: &grph_core::Grph,
    query: &str,
    requested_max_files: u32,
) -> grph_core::Result<String> {
    let file_count = grph.db().count_files().unwrap_or(0).min(u32::MAX as u64) as u32;
    let budget = crate::budget::explore_output_budget(file_count);
    let max_files = if requested_max_files == 0 {
        budget.max_files
    } else {
        (requested_max_files as usize).min(budget.max_files)
    };
    let nodes = explore_nodes(grph, query, (max_files as u32).saturating_mul(8).max(20))?;
    if nodes.is_empty() {
        return Ok("No symbols found.".to_string());
    }

    let mut by_file = BTreeMap::<String, Vec<grph_core::Node>>::new();
    for node in nodes {
        by_file
            .entry(node.file_path.clone())
            .or_default()
            .push(node);
    }

    let mut out = String::new();
    out.push_str(&format!("## Explore: `{}`\n\n", query));

    let relationships = collect_relationships(grph, &by_file)?;
    if budget.include_relationships && !relationships.is_empty() {
        out.push_str("### Relationships\n\n");
        for line in relationships.iter().take(30) {
            out.push_str("- ");
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }

    out.push_str("### Source Code\n\n");
    let mut emitted_files = 0usize;
    for (file, mut nodes) in by_file.into_iter() {
        if emitted_files >= max_files || out.len() >= budget.max_chars {
            break;
        }
        nodes.sort_by_key(|n| (n.start_line, n.end_line));
        let path = resolve_source_path(grph.project_root(), &file);
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let lines: Vec<&str> = content.lines().collect();
        if lines.is_empty() {
            continue;
        }
        let symbols = nodes
            .iter()
            .take(budget.max_symbols_in_file_header)
            .map(|n| format!("{}:{}", n.name, n.start_line))
            .collect::<Vec<_>>()
            .join(", ");
        let lang = nodes.first().map(|n| n.language.as_str()).unwrap_or("");
        let clusters = build_clusters(&nodes, lines.len() as u32, 3, budget.gap_threshold);
        let mut body = String::new();
        for (idx, (start, end)) in clusters.into_iter().enumerate() {
            if idx > 0 {
                body.push_str("\n⋮\n");
            }
            let slice = number_source_lines(&lines, start, end);
            if body.len() + slice.len() > budget.max_chars_per_file {
                body.push_str("\n⋮ trimmed to per-file budget ⋮\n");
                break;
            }
            body.push_str(&slice);
        }
        if body.trim().is_empty() {
            continue;
        }

        let section = format!(
            "#### {} — {}\n\n```{}\n{}\n```\n\n",
            file,
            symbols,
            lang,
            body.trim_end()
        );
        if out.len() + section.len() > budget.max_chars {
            out.push_str("\n⋮ explore output trimmed to budget ⋮\n");
            break;
        }
        out.push_str(&section);
        emitted_files += 1;
    }

    if budget.include_completeness_signal {
        out.push_str("Complete source code is included above for the selected clusters.\n");
    }
    if budget.include_budget_note {
        out.push_str(&format!(
            "Explore budget: {} chars, {} files, {} chars/file.\n",
            budget.max_chars, budget.max_files, budget.max_chars_per_file
        ));
    }

    if out.len() > budget.max_chars {
        out.truncate(budget.max_chars);
        out.push_str("\n\n⋮ explore output trimmed to budget ⋮\n");
    }
    Ok(out)
}

fn explore_nodes(
    grph: &grph_core::Grph,
    query: &str,
    limit: u32,
) -> grph_core::Result<Vec<grph_core::Node>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for node in grph.search(query, None, limit)? {
        if seen.insert(node.id.clone()) {
            out.push(node);
        }
    }

    for term in query_terms(query).into_iter().filter(|term| term.len() > 1) {
        for node in grph.search(&term, None, limit)? {
            if seen.insert(node.id.clone()) {
                out.push(node);
                if out.len() >= limit as usize {
                    return Ok(out);
                }
            }
        }
    }

    // Content fallback: if symbol search is sparse, scan indexed file contents
    // and add high-value symbols from files containing query terms.
    if out.len() < (limit as usize).min(8) {
        let terms = query_terms(query);
        let mut files = Vec::<(String, usize)>::new();
        for file in grph.db().list_files(None)? {
            let path = resolve_source_path(grph.project_root(), &file.path);
            let Ok(content) = std::fs::read_to_string(path) else {
                continue;
            };
            let lower = content.to_lowercase();
            let hits = terms
                .iter()
                .map(|t| lower.matches(t).take(5).count())
                .sum::<usize>();
            if hits > 0 {
                files.push((file.path, hits));
            }
        }
        files.sort_by(|a, b| b.1.cmp(&a.1));
        for (file, _) in files.into_iter().take(8) {
            for node in grph.db().list_nodes_by_file(&file)?.into_iter().take(12) {
                if seen.insert(node.id.clone()) {
                    out.push(node);
                    if out.len() >= limit as usize {
                        return Ok(out);
                    }
                }
            }
        }
    }

    Ok(out)
}

fn collect_relationships(
    grph: &grph_core::Grph,
    by_file: &BTreeMap<String, Vec<grph_core::Node>>,
) -> grph_core::Result<Vec<String>> {
    let mut ids = HashSet::new();
    let mut names = HashMap::new();
    for nodes in by_file.values() {
        for node in nodes {
            ids.insert(node.id.clone());
            names.insert(node.id.clone(), node.name.clone());
            names.insert(node.name.clone(), node.name.clone());
        }
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for id in ids.iter() {
        for edge in grph.db().get_edges_for_node(id)? {
            if edge.kind != grph_core::EdgeKind::Calls
                && edge.kind != grph_core::EdgeKind::References
            {
                continue;
            }
            let source_known = ids.contains(&edge.source) || names.contains_key(&edge.source);
            let target_known = ids.contains(&edge.target) || names.contains_key(&edge.target);
            if !(source_known && target_known) {
                continue;
            }
            let src = names
                .get(&edge.source)
                .cloned()
                .unwrap_or(edge.source.clone());
            let tgt = names
                .get(&edge.target)
                .cloned()
                .unwrap_or(edge.target.clone());
            let key = format!("{}>{}:{}", src, tgt, edge.kind.as_str());
            if seen.insert(key) {
                out.push(format!("{} → {} ({})", src, tgt, edge.kind.as_str()));
            }
        }
    }
    out.sort();
    Ok(out)
}

fn query_terms(query: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in query.split(|c: char| !c.is_alphanumeric() && c != '_') {
        let term = raw.trim().to_lowercase();
        if term.len() < 2
            || matches!(
                term.as_str(),
                "the" | "and" | "for" | "with" | "from" | "how" | "what" | "why" | "code" | "work"
            )
        {
            continue;
        }
        if !out.iter().any(|t| t == &term) {
            out.push(term);
        }
    }
    out
}

fn build_clusters(
    nodes: &[grph_core::Node],
    line_count: u32,
    padding: u32,
    gap: u32,
) -> Vec<(u32, u32)> {
    let mut ranges = Vec::<(u32, u32)>::new();
    for node in nodes {
        let start = node.start_line.saturating_sub(padding).max(1);
        let end = (node.end_line.max(node.start_line) + padding).min(line_count);
        if end >= start {
            ranges.push((start, end));
        }
    }
    ranges.sort();
    let mut clusters: Vec<(u32, u32)> = Vec::new();
    for (start, end) in ranges {
        if let Some(last) = clusters.last_mut() {
            if start <= last.1 + gap {
                last.1 = last.1.max(end);
                continue;
            }
        }
        clusters.push((start, end));
    }
    clusters
}

fn number_source_lines(lines: &[&str], start: u32, end: u32) -> String {
    let mut out = String::new();
    for line_no in start..=end {
        if let Some(line) = lines.get((line_no - 1) as usize) {
            out.push_str(&format!("{}\t{}\n", line_no, line));
        }
    }
    out
}

fn resolve_source_path(root: &Path, file_path: &str) -> PathBuf {
    let path = Path::new(file_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

pub fn tool_defs() -> Vec<serde_json::Value> {
    all_tool_defs()
        .into_iter()
        .filter(|tool| {
            tool.get("name")
                .and_then(Value::as_str)
                .map(is_tool_allowed)
                .unwrap_or(false)
        })
        .collect()
}

fn all_tool_defs() -> Vec<serde_json::Value> {
    vec![
        json!({"name":"grph_search","description":"Search symbols by name prefix","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"kind":{"type":"string"},"limit":{"type":"integer"},"json":{"type":"boolean"},"projectPath":{"type":"string"}},"required":["query"]}}),
        json!({"name":"grph_context","description":"PRIMARY TOOL — call this FIRST for code understanding, architecture, feature, bug-context, or 'how does X work' questions. Performs hybrid retrieval + ranked graph expansion and returns bounded entry points, related symbols/files, key code snippets, and call-path hints in one response. Prefer this over chaining search/read/grep; use grph_explore or grph_node only for targeted follow-up source.","inputSchema":{"type":"object","properties":{"task":{"type":"string","description":"Description of the task, bug, feature, or code question to build context for"},"max_nodes":{"type":"integer","description":"Maximum symbols to include in the ranked context graph; default 20"},"projectPath":{"type":"string"}},"required":["task"]}}),
        json!({"name":"grph_callers","description":"Find what calls a symbol","inputSchema":{"type":"object","properties":{"symbol":{"type":"string"},"limit":{"type":"integer"},"projectPath":{"type":"string"}},"required":["symbol"]}}),
        json!({"name":"grph_uncalled","description":"List functions with no callers","inputSchema":{"type":"object","properties":{"limit":{"type":"integer"},"json":{"type":"boolean"},"projectPath":{"type":"string"}}}}),
        json!({"name":"grph_callees","description":"Find what a symbol calls","inputSchema":{"type":"object","properties":{"symbol":{"type":"string"},"limit":{"type":"integer"},"projectPath":{"type":"string"}},"required":["symbol"]}}),
        json!({"name":"grph_impact","description":"Analyze impact radius","inputSchema":{"type":"object","properties":{"symbol":{"type":"string"},"depth":{"type":"integer"},"projectPath":{"type":"string"}},"required":["symbol"]}}),
        json!({"name":"grph_node","description":"Return details for one symbol, with callers/callees. Pass includeCode=true to include source for leaf symbols or a compact structure outline for classes/modules/containers.","inputSchema":{"type":"object","properties":{"symbol":{"type":"string"},"includeCode":{"type":"boolean","description":"Include source/outline; default false to minimize context"},"projectPath":{"type":"string"}},"required":["symbol"]}}),
        json!({"name":"grph_status","description":"Show index statistics","inputSchema":{"type":"object","properties":{"projectPath":{"type":"string"}}}}),
        json!({"name":"grph_files","description":"List indexed files","inputSchema":{"type":"object","properties":{"json":{"type":"boolean"},"projectPath":{"type":"string"}}}}),
        json!({"name":"grph_trace","description":"Trace call path between two symbols","inputSchema":{"type":"object","properties":{"from":{"type":"string"},"to":{"type":"string"},"projectPath":{"type":"string"}},"required":["from","to"]}}),
        json!({"name":"grph_explore","description":"Return verbatim, line-numbered source for several related symbols grouped by file in one bounded call. Use after grph_context when you need more source for listed symbols; prefer this over many grph_node/read calls. Uses adaptive output budgets, source clustering, relationships, and content fallback for query terms.","inputSchema":{"type":"object","properties":{"query":{"type":"string","description":"Specific symbol/file/code terms, not a long natural-language prompt"},"max_files":{"type":"integer","description":"Maximum files to include, capped by project-size budget"},"projectPath":{"type":"string"}},"required":["query"]}}),
    ]
}

pub fn validate_tool_args(
    tool_name: &str,
    args: &Map<String, Value>,
) -> std::result::Result<(), String> {
    let tool = normalize_tool_name(tool_name);
    validate_common_args(args)?;
    match tool.as_str() {
        "grph_search" => {
            require_string(args, "query", 10_000)?;
            optional_string(args, "kind", 128)?;
            optional_u32(args, "limit")?;
            optional_bool(args, "json")?;
        }
        "grph_context" => {
            require_string(args, "task", 50_000)?;
            optional_u32(args, "max_nodes")?;
        }
        "grph_uncalled" => {
            optional_u32(args, "limit")?;
            optional_bool(args, "json")?;
        }
        "grph_callers" | "grph_callees" | "grph_impact" | "grph_node" => {
            require_string(args, "symbol", 10_000)?;
            optional_u32(args, "limit")?;
            optional_u32(args, "depth")?;
            optional_bool(args, "includeCode")?;
            optional_bool(args, "include_code")?;
        }
        "grph_status" => {}
        "grph_files" => {
            optional_bool(args, "json")?;
            optional_string(args, "path", 4096)?;
            optional_string(args, "pattern", 4096)?;
        }
        "grph_trace" => {
            require_string(args, "from", 10_000)?;
            require_string(args, "to", 10_000)?;
        }
        "grph_explore" => {
            require_string(args, "query", 10_000)?;
            optional_u32(args, "max_files")?;
        }
        _ => return Err(format!("Unknown tool: {tool_name}")),
    }
    Ok(())
}

fn validate_common_args(args: &Map<String, Value>) -> std::result::Result<(), String> {
    optional_string(args, "projectPath", 4096)
}

fn require_string(
    args: &Map<String, Value>,
    key: &str,
    max_len: usize,
) -> std::result::Result<(), String> {
    match args.get(key) {
        Some(Value::String(value)) if value.len() <= max_len => Ok(()),
        Some(Value::String(_)) => Err(format!(
            "Argument '{key}' exceeds maximum length of {max_len}"
        )),
        Some(_) => Err(format!("Argument '{key}' must be a string")),
        None => Err(format!("Missing required argument: {key}")),
    }
}

fn optional_string(
    args: &Map<String, Value>,
    key: &str,
    max_len: usize,
) -> std::result::Result<(), String> {
    match args.get(key) {
        Some(Value::String(value)) if value.len() <= max_len => Ok(()),
        Some(Value::String(_)) => Err(format!(
            "Argument '{key}' exceeds maximum length of {max_len}"
        )),
        Some(_) => Err(format!("Argument '{key}' must be a string")),
        None => Ok(()),
    }
}

fn optional_u32(args: &Map<String, Value>, key: &str) -> std::result::Result<(), String> {
    match args.get(key) {
        Some(Value::Number(n))
            if n.as_u64().is_some() && n.as_u64().unwrap() <= u32::MAX as u64 =>
        {
            Ok(())
        }
        Some(_) => Err(format!(
            "Argument '{key}' must be a non-negative integer <= {}",
            u32::MAX
        )),
        None => Ok(()),
    }
}

fn optional_bool(args: &Map<String, Value>, key: &str) -> std::result::Result<(), String> {
    match args.get(key) {
        Some(Value::Bool(_)) => Ok(()),
        Some(_) => Err(format!("Argument '{key}' must be a boolean")),
        None => Ok(()),
    }
}

pub fn is_tool_allowed(tool_name: &str) -> bool {
    let allowlist = std::env::var("GRPH_MCP_TOOLS").ok();
    let Some(raw) = allowlist else {
        return true;
    };
    let entries = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(normalize_tool_name)
        .collect::<std::collections::HashSet<_>>();
    if entries.is_empty() {
        return true;
    }
    entries.contains(&normalize_tool_name(tool_name))
}

pub fn normalize_tool_name(name: &str) -> String {
    let short = name.strip_prefix("grph_").unwrap_or(name);
    format!("grph_{}", short)
}
