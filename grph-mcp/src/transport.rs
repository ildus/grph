use crate::{
    instructions::SERVER_INSTRUCTIONS,
    tools::{self, is_tool_allowed, normalize_tool_name, tool_defs, validate_tool_args},
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

const DEFAULT_MAX_MESSAGE_BYTES: usize = 1_048_576;

/// Handle a single MCP JSON-RPC message with a fresh stateless session.
///
/// Tests that need roots/list or response correlation should use `McpSession`.
pub fn handle_message(message: &str, project_root: &PathBuf) -> String {
    let mut session = McpSession::new(project_root.clone());
    session.handle_message(message).join("")
}

#[derive(Clone, Debug)]
struct PendingToolCall {
    id: Option<Value>,
    tool_name: String,
    args: serde_json::Map<String, Value>,
}

#[derive(Debug)]
pub struct McpSession {
    launch_root: PathBuf,
    project_root: Option<PathBuf>,
    client_supports_roots: bool,
    pending_roots: HashMap<String, PendingToolCall>,
    next_server_request_id: u64,
}

impl McpSession {
    pub fn new(launch_root: PathBuf) -> Self {
        let project_root = if has_grph_db(&launch_root) {
            Some(launch_root.clone())
        } else {
            None
        };
        Self {
            launch_root,
            project_root,
            client_supports_roots: false,
            pending_roots: HashMap::new(),
            next_server_request_id: 1,
        }
    }

    pub fn handle_message(&mut self, message: &str) -> Vec<String> {
        if message.as_bytes().len() > max_message_bytes() {
            return vec![serialize_response(json!({
                "jsonrpc": "2.0",
                "error": {"code": -32600, "message": "Request too large"},
                "id": null
            }))];
        }

        let parsed: Value = match serde_json::from_str(message) {
            Ok(v) => v,
            Err(e) => {
                return vec![serialize_response(json!({
                    "jsonrpc": "2.0",
                    "error": {"code": -32700, "message": format!("Parse error: {}", e)},
                    "id": null
                }))];
            }
        };

        if let Value::Array(items) = parsed {
            if items.is_empty() {
                return vec![serialize_response(json!({
                    "jsonrpc": "2.0",
                    "error": {"code": -32600, "message": "Invalid Request: empty batch"},
                    "id": null
                }))];
            }
            let mut responses = Vec::new();
            for item in items {
                if let Some(response) = self.handle_value(item) {
                    responses.push(response);
                }
            }
            if responses.is_empty() {
                Vec::new()
            } else {
                vec![serialize_response(Value::Array(responses))]
            }
        } else {
            self.handle_value(parsed)
                .map(serialize_response)
                .into_iter()
                .collect()
        }
    }

    fn handle_value(&mut self, parsed: Value) -> Option<Value> {
        let obj = match parsed.as_object() {
            Some(obj) => obj,
            None => {
                return Some(jsonrpc_error(
                    None,
                    -32600,
                    "Invalid Request: expected object",
                ))
            }
        };

        let id = obj.get("id").cloned();
        let is_notification = id.is_none();

        // Client response to a server-initiated roots/list request.
        if obj.get("method").is_none() && obj.contains_key("result") {
            if let Some(id_key) = id.as_ref().and_then(json_id_key) {
                if let Some(pending) = self.pending_roots.remove(&id_key) {
                    if let Some(root) = first_root_uri_to_path(obj.get("result")) {
                        self.project_root = Some(root.clone());
                        return Some(self.execute_tool_call(
                            pending.id,
                            &pending.tool_name,
                            pending.args,
                            root,
                        ));
                    }
                    return Some(tool_call_result(
                        pending.id,
                        tools::mcp_error("No Grph project is loaded. Client roots/list returned no usable file:// root. Pass projectPath in the tool arguments or start grph serve --mcp from an initialized project."),
                    ));
                }
            }
            return None;
        }

        let method = match obj.get("method").and_then(|m| m.as_str()) {
            Some(method) => method,
            None => return Some(jsonrpc_error(id, -32600, "Invalid Request: missing method")),
        };

        match method {
            "initialize" => {
                self.apply_initialize_params(obj.get("params"));
                if is_notification {
                    None
                } else {
                    Some(handle_initialize(id))
                }
            }
            "initialized" | "notifications/initialized" => None,
            "tools/list" => {
                if is_notification {
                    None
                } else {
                    Some(handle_tools_list(id))
                }
            }
            "tools/call" => {
                if is_notification {
                    return None;
                }
                Some(self.handle_tool_call(obj.get("params"), id))
            }
            _ => {
                if is_notification {
                    None
                } else {
                    Some(jsonrpc_error(
                        id,
                        -32601,
                        format!("Method not found: {}", method),
                    ))
                }
            }
        }
    }

    fn apply_initialize_params(&mut self, params: Option<&Value>) {
        let Some(params) = params.and_then(|p| p.as_object()) else {
            return;
        };
        self.client_supports_roots = params
            .get("capabilities")
            .and_then(|c| c.get("roots"))
            .is_some();

        if let Some(path) = params
            .get("rootUri")
            .and_then(Value::as_str)
            .and_then(file_uri_to_path)
        {
            self.project_root = Some(path);
            return;
        }
        if let Some(path) = params
            .get("workspaceFolders")
            .and_then(Value::as_array)
            .and_then(|folders| {
                folders.iter().find_map(|f| {
                    f.get("uri")
                        .and_then(Value::as_str)
                        .and_then(file_uri_to_path)
                })
            })
        {
            self.project_root = Some(path);
        }
    }

    fn handle_tool_call(&mut self, params: Option<&Value>, id: Option<Value>) -> Value {
        let Some(params) = params.and_then(Value::as_object) else {
            return jsonrpc_error(id, -32602, "Invalid params: expected object");
        };
        let Some(tool_name) = params.get("name").and_then(Value::as_str) else {
            return jsonrpc_error(id, -32602, "Invalid params: missing string field 'name'");
        };
        let args = match params.get("arguments") {
            Some(Value::Object(args)) => args.clone(),
            Some(_) => {
                return jsonrpc_error(id, -32602, "Invalid params: arguments must be an object")
            }
            None => serde_json::Map::new(),
        };

        if !is_tool_allowed(tool_name) {
            return tool_call_result(
                id,
                tools::mcp_error(format!(
                    "Tool {tool_name} is disabled via GRPH_MCP_TOOLS"
                )),
            );
        }

        if let Err(message) = validate_tool_args(tool_name, &args) {
            return tool_call_result(id, tools::mcp_error(message));
        }

        if let Some(project_path) = args.get("projectPath").and_then(Value::as_str) {
            let requested = PathBuf::from(project_path);
            let root = if has_grph_db(&requested) {
                requested
            } else if let Some(root) = self.project_root.clone().filter(|root| has_grph_db(root)) {
                root
            } else if has_grph_db(&self.launch_root) {
                self.launch_root.clone()
            } else {
                requested
            };
            return self.execute_tool_call(id, tool_name, args.clone(), root);
        }

        if let Some(root) = self.project_root.clone() {
            return self.execute_tool_call(id, tool_name, args, root);
        }

        if self.client_supports_roots {
            let server_id = format!("grph-roots-{}", self.next_server_request_id);
            self.next_server_request_id += 1;
            self.pending_roots.insert(
                server_id.clone(),
                PendingToolCall {
                    id,
                    tool_name: tool_name.to_string(),
                    args,
                },
            );
            return json!({"jsonrpc":"2.0", "id": server_id, "method":"roots/list", "params": {}});
        }

        tool_call_result(id, tools::mcp_error(no_project_message(&self.launch_root)))
    }

    fn execute_tool_call(
        &mut self,
        id: Option<Value>,
        tool_name: &str,
        args: serde_json::Map<String, Value>,
        project_root: PathBuf,
    ) -> Value {
        let normalized_tool_name = normalize_tool_name(tool_name);
        let result = match normalized_tool_name.as_str() {
            "grph_search" => tools::handle_search(&args, &project_root),
            "grph_context" => tools::handle_context(&args, &project_root),
            "grph_callers" => tools::handle_callers(&args, &project_root),
            "grph_uncalled" => tools::handle_uncalled(&args, &project_root),
            "grph_callees" => tools::handle_callees(&args, &project_root),
            "grph_impact" => tools::handle_impact(&args, &project_root),
            "grph_node" => tools::handle_node(&args, &project_root),
            "grph_status" => tools::handle_status(&args, &project_root),
            "grph_files" => tools::handle_files(&args, &project_root),
            "grph_trace" => tools::handle_trace(&args, &project_root),
            "grph_explore" => tools::handle_explore(&args, &project_root),
            _ => tools::mcp_error(format!("Unknown tool: {}", tool_name)),
        };
        tool_call_result(id, result)
    }
}

fn handle_initialize(id: Option<Value>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "result": {
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {"listChanged": false}
            },
            "serverInfo": {"name": "grph-mcp", "version": "0.1.0"},
            "instructions": SERVER_INSTRUCTIONS
        },
        "id": id
    })
}

