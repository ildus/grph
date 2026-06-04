use crate::compile_commands::CompileDatabase;
use crate::db::Database;
use crate::errors::Result;
use crate::resolution::builtins::is_builtin;
use crate::resolution::import_resolver::resolve_import;
use crate::types::{Language, Node, NodeKind, UnresolvedRefGroup};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub struct ReferenceResolver {
    db: Database,
    project_root: PathBuf,
    candidates_by_name: HashMap<String, Vec<Node>>,
    imports_by_file: HashMap<String, HashSet<String>>,
    header_index: Option<HashMap<String, Vec<String>>>,
    known_names: Option<HashSet<String>>,
    compile_db: Option<Option<CompileDatabase>>,
}

impl ReferenceResolver {
    pub fn new(db: Database, project_root: PathBuf) -> Self {
        Self {
            db,
            project_root,
            candidates_by_name: HashMap::new(),
            imports_by_file: HashMap::new(),
            header_index: None,
            known_names: None,
            compile_db: None,
        }
    }

    /// Resolve all unresolved references after extraction.
    ///
    /// For each unresolved reference, try to find a matching node:
    /// 1. Same-file: look for a node with matching name in the same file
    /// 2. Cross-file: search across all indexed files
    /// 3. If found, update the edge target to the resolved node ID
    /// 4. If not found, mark it attempted but keep it for diagnostics
    pub fn resolve_all(&mut self) -> Result<ResolutionResult> {
        self.resolve_all_with_limit(100_000)
    }

    /// Resolve up to `limit` unresolved reference groups across the project.
    /// A group can represent many individual refs with the same
    /// (name, kind, file, language), so this is much cheaper than row-by-row
    /// resolution on macro-heavy C/C++ projects.
    pub fn resolve_all_with_limit(&mut self, limit: u32) -> Result<ResolutionResult> {
        let groups = self.db.get_unresolved_ref_groups(limit)?;
        self.resolve_groups(&groups)
    }

    /// Resolve unresolved references emitted by a single file re-index.
    pub fn resolve_file(&mut self, file_path: &str) -> Result<ResolutionResult> {
        self.resolve_file_with_limit(file_path, 100_000)
    }

    /// Resolve up to `limit` unresolved reference groups for one file.
    pub fn resolve_file_with_limit(
        &mut self,
        file_path: &str,
        limit: u32,
    ) -> Result<ResolutionResult> {
        let groups = self
            .db
            .get_unresolved_ref_groups_for_file(file_path, limit)?;
        self.resolve_groups(&groups)
    }

    fn resolve_groups(&mut self, groups: &[UnresolvedRefGroup]) -> Result<ResolutionResult> {
        let total_groups = groups.len() as u64;
        let total_refs: u64 = groups.iter().map(|g| g.count).sum();
        let mut resolved = 0u64;
        let mut unresolved = 0u64;
        let mut resolved_groups = 0u64;
        let mut unresolved_groups = 0u64;

        self.db.conn().execute_batch("PRAGMA foreign_keys = OFF")?;

        for group in groups {
            let language = Language::from_str(&group.language).unwrap_or(Language::Python);
            let name = &group.reference_name;

            if is_builtin(name, language) {
                let deleted = self.db.delete_unresolved_ref_group(group)? as u64;
                resolved += deleted;
                resolved_groups += 1;
                continue;
            }

            match self.resolve_reference_candidate(
                name,
                &group.file_path,
                language,
                &group.reference_kind,
            )? {
                Some(node) => {
                    let metadata = serde_json::json!({
                        "resolvedBy": "cross-file-grouped",
                        "referenceCount": group.count,
                    });
                    let provenance = "tree-sitter+resolved-cross-file";
                    let changed = self
                        .db
                        .update_edges_for_unresolved_group(group, &node.id, provenance, &metadata)?
                        as u64;
                    let deleted = self.db.delete_unresolved_ref_group(group)? as u64;
                    // Count deleted unresolved rows as resolved. `changed` can be
                    // lower when duplicate unresolved rows point at the same edge.
                    resolved += deleted.max(changed);
                    resolved_groups += 1;
                }
                None => {
                    // Keep failed refs for diagnostics/debugging, but mark the
                    // group as attempted so normal resolver passes do not keep
                    // paying the same lookup/grouping cost for permanent
                    // external/generated/macro noise.
                    let reason = self.unresolved_reason(name, language)?;
                    let marked = self.db.mark_unresolved_ref_group_attempted(group, reason)? as u64;
                    unresolved += marked.max(group.count);
                    unresolved_groups += 1;
                }
            }
        }

        self.db.conn().execute_batch("PRAGMA foreign_keys = ON")?;

        Ok(ResolutionResult {
            resolved,
            unresolved,
            total: total_refs,
            resolved_groups,
            unresolved_groups,
            total_groups,
            remaining: self.db.count_unresolved_refs().unwrap_or(0),
        })
    }

