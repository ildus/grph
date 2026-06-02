use crate::db::Database;
use crate::errors::Result;
use crate::graph::GraphTraverser;
use crate::types::{Language, Node};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Markdown,
    Json,
}

pub struct ContextBuilder {
    db: Database,
    traverser: GraphTraverser,
    project_root: Option<PathBuf>,
}

impl ContextBuilder {
    pub fn new(db: Database) -> Self {
        Self::new_with_root(db, None::<PathBuf>)
    }

    pub fn new_with_root(db: Database, project_root: Option<impl Into<PathBuf>>) -> Self {
        let traverser = GraphTraverser::new(db.clone());
        Self {
            db,
            traverser,
            project_root: project_root.map(Into::into),
        }
    }

    /// Build comprehensive context for an AI agent
    pub fn build_context(
        &self,
        task_description: &str,
        max_nodes: u32,
        include_code: bool,
        format: OutputFormat,
    ) -> Result<String> {
        // 1. Search for matching symbols. Task descriptions are free-form text,
        // while the current DB search is symbol-prefix based, so search both the
        // raw task and extracted identifier-like keywords.
        let nodes = self.search_task_symbols(task_description, max_nodes)?;

        if nodes.is_empty() {
            return Ok("No relevant symbols found.".to_string());
        }

        // 2. For each match, get callers and callees
        let mut entry_points = Vec::new();
        let mut related_symbols = Vec::new();
        let mut code_blocks = Vec::new();

        for node in &nodes {
            entry_points.push(node.clone());

            // Get callers
            if let Ok(callers) = self.traverser.callers(&node.id, 5) {
                for (caller, _) in callers {
                    if !related_symbols.iter().any(|n: &Node| n.id == caller.id) {
                        related_symbols.push(caller);
                    }
                }
            }

            // Get callees
            if let Ok(callees) = self.traverser.callees(&node.id, 5) {
                for (callee, _) in callees {
                    if !related_symbols.iter().any(|n: &Node| n.id == callee.id) {
                        related_symbols.push(callee);
                    }
                }
            }

            // Collect code blocks if requested
            if include_code {
                code_blocks.push(self.build_code_block(node));
            }
        }

        // 3. Format as markdown
        if format == OutputFormat::Markdown {
            Ok(self.format_markdown(
                task_description,
                &entry_points,
                &related_symbols,
                &code_blocks,
            ))
        } else {
            self.format_output(&nodes, format)
        }
    }

    fn search_task_symbols(&self, task_description: &str, max_nodes: u32) -> Result<Vec<Node>> {
        let mut terms = Vec::new();
        terms.push(task_description.to_string());

        for token in task_description
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .map(str::trim)
            .filter(|t| t.len() >= 2)
        {
            let lower = token.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "the"
                    | "and"
                    | "for"
                    | "with"
                    | "from"
                    | "into"
                    | "как"
                    | "что"
                    | "где"
                    | "для"
                    | "при"
                    | "над"
                    | "про"
                    | "fix"
                    | "add"
                    | "change"
                    | "update"
                    | "implement"
            ) {
                continue;
            }
            if !terms.iter().any(|t| t == token) {
                terms.push(token.to_string());
            }
        }

