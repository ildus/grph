/// Server instructions for MCP agents
pub const SERVER_INSTRUCTIONS: &str = r#"Grph is a semantic code intelligence tool. Use it to understand and query codebases.

## Answering Directly
Start with **grph_context** for code questions. Use 2-3 grph calls maximum; do not delegate to grep/read unless grph output is insufficient.

## Tool Selection
- **grph_context**: PRIMARY first call for code understanding, architecture, features, bugs, and “how does X work”; returns ranked entry points, related symbols/files, key code, call-path hints, confidence, and a short “Next reads” list
- **grph_search**: Quick symbol search by name when you already know a symbol
- **grph_trace**: Find call path between two symbols
- **grph_callers**: Who calls this symbol?
- **grph_uncalled**: Functions with no callers
- **grph_callees**: What does this symbol call?
- **grph_impact**: Impact radius of changing a symbol
- **grph_node**: Details about a specific symbol
- **grph_explore**: Verbatim line-numbered source for several related symbols grouped by file; use after context for targeted follow-up instead of many node/read calls
- **grph_files**: File structure from the index
- **grph_status**: Index statistics

## Query Tips (ranking is algorithmic)
- Prefer concrete signals: symbol names, `file:line`, or paste a compiler diagnostic
- For type/prototype errors, include the `note: expected ... but argument is of type ...` line — context treats this as `debug-compile` intent and boosts that locus
- Avoid wrapper phrases like “get ranked context for …”; pass the task/topic only
- Follow **Next reads** first; ignore low-confidence broad hits

## Common Chains
- Code question: context("{task or question}") → answer, or node/explore one listed symbol if more source is needed
- Flow tracing: context("flow from X to Y") → trace only if the embedded call paths are insufficient
- Onboarding: context("understand {feature}") → explore related symbols
- Refactor planning: context("change {symbol}") → impact(symbol) if blast radius is needed
- Dead-code scan: uncalled(limit=20)
- Debugging: context("{bug symptom or compiler diagnostic}") → node/explore listed suspects

## Limitations
- Index has ~1s lag after file changes
- Cross-file resolution is best-effort

## After you change files
When you create, edit, or delete source files, refresh only those paths before the next grph query:
`grph index --file <path>`
Run that once per changed file. Use a full `grph index` only when many files changed or the project has no index yet.
"#;
