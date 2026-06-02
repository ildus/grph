use serde_json::{json, Map, Value};
use std::path::PathBuf;

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

    match open(project_root).and_then(|grph| search_first(&grph, symbol)) {
        Ok(Some(node)) => json_response(json!(node)),
        Ok(None) => mcp_error(format!("Symbol not found: {}", symbol)),
        Err(e) => mcp_error(format!("Node lookup failed: {}", e)),
    }
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
    let max_files = arg_u32(args, "max_files", 12) as usize;

    match open(project_root).and_then(|grph| explore_nodes(&grph, query, (max_files as u32) * 5)) {
        Ok(nodes) => {
            let mut by_file = std::collections::BTreeMap::<String, Vec<grph_core::Node>>::new();
            for node in nodes {
                by_file
                    .entry(node.file_path.clone())
                    .or_default()
                    .push(node);
            }
            let mut out = String::new();
            for (file, nodes) in by_file.into_iter().take(max_files) {
                out.push_str(&format!("### {}\n", file));
                for node in nodes.iter().take(20) {
                    out.push_str(&format!("  {} (line {})\n", node.name, node.start_line));
                }
            }
            text_response(if out.is_empty() {
                "No symbols found.".to_string()
            } else {
                out
            })
        }
        Err(e) => mcp_error(format!("Explore failed: {}", e)),
    }
}

fn explore_nodes(
    grph: &grph_core::Grph,
    query: &str,
    limit: u32,
) -> grph_core::Result<Vec<grph_core::Node>> {
    let nodes = grph.search(query, None, limit)?;
    if !nodes.is_empty() {
        return Ok(nodes);
    }

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for term in query.split_whitespace().filter(|term| term.len() > 1) {
        for node in grph.search(term, None, limit)? {
            if seen.insert(node.id.clone()) {
                out.push(node);
                if out.len() >= limit as usize {
                    return Ok(out);
                }
            }
        }
    }
    Ok(out)
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
        json!({"name":"grph_context","description":"Build AI context for a task","inputSchema":{"type":"object","properties":{"task":{"type":"string"},"max_nodes":{"type":"integer"},"projectPath":{"type":"string"}},"required":["task"]}}),
        json!({"name":"grph_callers","description":"Find what calls a symbol","inputSchema":{"type":"object","properties":{"symbol":{"type":"string"},"limit":{"type":"integer"},"projectPath":{"type":"string"}},"required":["symbol"]}}),
        json!({"name":"grph_uncalled","description":"List functions with no callers","inputSchema":{"type":"object","properties":{"limit":{"type":"integer"},"json":{"type":"boolean"},"projectPath":{"type":"string"}}}}),
        json!({"name":"grph_callees","description":"Find what a symbol calls","inputSchema":{"type":"object","properties":{"symbol":{"type":"string"},"limit":{"type":"integer"},"projectPath":{"type":"string"}},"required":["symbol"]}}),
        json!({"name":"grph_impact","description":"Analyze impact radius","inputSchema":{"type":"object","properties":{"symbol":{"type":"string"},"depth":{"type":"integer"},"projectPath":{"type":"string"}},"required":["symbol"]}}),
        json!({"name":"grph_node","description":"Return a symbol node as JSON","inputSchema":{"type":"object","properties":{"symbol":{"type":"string"},"projectPath":{"type":"string"}},"required":["symbol"]}}),
        json!({"name":"grph_status","description":"Show index statistics","inputSchema":{"type":"object","properties":{"projectPath":{"type":"string"}}}}),
        json!({"name":"grph_files","description":"List indexed files","inputSchema":{"type":"object","properties":{"json":{"type":"boolean"},"projectPath":{"type":"string"}}}}),
        json!({"name":"grph_trace","description":"Trace call path between two symbols","inputSchema":{"type":"object","properties":{"from":{"type":"string"},"to":{"type":"string"},"projectPath":{"type":"string"}},"required":["from","to"]}}),
        json!({"name":"grph_explore","description":"Explore symbols grouped by file","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"max_files":{"type":"integer"},"projectPath":{"type":"string"}},"required":["query"]}}),
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