        let mut nodes = Vec::new();
        for term in terms {
            if nodes.len() >= max_nodes as usize {
                break;
            }
            let remaining = max_nodes.saturating_sub(nodes.len() as u32);
            for node in self.db.search_nodes(&term, None, remaining)? {
                if !nodes.iter().any(|existing: &Node| existing.id == node.id) {
                    nodes.push(node);
                }
            }
        }
        Ok(nodes)
    }

    fn format_markdown(
        &self,
        task: &str,
        entry_points: &[Node],
        related: &[Node],
        code_blocks: &Vec<String>,
    ) -> String {
        let mut md = String::new();

        md.push_str(&format!("## Grph Context: \"{}\"\n\n", task));

        // Entry Points
        if !entry_points.is_empty() {
            md.push_str("### Entry Points\n\n");
            for node in entry_points {
                md.push_str(&format!(
                    "- **{}** ({})\n",
                    node.name,
                    format_file_and_line(&node.file_path, node.start_line)
                ));
                if let Some(sig) = &node.signature {
                    md.push_str(&format!("  ```\n  {}\n  ```\n\n", sig));
                } else {
                    md.push('\n');
                }
            }
        }

        // Related Symbols
        if !related.is_empty() {
            md.push_str("### Related Symbols\n\n");
            for node in related {
                md.push_str(&format!(
                    "- **{}** ({})\n",
                    node.name,
                    format_file_and_line(&node.file_path, node.start_line)
                ));
            }
            md.push('\n');
        }

        // Code Blocks
        if !code_blocks.is_empty() {
            md.push_str("### Code\n\n");
            for (_i, block) in code_blocks.iter().enumerate() {
                md.push_str(&format!("{}\n\n", block));
            }
        }

        md
    }

    fn format_output(&self, nodes: &[Node], format: OutputFormat) -> Result<String> {
        match format {
            OutputFormat::Json => Ok(serde_json::to_string_pretty(nodes)?),
            OutputFormat::Markdown => {
                let mut md = String::new();
                for node in nodes {
                    md.push_str(&format!(
                        "- **{}** ({}, line {})\n",
                        node.name, node.file_path, node.start_line
                    ));
                }
                Ok(md)
            }
        }
    }

    fn build_code_block(&self, node: &Node) -> String {
        match self.read_source_slice(node) {
            Ok((start_line, source)) if !source.trim().is_empty() => format!(
                "#### {} ({}:{})

```{}
{}
```
",
                node.name,
                node.file_path,
                start_line,
                node.language.as_str(),
                source
            ),
            _ => format!(
                "#### {} ({}:{})

```{}
{}
```
",
                node.name,
                node.file_path,
                node.start_line,
                node.language.as_str(),
                node.signature.as_deref().unwrap_or("<source unavailable>")
            ),
        }
    }

    fn read_source_slice(&self, node: &Node) -> Result<(u32, String)> {
        let path = self.resolve_source_path(&node.file_path);
        let content = std::fs::read_to_string(&path)?;
        let lines: Vec<&str> = content.lines().collect();
        if lines.is_empty() {
            return Ok((1, String::new()));
        }

        let start = node.start_line.max(1);
        let end = infer_block_end(&lines, node.start_line, node.language).min(lines.len() as u32);

        let slice = lines[(start - 1) as usize..end as usize].join(
            "
",
        );
        Ok((start, slice))
    }

    fn resolve_source_path(&self, file_path: &str) -> PathBuf {
        let path = Path::new(file_path);
        if path.is_absolute() {
            path.to_path_buf()
        } else if let Some(root) = &self.project_root {
            root.join(path)
        } else {
            path.to_path_buf()
        }
    }
}

fn infer_block_end(lines: &[&str], start_line: u32, language: Language) -> u32 {
    let start_idx = start_line.saturating_sub(1) as usize;
    if start_idx >= lines.len() {
        return start_line;
    }

    if language == Language::Python {
        let base_indent = indentation(lines[start_idx]);
        let mut last = start_idx;
        for (idx, line) in lines.iter().enumerate().skip(start_idx + 1) {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                last = idx;
                continue;
            }
            if indentation(line) <= base_indent {
                break;
            }
            last = idx;
        }
        return (last + 1) as u32;
    }

    let mut brace_depth: i32 = 0;
    let mut saw_open = false;
    for (idx, line) in lines.iter().enumerate().skip(start_idx) {
        for ch in line.chars() {
            match ch {
                '{' => {
                    brace_depth += 1;
                    saw_open = true;
                }
                '}' => brace_depth -= 1,
                _ => {}
            }
        }
        if saw_open && brace_depth <= 0 {
            return (idx + 1) as u32;
        }
        if !saw_open && idx > start_idx && line.trim_end().ends_with(';') {
            return (idx + 1) as u32;
        }
    }

    start_line.min(lines.len() as u32)
}

fn indentation(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).count()
}

fn format_file_and_line(file_path: &str, line: u32) -> String {
    format!("{}:{}", file_path, line)
}
