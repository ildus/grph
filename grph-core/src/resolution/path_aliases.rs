/// Path aliases for TypeScript/JavaScript (tsconfig paths)
/// Not yet implemented in v0.1

#[allow(dead_code)]
pub struct AliasMap {
    aliases: Vec<(String, Vec<String>)>,
}

impl AliasMap {
    pub fn new() -> Self {
        Self {
            aliases: Vec::new(),
        }
    }

    pub fn load_from_tsconfig(
        _project_root: &std::path::Path,
    ) -> Result<Self, crate::errors::GrphError> {
        // Not implemented in v0.1
        Ok(Self::new())
    }

    pub fn resolve(&self, _import_path: &str) -> Option<Vec<std::path::PathBuf>> {
        None
    }
}
