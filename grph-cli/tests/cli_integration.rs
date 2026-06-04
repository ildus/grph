use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_project(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("grph-cli-{name}-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn cli_indexes_and_traces_python_project() {
    let dir = temp_project("trace");
    fs::write(
        dir.join("main.py"),
        r#"
def say_hello(name):
    return name.upper()

def greet(name):
    return say_hello(name)

class App:
    def run(self):
        return greet("world")
"#,
    )
    .unwrap();

    let grph = env!("CARGO_BIN_EXE_grph");

    let init = Command::new(grph)
        .args(["init", "-i"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let query = Command::new(grph)
        .args(["query", "greet"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(query.status.success());
    let query_stdout = String::from_utf8_lossy(&query.stdout);
    assert!(query_stdout.contains("greet"));
    assert!(
        !query_stdout.contains("def greet(name): {"),
        "query output should show signatures, not body-open markers: {query_stdout}"
    );

    let glob_query = Command::new(grph)
        .args(["query", "gre*"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(glob_query.status.success());
    assert!(String::from_utf8_lossy(&glob_query.stdout).contains("greet"));

    let trace = Command::new(grph)
        .args(["trace", "run", "say_hello"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        trace.status.success(),
        "trace failed: {}",
        String::from_utf8_lossy(&trace.stderr)
    );
    let stdout = String::from_utf8_lossy(&trace.stdout);
    assert!(stdout.contains("run"));
    assert!(stdout.contains("greet"));
    assert!(stdout.contains("say_hello"));

    fs::remove_dir_all(dir).ok();
}

#[test]
fn cli_query_prioritizes_source_over_build_artifacts() {
    let dir = temp_project("query-rank");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("target/debug/build/tree-sitter/out")).unwrap();
    fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(
        dir.join("target/debug/build/tree-sitter/out/flag_check.c"),
        "int main(void) { return 0; }\n",
    )
    .unwrap();

    let grph = env!("CARGO_BIN_EXE_grph");
    let init = Command::new(grph)
        .args(["init", "-i"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let query = Command::new(grph)
        .args(["query", "main"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(query.status.success());
    let stdout = String::from_utf8_lossy(&query.stdout);
    assert!(stdout.contains("Search Results for \"main\""), "{stdout}");
    assert!(stdout.contains("src/main.rs"), "{stdout}");
    assert!(!stdout.contains("flag_check.c"), "{stdout}");

    fs::remove_dir_all(dir).ok();
}

#[test]
fn cli_init_is_idempotent() {
    let dir = temp_project("init-idempotent");
    let grph = env!("CARGO_BIN_EXE_grph");

    let first = Command::new(grph).arg("init").arg(&dir).output().unwrap();
    assert!(
        first.status.success(),
        "first init failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    let second = Command::new(grph).arg("init").arg(&dir).output().unwrap();
    assert!(
        second.status.success(),
        "second init should be idempotent, stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    let init_index = Command::new(grph)
        .args(["init", "-i"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        init_index.status.success(),
        "init -i should also be idempotent, stderr: {}",
        String::from_utf8_lossy(&init_index.stderr)
    );

    fs::remove_dir_all(dir).ok();
}

#[test]
fn cli_query_formats_rust_signature_without_body_marker() {
    let dir = temp_project("rust-signature");
    fs::write(
        dir.join("main.rs"),
        "fn main() -> Result<(), Box<dyn std::error::Error>> {\n    Ok(())\n}\n",
    )
    .unwrap();

    let grph = env!("CARGO_BIN_EXE_grph");
    let init = Command::new(grph)
        .args(["init", "-i"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let query = Command::new(grph)
        .args(["query", "main"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(query.status.success());
    let stdout = String::from_utf8_lossy(&query.stdout);
    assert!(stdout.contains("function    main"));
    assert!(stdout.contains("() -> Result<(), Box<dyn std::error::Error>>"));
    assert!(
        !stdout.contains("fn main()"),
        "expected compact signature: {stdout}"
    );
    assert!(
        !stdout.contains("{\n"),
        "body opener leaked into signature: {stdout}"
    );

    fs::remove_dir_all(dir).ok();
}

#[test]
fn cli_generates_universal_ctags_file() {
    let dir = temp_project("ctags");
    fs::write(
        dir.join("main.rs"),
        "struct App;\nfn main() -> Result<(), ()> {\n    Ok(())\n}\n",
    )
    .unwrap();

    let grph = env!("CARGO_BIN_EXE_grph");
    let init = Command::new(grph)
        .args(["init", "-i"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let ctags = Command::new(grph).arg("ctags").arg(&dir).output().unwrap();
    assert!(
        ctags.status.success(),
        "ctags failed: {}",
        String::from_utf8_lossy(&ctags.stderr)
    );

    let tags = fs::read_to_string(dir.join("tags")).unwrap();
    assert!(tags.contains("!_TAG_FILE_FORMAT\t2"), "{tags}");
    assert!(
        tags.contains("main\tmain.rs\t2;\"\tf\tkind:function"),
        "{tags}"
    );
    assert!(
        tags.contains("App\tmain.rs\t1;\"\ts\tkind:struct"),
        "{tags}"
    );

    fs::remove_dir_all(dir).ok();
}

fn send_mcp_frame(stdin: &mut std::process::ChildStdin, value: serde_json::Value) {
    use std::io::Write;
    let payload = value.to_string();
    write!(
        stdin,
        "Content-Length: {}\r\n\r\n{}",
        payload.as_bytes().len(),
        payload
    )
    .unwrap();
    stdin.flush().unwrap();
}

fn read_mcp_frame(stdout: &mut std::process::ChildStdout) -> serde_json::Value {
    use std::io::Read;
    let mut header = Vec::new();
    let mut buf = [0u8; 1];
    while !header.ends_with(b"\r\n\r\n") {
        stdout.read_exact(&mut buf).unwrap();
        header.push(buf[0]);
    }
    let header_text = String::from_utf8(header).unwrap();
    let len = header_text
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .expect("Content-Length header");
    let mut payload = vec![0u8; len];
    stdout.read_exact(&mut payload).unwrap();
    serde_json::from_slice(&payload).unwrap()
}

#[test]
fn cli_mcp_stdio_content_length_end_to_end() {
    let dir = temp_project("mcp-stdio");
    fs::write(dir.join("main.py"), "def greet():\n    return 1\n").unwrap();

    let grph = env!("CARGO_BIN_EXE_grph");
    let init = Command::new(grph)
        .args(["init", "-i"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let mut child = Command::new(grph)
        .args(["serve", "--mcp"])
        .current_dir(&dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    send_mcp_frame(
        &mut stdin,
        serde_json::json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{"capabilities":{},"clientInfo":{"name":"test","version":"0"}}
        }),
    );
    let init_response = read_mcp_frame(&mut stdout);
    assert_eq!(init_response["id"], serde_json::json!(1));
    assert_eq!(init_response["result"]["serverInfo"]["name"], "grph-mcp");

    send_mcp_frame(
        &mut stdin,
        serde_json::json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{"name":"grph_search","arguments":{"query":"greet"}}
        }),
    );
    let search_response = read_mcp_frame(&mut stdout);
    assert_eq!(search_response["id"], serde_json::json!(2));
    let text = search_response["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("greet"), "{text}");

    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    fs::remove_dir_all(dir).ok();
}

#[test]
fn cli_context_surfaces_relevant_tail_callee_from_large_function() {
    let dir = temp_project("context-tail-callee");
    let mut source = String::new();
    source.push_str(
        "fn bootstrap() {
",
    );
    source.push_str(
        "    // Widget relocation failure happens in this orchestration path.
",
    );
    for i in 0..30 {
        source.push_str(&format!(
            "    utility_{i}();
"
        ));
    }
    source.push_str(
        "    specialized_tail_helper();
",
    );
    source.push_str(
        "}

",
    );
    for i in 0..30 {
        source.push_str(&format!(
            "fn utility_{i}() {{}}
"
        ));
    }
    source.push_str(
        "fn specialized_tail_helper() {}
",
    );
    fs::write(dir.join("main.rs"), source).unwrap();

    let grph = env!("CARGO_BIN_EXE_grph");
    let init = Command::new(grph)
        .args(["init", "-i"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let context = Command::new(grph)
        .args([
            "context",
            "--max-nodes",
            "8",
            "widget relocation failure orchestration path",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        context.status.success(),
        "context failed: {}",
        String::from_utf8_lossy(&context.stderr)
    );
    let stdout = String::from_utf8_lossy(&context.stdout);
    assert!(stdout.contains("bootstrap"), "{stdout}");
    assert!(
        stdout.contains("specialized_tail_helper"),
        "tail callee should survive context budget despite many earlier utility calls: {stdout}"
    );
    assert!(
        stdout.contains("callee of bootstrap"),
        "related-symbol output should explain why the helper is relevant: {stdout}"
    );
    assert!(
        stdout.contains("specialized_tail_helper();"),
        "code snippets should center on selected call sites when the full function is too large: {stdout}"
    );

    fs::remove_dir_all(dir).ok();
}
