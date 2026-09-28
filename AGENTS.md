# grph agent notes

Semantic code graph CLI plus MCP and LSP servers. Workspace crates: `grph-core` (extraction, SQLite, resolution), `grph-cli`, `grph-mcp`, `grph-lsp`. The database is `.grph/grph.db`.

## Commands

- `grph index` creates the database if needed and updates changed files. Change detection is size plus mtime (`stored mtime >= file mtime` and same size means unchanged). Keep that check.
- `grph index --file <path>` refreshes one file. After you create, edit, or delete source, run this for each changed path before the next query. Say so in `grph-mcp/src/instructions.rs` if that guidance changes.
- `grph index --force` rebuilds the whole graph. `grph index --resolve` resolves pending refs without parsing.
- `query`, `status`, `serve`, and the other read commands must not create `.grph/`. Missing database is `Not initialized: run grph index first`.
- `--compile-commands <path>` is stored and reused on later indexes, including `--file`. `--no-compile-commands` clears it. The two flags together are an error. When a hint is active, say it is used while indexing and that the scan is not restricted.

## Indexing invariants

- Replacing a file's nodes must keep cross-file edges that point at those symbols. Node ids include the start line, so retarget saved incoming edges onto the new ids (qualified name, then unique name, then closest line). Drop the edge only when the symbol is gone. `delete_file_nodes` still removes incoming edges for a deleted file.
- A failed store batch fails the index. Do not log it and exit 0.
- `Database` clone must reopen `grph.db`. Never fall back to an in-memory database.
- If symbol-search triggers (`nodes_ai`, `nodes_ad`, `nodes_au`) are missing on open, recreate them and rebuild `nodes_fts`. A killed large index drops those triggers before the rebuild.

## Tests and release

- `cargo test --workspace`
- CLI behavior lives in `grph-cli/tests/cli_integration.rs`. Graph and extraction behavior lives in `grph-core/tests/tree_sitter_integration.rs`.
- Version is the same in all four `Cargo.toml` files, `Cargo.lock`, the MCP `serverInfo` version, and the ctags `!_TAG_PROGRAM_VERSION` header.
- Release targets are in `dist-workspace.toml`. `x86_64-unknown-linux-musl` is the static musl build. The installer is `grph-cli-installer.sh` from the GitHub release.
