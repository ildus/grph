/// Server instructions for MCP agents
pub const SERVER_INSTRUCTIONS: &str = r#"Grph is a semantic code intelligence tool. Use it to understand and query codebases.

## Answering Directly
Use 2-3 codegraph calls maximum. Don't delegate to grep/read for code questions.

## Tool Selection
- **grph_search**: Quick symbol search by name
- **grph_context**: Comprehensive task context (search + callers + callees)
- **grph_trace**: Find call path between two symbols
- **grph_callers**: Who calls this symbol?
- **grph_uncalled**: Functions with no callers
- **grph_callees**: What does this symbol call?
- **grph_impact**: Impact radius of changing a symbol
- **grph_node**: Details about a specific symbol
- **grph_explore**: Source code for multiple related symbols
- **grph_files**: File structure from the index
- **grph_status**: Index statistics

## Common Chains
- Flow tracing: search → callers → callees → trace
- Onboarding: context("understand {feature}") → explore related symbols
- Refactor planning: impact(symbol) → callers → callees
- Dead-code scan: uncalled(limit=20)
- Debugging: search(bug_keyword) → context → trace

## Limitations
- Index has ~1s lag after file changes
- Cross-file resolution is best-effort
"#;
