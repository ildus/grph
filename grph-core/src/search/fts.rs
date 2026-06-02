use crate::db::Database;
use crate::errors::Result;
use crate::search::query_parser::SearchQuery;
use crate::types::Node;

/// Perform FTS5 search with the given query
pub fn fts_search(db: &Database, query: &SearchQuery, limit: u32) -> Result<Vec<Node>> {
    let fts_query = query.build_fts_query();

    if let Some(kind) = &query.kind_filter {
        db.search_nodes(&fts_query, Some(*kind), limit)
    } else {
        db.search_nodes(&fts_query, None, limit)
    }
}