    fn unresolved_reason(&mut self, name: &str, language: Language) -> Result<&'static str> {
        if is_builtin(name, language) {
            return Ok("external_builtin");
        }
        if !self.has_any_possible_match(name)? {
            return Ok("not_found");
        }
        if matches!(language, Language::Cpp | Language::C | Language::Esqlc)
            && (name.contains('.') || name.contains("::"))
        {
            return Ok("member_call_no_receiver_type");
        }
        if matches!(language, Language::Cpp)
            && name
                .chars()
                .next()
                .map(|c| c.is_ascii_uppercase())
                .unwrap_or(false)
        {
            return Ok("framework_or_member_call");
        }
        Ok("ambiguous_or_unresolved")
    }

    fn has_any_possible_match(&mut self, name: &str) -> Result<bool> {
        if self.known_names.is_none() {
            self.known_names = Some(self.db.get_all_node_names()?.into_iter().collect());
        }
        let Some(known) = self.known_names.as_ref() else {
            return Ok(true);
        };

        if known.contains(name) {
            return Ok(true);
        }

        // Qualified/member references often carry receiver syntax even though
        // indexed nodes store the member's simple name. Check common pieces so
        // obj.method, Class::method, and path-like references can still proceed
        // to normal resolution when a plausible symbol exists.
        for sep in ['.', ':', '/', '\\'] {
            for part in name.split(sep).filter(|part| !part.is_empty()) {
                if known.contains(part) {
                    return Ok(true);
                }
                if let Some(first) = part.chars().next() {
                    let mut capitalized = first.to_uppercase().to_string();
                    capitalized.push_str(&part[first.len_utf8()..]);
                    if known.contains(&capitalized) {
                        return Ok(true);
                    }
                }
            }
        }

        Ok(false)
    }

    fn resolve_reference_candidate(
        &mut self,
        name: &str,
        file_path: &str,
        language: Language,
        reference_kind: &str,
    ) -> Result<Option<Node>> {
        // Fast negative pre-filter. Large C/Esqlc projects produce many
        // references to external APIs, macros, or generated symbols that never
        // appear as indexed nodes. Keep a lightweight set of known symbol names
        // so those misses avoid same-file/cross-file DB lookups entirely.
        if !self.has_any_possible_match(name)? {
            return Ok(None);
        }

        if let Some(node) = self.db.get_node_by_name(name, file_path)? {
            return Ok(Some(node));
        }

        let candidates = if let Some(cached) = self.candidates_by_name.get(name) {
            cached.clone()
        } else {
            let candidates = self.db.get_nodes_by_name_all(name, 50)?;
            self.candidates_by_name
                .insert(name.to_string(), candidates.clone());
            candidates
        };
        if candidates.is_empty() {
            return Ok(None);
        }
        if candidates.len() == 1 {
            return Ok(candidates.into_iter().next());
        }

        // For C-family call edges, compile_commands.json describes the active
        // build/platform better than the textual include graph. Try it before
        // imported headers so a platform header macro/prototype does not steal
        // callers from the compiled implementation.
        let c_call_ref = is_c_family(language) && is_call_reference_kind(reference_kind);
        if c_call_ref {
            if let Some(node) = self.best_compile_db_candidate(&candidates, file_path)? {
                return Ok(Some(node));
            }
        }

        let imported_files = self.imported_files_for(file_path, language)?;
        if let Some(node) =
            best_imported_candidate(&candidates, &imported_files, language, reference_kind)
        {
            return Ok(Some(node));
        }

        if is_c_family(language) && !c_call_ref {
            if let Some(node) = self.best_compile_db_candidate(&candidates, file_path)? {
                return Ok(Some(node));
            }
        }

        // In large portable C codebases the same API often has one implementation
        // per platform (e.g. zq_unix_win/zq.c and zq_vms/zq.c). If no included
        // declaration disambiguates the symbol, prefer the platform branch that
        // matches this checkout instead of leaving every call unresolved.
        if is_c_family(language) {
            if let Some(node) = best_c_platform_candidate(&candidates, reference_kind) {
                return Ok(Some(node));
            }
        }

        let from_dir = Path::new(file_path)
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if let Some(node) = candidates
            .iter()
            .find(|node| !from_dir.is_empty() && node.file_path.starts_with(&from_dir))
        {
            return Ok(Some(node.clone()));
        }

        // Prefer high-information definitions/functions over imports/parameters
        // when ambiguity remains. If top score ties, leave unresolved rather than
        // creating a misleading edge.
        let mut ranked = candidates;
        ranked.sort_by(|a, b| resolution_kind_score(b.kind).cmp(&resolution_kind_score(a.kind)));
        if ranked.len() >= 2
            && resolution_kind_score(ranked[0].kind) == resolution_kind_score(ranked[1].kind)
        {
            Ok(None)
        } else {
            Ok(ranked.into_iter().next())
        }
    }

    fn compile_database(&mut self) -> Result<Option<&CompileDatabase>> {
        if self.compile_db.is_none() {
            let db = if let Some(path) = self
                .db
                .get_project_metadata("index.compile_commands.path")?
            {
                if path.is_empty() {
                    None
                } else {
                    CompileDatabase::load(&self.project_root, Path::new(&path)).ok()
                }
            } else {
                None
            };
            self.compile_db = Some(db);
        }
        Ok(self.compile_db.as_ref().and_then(|db| db.as_ref()))
    }

    fn best_compile_db_candidate(
        &mut self,
        candidates: &[Node],
        file_path: &str,
    ) -> Result<Option<Node>> {
        let Some(compile_db) = self.compile_database()? else {
            return Ok(None);
        };
        if let Some(node) = candidates
            .iter()
            .find(|node| compile_db.contains_file(&node.file_path))
        {
            return Ok(Some(node.clone()));
        }
        let Some(unit) = compile_db.unit_for_file(file_path) else {
            return Ok(None);
        };
        for include_dir in &unit.include_dirs {
            let prefix = include_dir.to_string_lossy().replace('\\', "/");
            if let Some(node) = candidates
                .iter()
                .find(|node| node.file_path.starts_with(&prefix))
            {
                return Ok(Some(node.clone()));
            }
        }
        Ok(None)
    }

    fn imported_files_for(
        &mut self,
        file_path: &str,
        language: Language,
    ) -> Result<HashSet<String>> {
        if let Some(cached) = self.imports_by_file.get(file_path) {
            return Ok(cached.clone());
        }
        let mut out = self.direct_imported_files_for(file_path, language)?;

        // C/C++ include graphs often route symbols through transitive headers:
        // source.c -> <er.h> -> <ercl.h> -> ERx. Follow a shallow include graph
        // so macros/constants in included headers can disambiguate correctly.
        if matches!(language, Language::C | Language::Cpp | Language::Esqlc) {
            let direct: Vec<String> = out.iter().cloned().collect();
            let mut visited = out.clone();
            for header in direct {
                self.collect_transitive_c_includes(&header, language, 0, &mut visited)?;
            }
            out.extend(visited);
        }

        self.imports_by_file
            .insert(file_path.to_string(), out.clone());
        Ok(out)
    }

    fn direct_imported_files_for(
        &mut self,
        file_path: &str,
        language: Language,
    ) -> Result<HashSet<String>> {
        let mut out = HashSet::new();
        for node in self.db.list_nodes_by_file(file_path)? {
            if node.kind != NodeKind::Import {
                continue;
            }
            let mut imports = vec![node.name.clone()];
            if let Some(sig) = node.signature.as_ref() {
                imports.push(sig.clone());
            }
            if let Some(doc) = node.docstring.as_ref() {
                imports.push(doc.clone());
            }
            for import_text in imports {
                for candidate in
                    import_to_candidate_files(&import_text, file_path, &self.project_root, language)
                {
                    out.insert(candidate);
                }
                if matches!(language, Language::C | Language::Cpp | Language::Esqlc) {
                    for include in extract_import_paths(&import_text, language) {
                        for candidate in self.compile_db_header_candidates(&include, file_path)? {
                            out.insert(candidate);
                        }
                        for candidate in self.c_header_candidates(&include)? {
                            out.insert(candidate);
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    fn collect_transitive_c_includes(
        &mut self,
        file_path: &str,
        language: Language,
        depth: u8,
        visited: &mut HashSet<String>,
    ) -> Result<()> {
        if depth >= 4 {
            return Ok(());
        }
        let imports = self.direct_imported_files_for(file_path, language)?;
        for include in imports {
            if visited.insert(include.clone()) {
                self.collect_transitive_c_includes(&include, language, depth + 1, visited)?;
            }
        }
        Ok(())
    }

    fn compile_db_header_candidates(
        &mut self,
        import_path: &str,
        including_file: &str,
    ) -> Result<Vec<String>> {
        let Some(compile_db) = self.compile_database()? else {
            return Ok(Vec::new());
        };
        let Some(unit) = compile_db.unit_for_file(including_file) else {
            return Ok(Vec::new());
        };
        let include = import_path
            .trim()
            .trim_matches(|c| matches!(c, '<' | '>' | '"') || c == char::from(39));
        let mut bases = Vec::new();
        if !import_path.trim_start().starts_with('<') {
            if let Some(parent) = Path::new(including_file).parent() {
                bases.push(parent.to_path_buf());
            }
        }
        bases.push(unit.directory.clone());
        bases.extend(unit.include_dirs.iter().cloned());
        let mut out = Vec::new();
        for base in bases {
            let joined = base.join(include);
            let candidate = if joined.is_absolute() {
                joined
                    .strip_prefix(&self.project_root)
                    .unwrap_or(&joined)
                    .to_string_lossy()
                    .replace('\\', "/")
            } else {
                joined.to_string_lossy().replace('\\', "/")
            };
            if self.db.get_file(&candidate)?.is_some() {
                out.push(candidate);
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    fn c_header_candidates(&mut self, import_path: &str) -> Result<Vec<String>> {
        let basename = Path::new(import_path.trim().trim_matches(['<', '>', '"', '\'']))
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(import_path)
            .to_string();
        if basename.is_empty() {
            return Ok(Vec::new());
        }
        if self.header_index.is_none() {
            let mut index: HashMap<String, Vec<String>> = HashMap::new();
            for file in self.db.list_files(None)? {
                if !matches!(file.language, Language::C | Language::Cpp | Language::Esqlc) {
                    continue;
                }
                let Some(name) = Path::new(&file.path).file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                index.entry(name.to_string()).or_default().push(file.path);
            }
            for files in index.values_mut() {
                files.sort_by_key(|path| c_header_preference(path));
            }
            self.header_index = Some(index);
        }
        Ok(self
            .header_index
            .as_ref()
            .and_then(|idx| idx.get(&basename).cloned())
            .unwrap_or_default())
    }

    /// Try to find a symbol by name across the codebase
    pub fn resolve_name(
        &self,
        name: &str,
        file_path: &str,
        language: Language,
    ) -> Option<Vec<String>> {
        if is_builtin(name, language) {
            return Some(Vec::new());
        }

        // Search for exact matches first
        if let Ok(Some(node)) = self.db.get_node_by_name(name, file_path) {
            return Some(vec![node.id]);
        }

        // Search across all files
        if let Ok(nodes) = self.db.search_nodes(name, None, 20) {
            let ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
            if !ids.is_empty() {
                return Some(ids);
            }
        }

        None
    }

    /// Follow an import statement to its source file
    pub fn resolve_import(
        &self,
        import_path: &str,
        from_file: &str,
        language: Language,
    ) -> Option<PathBuf> {
        resolve_import(import_path, from_file, &self.project_root, language)
    }
}

fn resolution_kind_score(kind: NodeKind) -> u8 {
    match kind {
        NodeKind::Function | NodeKind::Method => 9,
        NodeKind::Class
        | NodeKind::Struct
        | NodeKind::Interface
        | NodeKind::Trait
        | NodeKind::Protocol => 8,
        NodeKind::Component => 7,
        NodeKind::Variable | NodeKind::Constant | NodeKind::Property | NodeKind::Field => 5,
        NodeKind::Module | NodeKind::Namespace | NodeKind::Enum | NodeKind::TypeAlias => 4,
        NodeKind::Import | NodeKind::Export | NodeKind::Parameter => 1,
        _ => 2,
    }
}

fn best_imported_candidate(
    candidates: &[Node],
    imported_files: &HashSet<String>,
    language: Language,
    reference_kind: &str,
) -> Option<Node> {
    let mut matches: Vec<&Node> = candidates
        .iter()
        .filter(|node| imported_files.contains(&node.file_path))
        .collect();
    if matches.is_empty() {
        return None;
    }

    // For C call expressions, an included header declaration/macro is useful
    // evidence but is not necessarily the call target. If an implementation
    // definition with the same name exists elsewhere, let compile-db/platform
    // resolution choose it instead of eagerly returning the header node.
    if is_c_family(language) && is_call_reference_kind(reference_kind) {
        let implementation_exists = candidates.iter().any(is_c_implementation_definition);
        let implementation_matches: Vec<&Node> = matches
            .iter()
            .copied()
            .filter(|node| is_c_implementation_definition(node))
            .collect();
        if !implementation_matches.is_empty() {
            matches = implementation_matches;
        } else if implementation_exists {
            return None;
        }
    }

    matches.sort_by_key(|node| c_candidate_sort_key(node, reference_kind));

    if is_c_family(language) {
        return matches.first().cloned().cloned();
    }

    if matches.len() == 1 {
        matches.first().cloned().cloned()
    } else {
        None
    }
}

fn best_c_platform_candidate(candidates: &[Node], reference_kind: &str) -> Option<Node> {
    let mut ranked: Vec<&Node> = candidates
        .iter()
        .filter(|node| resolution_kind_score(node.kind) >= 5)
        .collect();
    if ranked.len() < 2 {
        return ranked.first().cloned().cloned();
    }
    ranked.sort_by_key(|node| c_candidate_sort_key(node, reference_kind));
    let best = ranked[0];
    let second = ranked[1];
    let best_key = c_candidate_disambiguation_key(best, reference_kind);
    let second_key = c_candidate_disambiguation_key(second, reference_kind);
    if best_key != second_key {
        Some(best.clone())
    } else {
        None
    }
}

fn c_candidate_sort_key(
    node: &Node,
    reference_kind: &str,
) -> (u8, u8, std::cmp::Reverse<u8>, String, u32) {
    (
        c_implementation_preference(node, reference_kind),
        c_header_preference(&node.file_path),
        std::cmp::Reverse(resolution_kind_score(node.kind)),
        node.file_path.clone(),
        node.start_line,
    )
}

fn c_candidate_disambiguation_key(node: &Node, reference_kind: &str) -> (u8, u8, u8) {
    (
        c_implementation_preference(node, reference_kind),
        c_header_preference(&node.file_path),
        resolution_kind_score(node.kind),
    )
}

fn c_implementation_preference(node: &Node, reference_kind: &str) -> u8 {
    if is_call_reference_kind(reference_kind) && is_c_implementation_definition(node) {
        0
    } else {
        1
    }
}

fn is_call_reference_kind(reference_kind: &str) -> bool {
    matches!(reference_kind, "calls" | "instantiates")
}

fn is_c_family(language: Language) -> bool {
    matches!(language, Language::C | Language::Cpp | Language::Esqlc)
}

fn is_c_implementation_definition(node: &Node) -> bool {
    matches!(node.kind, NodeKind::Function | NodeKind::Method)
        && !is_c_header_like_path(&node.file_path)
}

fn is_c_header_like_path(path: &str) -> bool {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    p.ends_with(".h")
        || p.ends_with(".hh")
        || p.ends_with(".hpp")
        || p.ends_with(".hxx")
        || p.contains("/hdr/")
        || p.contains("/hdr_")
        || p.contains("_hdr/")
}

fn c_header_preference(path: &str) -> u8 {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    if p.contains("/hdr_unix_win/") || p.contains("_unix_win/") || p.contains("/unix_win/") {
        0
    } else if p.contains("/hdr_unix/") || p.contains("_unix/") || p.contains("/unix/") {
        2
    } else if p.contains("/hdr_win/") || p.contains("_win/") || p.contains("/win/") {
        3
    } else if p.contains("/hdr_vms/") || p.contains("_vms/") || p.contains("/vms/") {
        8
    } else if p.contains("/hdr/") {
        1
    } else if p.contains("/erold") || p.contains("/old") {
        9
    } else {
        5
    }
}

fn import_to_candidate_files(
    import_text: &str,
    from_file: &str,
    project_root: &Path,
    language: Language,
) -> Vec<String> {
    let mut raw = extract_import_paths(import_text, language);
    raw.sort();
    raw.dedup();
    let mut out = Vec::new();
    for import_path in raw {
        let bases = import_path_bases(&import_path, from_file, language);
        for base in bases {
            for candidate in candidate_with_extensions(&base, language) {
                if project_root.join(&candidate).exists() {
                    out.push(candidate.replace('\\', "/"));
                }
            }
        }
    }
    out
}

fn extract_import_paths(import_text: &str, language: Language) -> Vec<String> {
    let text = import_text.trim();
    let mut out = Vec::new();
    match language {
        Language::JavaScript | Language::TypeScript | Language::Jsx | Language::Tsx => {
            if let Some(path) = quoted_tail(text) {
                out.push(path);
            }
        }
        Language::Python => {
            if let Some(rest) = text.strip_prefix("from ") {
                if let Some(module) = rest.split_whitespace().next() {
                    out.push(module.to_string());
                }
            } else if let Some(rest) = text.strip_prefix("import ") {
                out.extend(
                    rest.split(',')
                        .filter_map(|p| p.trim().split_whitespace().next().map(str::to_string)),
                );
            } else if !text.is_empty() {
                out.push(text.to_string());
            }
        }
        _ => {
            if !text.is_empty() {
                out.push(cleanup_c_like_import_path(text));
            }
        }
    }
    out
}

fn cleanup_c_like_import_path(import_text: &str) -> String {
    let mut text = import_text.trim().trim_end_matches(';').trim();
    if let Some(rest) = text.strip_prefix('#') {
        text = rest.trim_start();
    }
    if let Some(rest) = text.strip_prefix("include") {
        text = rest.trim_start();
    }
    text.trim_matches(|c| matches!(c, '<' | '>' | '"' | '\'' | ' ' | '\t'))
        .to_string()
}

fn import_path_bases(import_path: &str, from_file: &str, language: Language) -> Vec<String> {
    let from_dir = Path::new(from_file)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let normalized = import_path.trim().trim_matches(['\"', '\'']);
    let mut out = Vec::new();
    if normalized.starts_with('.') || normalized.starts_with('/') {
        out.push(
            from_dir
                .join(normalized)
                .to_string_lossy()
                .replace('\\', "/"),
        );
    } else if language == Language::Python {
        out.push(normalized.replace('.', "/"));
    } else {
        out.push(normalized.to_string());
    }
    out
}

fn candidate_with_extensions(base: &str, language: Language) -> Vec<String> {
    let mut out = vec![base.to_string()];
    match language {
        Language::JavaScript | Language::TypeScript | Language::Jsx | Language::Tsx => {
            for ext in ["ts", "tsx", "js", "jsx", "mjs", "cjs"] {
                out.push(format!("{base}.{ext}"));
            }
            for ext in ["ts", "tsx", "js", "jsx"] {
                out.push(format!("{base}/index.{ext}"));
            }
        }
        Language::Python => {
            out.push(format!("{base}.py"));
            out.push(format!("{base}/__init__.py"));
        }
        Language::Rust => {
            out.push(format!("{base}.rs"));
            out.push(format!("{base}/mod.rs"));
        }
        Language::C | Language::Cpp | Language::Esqlc => {
            for ext in ["h", "hpp", "c", "cpp"] {
                out.push(format!("{base}.{ext}"));
            }
        }
        _ => {}
    }
    out
}

fn quoted_tail(text: &str) -> Option<String> {
    let quote = if text.contains('"') { '"' } else { '\'' };
    let end = text.rfind(quote)?;
    let start = text[..end].rfind(quote)?;
    Some(text[start + 1..end].to_string())
}

#[derive(Debug, Clone)]
pub struct ResolutionResult {
    pub resolved: u64,
    pub unresolved: u64,
    pub total: u64,
    pub resolved_groups: u64,
    pub unresolved_groups: u64,
    pub total_groups: u64,
    pub remaining: u64,
}
