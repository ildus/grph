use crate::errors::Result;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct CompileDatabase {
    pub path: PathBuf,
    units_by_file: HashMap<String, CompileUnit>,
    files_in_build: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct CompileUnit {
    pub file_path: String,
    pub directory: PathBuf,
    pub include_dirs: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct RawCompileCommand {
    directory: String,
    file: String,
    command: Option<String>,
    arguments: Option<Vec<String>>,
}

impl CompileDatabase {
    pub fn load(project_root: &Path, compile_commands_path: &Path) -> Result<Self> {
        let full_path = if compile_commands_path.is_absolute() {
            compile_commands_path.to_path_buf()
        } else {
            project_root.join(compile_commands_path)
        };
        let content = std::fs::read_to_string(&full_path)?;
        let raw: Vec<RawCompileCommand> = serde_json::from_str(&content)?;
        let mut units_by_file = HashMap::new();
        let mut files_in_build = HashSet::new();

        for item in raw {
            let directory = absolutize(project_root, Path::new(&item.directory));
            let file_abs = absolutize(&directory, Path::new(&item.file));
            let file_path = relative_to_root(project_root, &file_abs);
            let args = item.arguments.unwrap_or_else(|| {
                item.command
                    .as_deref()
                    .map(split_shell_words)
                    .unwrap_or_default()
            });
            let include_dirs = parse_include_dirs(project_root, &directory, &args);
            files_in_build.insert(file_path.clone());
            units_by_file.insert(
                file_path.clone(),
                CompileUnit {
                    file_path,
                    directory,
                    include_dirs,
                },
            );
        }

        Ok(Self {
            path: full_path,
            units_by_file,
            files_in_build,
        })
    }

    pub fn unit_for_file(&self, file_path: &str) -> Option<&CompileUnit> {
        self.units_by_file.get(file_path)
    }

    pub fn contains_file(&self, file_path: &str) -> bool {
        self.files_in_build.contains(file_path)
    }
}

pub fn detect_compile_commands(project_root: &Path) -> Option<PathBuf> {
    for candidate in ["compile_commands.json", "build/compile_commands.json"] {
        let path = project_root.join(candidate);
        if path.is_file() {
            return Some(PathBuf::from(candidate));
        }
    }
    None
}

fn parse_include_dirs(project_root: &Path, directory: &Path, args: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let mut value: Option<String> = None;
        if matches!(arg.as_str(), "-I" | "-isystem" | "-iquote" | "/I") {
            if let Some(next) = args.get(i + 1) {
                value = Some(next.clone());
                i += 1;
            }
        } else if let Some(rest) = arg.strip_prefix("-I") {
            if !rest.is_empty() {
                value = Some(rest.to_string());
            }
        } else if let Some(rest) = arg.strip_prefix("/I") {
            if !rest.is_empty() {
                value = Some(rest.to_string());
            }
        } else if let Some(rest) = arg.strip_prefix("-isystem") {
            if !rest.is_empty() {
                value = Some(rest.to_string());
            }
        }
        if let Some(v) = value {
            out.push(absolutize(directory, Path::new(&v)));
        }
        i += 1;
    }
    out.sort();
    out.dedup();
    out.into_iter()
        .map(|p| {
            if p.starts_with(project_root) {
                PathBuf::from(relative_to_root(project_root, &p))
            } else {
                p
            }
        })
        .collect()
}

fn split_shell_words(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in command.chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                cur.push(ch);
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn absolutize(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn relative_to_root(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
