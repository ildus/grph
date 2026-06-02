use crate::handlers::LspHandlers;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;

const MAX_FRAME_BYTES: usize = 1_048_576;

pub fn serve_stdio(project_root: PathBuf) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut stdout = std::io::stdout();
    let mut session = LspSession::new(project_root);

    loop {
        let Some(message) = read_message(&mut reader)? else {
            break;
        };
        for response in session.handle_message(&message) {
            write_message(&mut stdout, &response)?;
        }
    }
    Ok(())
}

struct LspSession {
    root: PathBuf,
    handlers: Option<LspHandlers>,
}

impl LspSession {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            handlers: None,
        }
    }

    fn handle_message(&mut self, message: &str) -> Vec<Value> {
        let parsed: Value = match serde_json::from_str(message) {
            Ok(value) => value,
            Err(err) => return vec![jsonrpc_error(None, -32700, format!("Parse error: {err}"))],
        };
        match parsed {
            Value::Array(items) => items
                .into_iter()
                .filter_map(|item| self.handle_value(item))
                .collect(),
            value => self.handle_value(value).into_iter().collect(),
        }
    }

    fn handle_value(&mut self, value: Value) -> Option<Value> {
        let id = value.get("id").cloned();
        let method = value.get("method").and_then(Value::as_str)?;
        let params = value.get("params").unwrap_or(&Value::Null);
        let is_notification = id.is_none();

        if method == "initialize" {
            let root = params
                .get("rootUri")
                .and_then(Value::as_str)
                .and_then(root_uri_to_path)
                .unwrap_or_else(|| self.root.clone());
            match LspHandlers::new(root) {
                Ok(handlers) => {
                    let result = handlers.initialize_result();
                    self.handlers = Some(handlers);
                    return Some(json!({"jsonrpc": "2.0", "id": id, "result": result}));
                }
                Err(err) => {
                    return Some(jsonrpc_error(
                        id,
                        -32603,
                        format!("Failed to open Grph project: {err}"),
                    ))
                }
            }
        }

        if matches!(method, "initialized" | "exit") {
            return None;
        }
        if method == "shutdown" {
            return id.map(|id| json!({"jsonrpc": "2.0", "id": id, "result": null}));
        }

        let Some(handlers) = self.handlers.as_mut() else {
            return if is_notification {
                None
            } else {
                Some(jsonrpc_error(id, -32002, "Server not initialized"))
            };
        };

        let result = match method {
            "textDocument/documentSymbol" => handlers.document_symbol(params),
            "textDocument/definition" => handlers.definition(params),
            "textDocument/references" => handlers.references(params),
            "textDocument/hover" => handlers.hover(params),
            "workspace/symbol" => handlers.workspace_symbol(params),
            "textDocument/prepareCallHierarchy" => handlers.prepare_call_hierarchy(params),
            "callHierarchy/incomingCalls" => handlers.incoming_calls(params),
            "callHierarchy/outgoingCalls" => handlers.outgoing_calls(params),
            "textDocument/didOpen" => {
                handlers.did_open(params);
                return None;
            }
            "textDocument/didChange" => {
                handlers.did_change(params);
                return None;
            }
            "textDocument/didClose" => {
                handlers.did_close(params);
                return None;
            }
            "textDocument/didSave" => {
                if let Err(err) = handlers.did_save(params) {
                    eprintln!("grph-lsp: sync failed: {err}");
                }
                return None;
            }
            _ => {
                return if is_notification {
                    None
                } else {
                    Some(jsonrpc_error(
                        id,
                        -32601,
                        format!("Method not found: {method}"),
                    ))
                }
            }
        };

        if is_notification {
            None
        } else {
            Some(match result {
                Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                Err(err) => jsonrpc_error(id, -32603, err.to_string()),
            })
        }
    }
}

fn read_message<R: BufRead + Read>(reader: &mut R) -> std::io::Result<Option<String>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            content_length = value.trim().parse::<usize>().ok();
        }
    }
    let Some(len) = content_length else {
        return Ok(None);
    };
    if len > MAX_FRAME_BYTES {
        drain(reader, len);
        return Ok(None);
    }
    let mut buf = vec![0; len];
    reader.read_exact(&mut buf)?;
    Ok(Some(String::from_utf8_lossy(&buf).to_string()))
}

fn write_message<W: Write>(writer: &mut W, value: &Value) -> std::io::Result<()> {
    let body = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    write!(
        writer,
        "Content-Length: {}\r\n\r\n{}",
        body.as_bytes().len(),
        body
    )?;
    writer.flush()
}

fn jsonrpc_error(id: Option<Value>, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

fn root_uri_to_path(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file://")?;
    let decoded = urlencoding::decode(raw).ok()?.to_string();
    Some(PathBuf::from(decoded))
}

fn drain<R: Read>(reader: &mut R, len: usize) {
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