fn handle_tools_list(id: Option<Value>) -> Value {
    json!({"jsonrpc":"2.0", "result": {"tools": tool_defs()}, "id": id})
}

fn tool_call_result(id: Option<Value>, result: Value) -> Value {
    json!({"jsonrpc":"2.0", "result": result, "id": id})
}

fn jsonrpc_error(id: Option<Value>, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc":"2.0", "error": {"code": code, "message": message.into()}, "id": id})
}

fn serialize_response(value: Value) -> String {
    serde_json::to_string(&value).unwrap_or_else(|e| {
        json!({"jsonrpc":"2.0", "error":{"code":-32603,"message":format!("Internal error: {}", e)}, "id":null}).to_string()
    })
}

fn max_message_bytes() -> usize {
    std::env::var("GRPH_MCP_MAX_MESSAGE_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_MESSAGE_BYTES)
}

fn json_id_key(id: &Value) -> Option<String> {
    match id {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn has_grph_db(path: &std::path::Path) -> bool {
    path.join(".grph").join("grph.db").exists()
}

fn no_project_message(launch_root: &std::path::Path) -> String {
    format!(
        "No Grph project is loaded. Looked in {}. Start grph serve --mcp from an initialized project, initialize with rootUri/workspaceFolders, or pass projectPath in the tool arguments.",
        launch_root.display()
    )
}

fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    #[cfg(windows)]
    {
        Some(PathBuf::from(rest.trim_start_matches('/')))
    }
    #[cfg(not(windows))]
    {
        Some(PathBuf::from(rest))
    }
}

fn first_root_uri_to_path(result: Option<&Value>) -> Option<PathBuf> {
    result?.get("roots")?.as_array()?.iter().find_map(|root| {
        root.get("uri")
            .and_then(Value::as_str)
            .and_then(file_uri_to_path)
    })
}

/// Maximum Content-Length value we'll accept. MCP messages larger than this
/// are pathological (e.g. the user asked for `grph_context` on every file in
/// the codebase). Default: 1 MiB. Override with `GRPH_MCP_FRAME_BYTES`.
const MAX_FRAME_BYTES: usize = 1_048_576;

/// Serve MCP JSON-RPC over stdio.
///
/// Supports the standard `Content-Length: N\r\n\r\n<json>` framing used by
/// MCP clients and also accepts newline-delimited JSON for simple smoke tests.
///
/// Malformed / partial frames are handled defensively:
/// - Non-numeric, zero, or negative Content-Length → error-and-recover
/// - Oversized Content-Length (> MAX_FRAME_BYTES) → error-and-recover
/// - Truncated body after valid Content-Length → protocol error + reset
/// - Multiple Content-Length headers → last one wins (spec-compliant)
/// - Garbage before {/[/Content-Length → protocol error + drain line
pub fn serve_stdio(project_root: PathBuf) -> std::io::Result<()> {
    use std::io::{BufRead, BufReader, Read, Write};

    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut stdout = std::io::stdout();
    let mut session = McpSession::new(project_root);

    let max_frame_bytes = max_frame_bytes();

    loop {
        let mut first_line = String::new();
        match reader.read_line(&mut first_line) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                eprintln!("grph-mcp: stdin read error: {e}");
                break;
            }
        }

        let first_trimmed = first_line.trim_end_matches(['\r', '\n']);
        if first_trimmed.is_empty() {
            continue;
        }

        // --- Newline-delimited JSON path (no Content-Length framing) ---
        if first_trimmed.starts_with('{') || first_trimmed.starts_with('[') {
            let message = first_trimmed.to_string();
            for response in session.handle_message(&message) {
                if response.is_empty() {
                    continue;
                }
                writeln!(stdout, "{}", response)?;
                stdout.flush()?;
            }
            continue;
        }

        // --- Content-Length-framed path ---
        let mut content_length: Option<usize> = parse_content_length_safe(first_trimmed);
        let mut header_err = None;

        loop {
            let mut header = String::new();
            match reader.read_line(&mut header) {
                Ok(0) => return Ok(()),
                Ok(_) => {}
                Err(_) => {
                    header_err = Some("stdin read error while reading headers".to_string());
                    break;
                }
            }
            let trimmed = header.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break;
            }
            // Multiple Content-Length headers: spec says last one wins.
            if let Some(cl) = parse_content_length_safe(trimmed) {
                content_length = Some(cl);
            }
        }

        if let Some(err) = header_err {
            eprintln!("grph-mcp: {err}");
            if let Err(e) = write_error_response(
                &mut stdout,
                jsonrpc_error(None, -32600, format!("Protocol error: {err}")),
            ) {
                eprintln!("grph-mcp: failed to write error response: {e}");
            }
            continue;
        }

        match content_length {
            Some(len) if len > max_frame_bytes => {
                let err_msg = format!(
                    "Content-Length {len} exceeds maximum {max_frame_bytes}. \
                     Set GRPH_MCP_FRAME_BYTES to increase."
                );
                eprintln!("grph-mcp: {err_msg}");
                if let Err(e) = write_error_response(
                    &mut stdout,
                    jsonrpc_error(None, -32600, format!("Protocol error: {err_msg}")),
                ) {
                    eprintln!("grph-mcp: failed to write error response: {e}");
                }
                // Drain the oversized body so the stream stays aligned.
                drain(&mut reader, len);
                continue;
            }
            Some(0) => {
                // Content-Length: 0 — valid (empty request), skip.
                continue;
            }
            Some(len) => {
                let mut buf = vec![0u8; len];
                match reader.read_exact(&mut buf) {
                    Ok(()) => {
                        let message = String::from_utf8_lossy(&buf).to_string();
                        for response in session.handle_message(&message) {
                            if response.is_empty() {
                                continue;
                            }
                            write!(
                                stdout,
                                "Content-Length: {}\r\n\r\n{}",
                                response.as_bytes().len(),
                                response
                            )?;
                            stdout.flush()?;
                        }
                    }
                    Err(e) => {
                        let err_msg =
                            format!("Truncated frame: expected {len} bytes but read failed: {e}");
                        eprintln!("grph-mcp: {err_msg}");
                        if let Err(e) = write_error_response(
                            &mut stdout,
                            jsonrpc_error(None, -32600, format!("Protocol error: {err_msg}")),
                        ) {
                            eprintln!("grph-mcp: failed to write error response: {e}");
                        }
                        // Don't drain — we already consumed what we could.
                        // Just continue to the next frame.
                        continue;
                    }
                }
            }
            None => {
                let err_msg = format!(
                    "Protocol error: expected Content-Length header but got '{first_trimmed}'"
                );
                eprintln!("grph-mcp: {err_msg}");
                if let Err(e) = write_error_response(
                    &mut stdout,
                    jsonrpc_error(None, -32600, format!("{err_msg}")),
                ) {
                    eprintln!("grph-mcp: failed to write error response: {e}");
                }
                // Resume at the next frame (headers loop already consumed
                // headers up to the blank-line separator).
            }
        }
    }

    Ok(())
}

