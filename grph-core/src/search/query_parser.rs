use crate::types::NodeKind;

#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub terms: Vec<String>,
    pub kind_filter: Option<NodeKind>,
    pub file_filter: Option<String>,
}

impl SearchQuery {
    /// Parse a search query string
    pub fn parse(input: &str) -> Self {
        let mut terms = Vec::new();
        let mut kind_filter = None;
        let mut file_filter = None;

        for token in input.split_whitespace() {
            // Check for kind: filter
            if token.starts_with("kind:") {
                if let Ok(kind) = token[5..].parse::<NodeKind>() {
                    kind_filter = Some(kind);
                }
            }
            // Check for file: filter
            else if token.starts_with("file:") {
                file_filter = Some(token[5..].to_string());
            }
            // Regular term
            else {
                terms.push(token.to_string());
            }
        }

        Self {
            terms,
            kind_filter,
            file_filter,
        }
    }

    /// Build an FTS5 query string from terms
    pub fn build_fts_query(&self) -> String {
        if self.terms.is_empty() {
            "*".to_string()
        } else {
            self.terms.join(" OR ")
        }
    }
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            terms: vec!["*".to_string()],
            kind_filter: None,
            file_filter: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple() {
        let query = SearchQuery::parse("find me something");
        assert_eq!(query.terms, vec!["find", "me", "something"]);
        assert!(query.kind_filter.is_none());
        assert!(query.file_filter.is_none());
    }

    #[test]
    fn test_parse_with_kind_filter() {
        let query = SearchQuery::parse("login kind:function");
        assert_eq!(query.terms, vec!["login"]);
        assert_eq!(query.kind_filter, Some(NodeKind::Function));
    }

    #[test]
    fn test_build_fts_query() {
        let query = SearchQuery {
            terms: vec!["login".to_string(), "auth".to_string()],
            ..Default::default()
        };
        assert_eq!(query.build_fts_query(), "login OR auth");
    }
}
