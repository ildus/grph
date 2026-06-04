/// Server instructions for MCP agents
pub const SERVER_INSTRUCTIONS: &str = r#"Grph is a semantic code intelligence tool. Use it to understand and query codebases.

## Answering Directly
Start with **grph_context** for code questions. Use 2-3 grph calls maximum; do not delegate to grep/read unless grph output is insufficient.

## Tool Selection
- **grph_context**: PRIMARY first call for code understanding, architecture, features, bugs, and “how does X work”; returns ranked entry points, related symbols/files, key code, and call-path hints
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

## Common Chains
- Code question: context("{task or question}") → answer, or node/explore one listed symbol if more source is needed
- Flow tracing: context("flow from X to Y") → trace only if the embedded call paths are insufficient
- Onboarding: context("understand {feature}") → explore related symbols
- Refactor planning: context("change {symbol}") → impact(symbol) if blast radius is needed
- Dead-code scan: uncalled(limit=20)
- Debugging: context("{bug symptom}") → node/explore listed suspects

## Limitations
- Index has ~1s lag after file changes
- Cross-file resolution is best-effort
"#;