/// Parse a Content-Length header value. Returns `None` for non-numeric,
/// negative, or empty values (rather than panicking on `.parse()`).
fn parse_content_length_safe(header: &str) -> Option<usize> {
    let (name, value) = header.split_once(':')?;
    if !name.eq_ignore_ascii_case("content-length") {
        return None;
    }
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    // Reject negative values (would wrap-around with `parse::<usize>`).
    if value.starts_with('-') {
        return None;
    }
    value.parse::<usize>().ok()
}

/// Discard up to `len` bytes from the reader so the stream stays aligned
/// after refusing an oversized frame.
fn drain<R: std::io::Read>(reader: &mut R, len: usize) {
    let mut sink = vec![0u8; 8192];
    let mut remaining = len;
    while remaining > 0 {
        let chunk = remaining.min(sink.len());
        match reader.read(&mut sink[..chunk]) {
            Ok(0) | Err(_) => break,
            Ok(n) => remaining -= n,
        }
    }
}

fn write_error_response<W: std::io::Write>(w: &mut W, err: Value) -> std::io::Result<()> {
    let body = serde_json::to_string(&err).unwrap_or_else(|_| {
        r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"Internal error"},"id":null}"#
            .to_string()
    });
    write!(
        w,
        "Content-Length: {}\r\n\r\n{}",
        body.as_bytes().len(),
        body
    )?;
    w.flush()
}

fn max_frame_bytes() -> usize {
    std::env::var("GRPH_MCP_FRAME_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(MAX_FRAME_BYTES)
}

// `parse_content_length` is now an alias that wraps `parse_content_length_safe`
// for backward compatibility with existing callers.
#[allow(dead_code)]
fn parse_content_length(header: &str) -> Option<usize> {
    parse_content_length_safe(header)
}
