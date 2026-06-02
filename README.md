# grph

Self-contained code intelligence CLI + MCP and LSP servers — a single binary
for semantic search, call graph traversal, and AI agent context across
Rust, Python, JavaScript/TypeScript, Go, C/C++, Shell, and embedded SQL/C.**

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.80+-orange.svg)](https://www.rust-lang.org)

---

## Features

- **Code graph extraction** — Functions, classes, interfaces, methods, imports, constants, decorators, and their relationships (`calls`, `contains`, `imports`, `extends`, `implements`, `decorates`)
- **Tree-sitter first, regex fallback** — precise AST extraction for supported languages; regex as safety net
- **Incremental sync** — `sync` detects changed files and replaces only stale graph fragments
- **Call graph traversal** — `callers`, `callees`, and bounded BFS `trace` between symbols
- **AI context builder** — Extracts keyword-matched source slices for agent task descriptions
- **MCP server** — JSON-RPC over stdio, 11 tools, compatible with Claude Desktop, opencode, Goose, and other MCP clients
- **LSP server** — JSON-RPC over stdio for editor features backed by the same `.grph/grph.db` index
- **Single binary** — ~12–14 MB, zero runtime dependencies
- **`.gitignore`-aware** — Uses the `ignore` crate; skips `node_modules/`, `target/`, etc.

## Supported Languages

| Language | Extensions | Tree-sitter | Regex Fallback |
|----------|------------|:-----------:|:--------------:|
| Rust | `.rs` | ✅ | ✅ |
| Python | `.py`, `.pyw`, `.pyi` | ✅ | ✅ |
| JavaScript | `.js`, `.mjs`, `.cjs`, `.jsx` | ✅ | ✅ |
| TypeScript | `.ts`, `.tsx` | ✅ | ✅ |
| Go | `.go` | ✅ | ✅ |
| C | `.c`, `.h` | ✅ | ✅ |
| C++ | `.cpp`, `.cc`, `.cxx`, `.c++`, `.hpp`, `.hxx`, `.h++` | ✅ | ✅ |
| Shell | `.sh`, `.bash` | ✅ | ✅ |
| ESQL/C and EQUEL/C | `.sc`, `.qsc`, `.qsh` | ✅ | ✅ |

## Installation

```bash
# From source (requires Rust 1.80+)
cargo install --path grph-cli

# Or build locally
cargo build --release
# Binary at: target/release/grph
```

## Quick Start

```bash
# Initialize the code graph in your project
grph init -i

# Search for symbols
grph query "handle_login"

# Search with JSON output
grph query "handle_login" --json

# Find who calls a function
grph callers handle_login

# Find what a function calls
grph callees handle_login

# Trace a path between two symbols
grph trace authenticate handle_login

# See project stats
grph status

# Build AI agent context for a task
grph context "fix the authentication bug in login handler"

# Start the MCP server from the current project
grph serve --mcp

# Start the LSP server from the current project
grph serve --lsp
```

## CLI Commands

| Command | Description |
|---------|-------------|
| `grph init` | Initialize `.grph/grph.db` database |
| `grph init -i` | Initialize + index all files |
| `grph index [--force] [--quiet] [-j <n>] [path]` | Extract symbols and edges from all source files |
| `grph sync [--file <path>] [path]` | Incremental sync — re-index changed files only |
| `grph status` | Show file / node / edge counts |
| `grph query <name> [--kind <kind>] [--limit <n>] [--json]` | LIKE-based search for symbols |
| `grph files [--format <fmt>] [--filter <pattern>] [--max-depth <n>] [--json]` | List indexed files |
| `grph callers <symbol> [--limit <n>] [--json]` | Find incoming `calls` edges |
| `grph callees <symbol> [--limit <n>] [--json]` | Find outgoing `calls` edges |
| `grph trace <from> <to>` | Bounded BFS path between two symbols |
| `grph context <task>` | Extract code context for an AI agent task |
| `grph explore <query>` | Grouped symbols + source snippets |
| `grph impact <symbol> [--depth <n>] [--json]` | Impact radius analysis |
| `grph ctags [--output <path>]` | Generate Universal Ctags-compatible `tags` file |
| `grph serve --mcp [--path <path>]` | Start MCP JSON-RPC server over stdio |
| `grph serve --lsp [--path <path>]` | Start LSP JSON-RPC server over stdio |
| `grph uninit [--force] [path]` | Remove `.grph` from a project |

## Ctags Generation

Generate a Universal Ctags-compatible `tags` file from indexed nodes — no re-parsing required. The tags use the tree-sitter line numbers already stored in the database.

```bash
# Generate tags in the project root
grph ctags

# Write to a custom path
grph ctags --output ~/.tags/myproject
```

**Output format** — extended format (`format=2`) with line-number addresses:

- Standard `{name}\t{file}\t{line};"\t{kind}` fields
- Extended fields: `kind:`, `line:`, `end:`, `language:`, `qualified:`, `signature:`
- Sorted alphabetically by name → file → line
- Header includes `!_TAG_FILE_FORMAT`, `!_TAG_FILE_SORTED`, `!_TAG_PROGRAM_NAME`, `!_TAG_PROGRAM_VERSION`
- Kind letters follow Universal Ctags conventions (`f`=function, `c`=class, `s`=struct, `m`=method, etc.)

Works with any editor that reads ctags files (Vim, Neovim, Emacs, VS Code with ctags extension).

## MCP Server

Start the MCP server for use with AI coding assistants:

```bash
grph serve --mcp
```

`grph serve --mcp` uses the current directory by default. Pass `--path /path/to/project` when the MCP client cannot set `cwd`.

Configure your MCP client (e.g., Claude Desktop):

```json
{
  "mcpServers": {
    "grph": {
      "command": "grph",
      "args": ["serve", "--mcp"],
      "cwd": "/path/to/your/project"
    }
  }
}
```

### MCP Tools

These are the tool names returned by `tools/list` and accepted by `tools/call`:

| Tool | Required arguments | Optional arguments | Description |
|------|--------------------|--------------------|-------------|
| `grph_search` | `query` | `kind`, `limit`, `json`, `projectPath` | Search symbols by name prefix |
| `grph_context` | `task` | `max_nodes`, `projectPath` | Build AI context for a task |
| `grph_callers` | `symbol` | `limit`, `projectPath` | Find what calls a symbol |
| `grph_uncalled` | | `limit`, `json`, `projectPath` | List functions with no callers |
| `grph_callees` | `symbol` | `limit`, `projectPath` | Find what a symbol calls |
| `grph_impact` | `symbol` | `depth`, `projectPath` | Analyze impact radius |
| `grph_node` | `symbol` | `projectPath` | Return one symbol node as JSON |
| `grph_status` | | `projectPath` | Show index statistics |
| `grph_files` | | `json`, `projectPath` | List indexed files |
| `grph_trace` | `from`, `to` | `projectPath` | Trace a call path between two symbols |
| `grph_explore` | `query` | `max_files`, `projectPath` | Explore symbols grouped by file |

Example `tools/call` requests:

```json
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"grph_search","arguments":{"query":"handle_login","limit":20}}}
```

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"grph_callers","arguments":{"symbol":"handle_login","projectPath":"/path/to/project"}}}
```

Tool arguments use snake_case for grph-specific options, such as `max_nodes` and `max_files`. `projectPath` selects the project root when the MCP client cannot provide one through `cwd`, `rootUri`, or `workspaceFolders`.

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `GRPH_MCP_TOOLS` | (all) | Comma-separated allowlist of MCP tools |
| `GRPH_MCP_FRAME_BYTES` | `1048576` | Max `Content-Length` frame size in bytes |
| `GRPH_MCP_MAX_MESSAGE_BYTES` | `1048576` | Max newline-delimited JSON message size in bytes |

## LSP Server

Start the LSP server for editors that can launch a stdio language server:

```bash
grph serve --lsp
```

`grph serve --lsp` uses the current directory by default. Pass `--path /path/to/project` when the editor cannot set `cwd`. The project must already be initialized and indexed with `grph init -i`.

Supported LSP features:

- Document symbols
- Go to definition
- References
- Hover
- Workspace symbol search
- Call hierarchy incoming and outgoing calls
- Incremental sync on document save

## Project Structure

```
grph/
├── grph-core/          # Core library — extraction, DB, graph traversal
│   └── src/
│       ├── extraction/  # Tree-sitter & regex extractors
│       ├── db/          # SQLite schema & queries
│       ├── graph/       # BFS, callers, callees, impact_radius
│       ├── context/     # AI context builder
│       ├── search/      # Symbol search
│       └── resolution/  # Cross-file reference resolution
├── grph-cli/           # CLI binary (clap-based)
├── grph-mcp/           # MCP JSON-RPC server
├── grph-lsp/           # LSP JSON-RPC server
├── Cargo.toml          # Workspace root
└── README.md           # This file
```

## How It Works

1. **Extraction** — Walks source files with `ignore::WalkBuilder`, parses each file with tree-sitter, extracts nodes (functions, classes, etc.) and edges (`calls`, `contains`, `extends`, `implements`, `decorates`). Falls back to regex if a tree-sitter grammar is unavailable.

2. **Resolution** — Resolves call targets to node IDs within the same file during extraction. Captures unresolved cross-file references and resolves them in a post-indexing pass.

3. **Storage** — Nodes and edges stored in SQLite (`grph.db`). Schema includes indexes for fast symbol lookup and graph traversal.

4. **Integration transports** — MCP and LSP both use JSON-RPC 2.0 over stdio. MCP supports Content-Length framing and line-delimited JSON, while LSP uses standard Content-Length framing.

## License

MIT
