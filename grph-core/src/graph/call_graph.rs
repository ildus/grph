use crate::db::Database;
use crate::errors::Result;
use crate::types::{Edge, Node};

impl Database {
    /// Get all callers of a node (incoming call edges)
    pub fn get_all_callers(&self, node_id: &str) -> Result<Vec<(Node, Edge)>> {
        self.find_callers(node_id, 1000)
    }

    /// Get all callees of a node (outgoing call edges)
    pub fn get_all_callees(&self, node_id: &str) -> Result<Vec<(Node, Edge)>> {
        self.find_callees(node_id, 1000)
    }

    /// Get the call graph for a set of nodes
    pub fn get_call_graph(&self, node_ids: &[String]) -> Result<Vec<(Node, Edge)>> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for id in node_ids {
            if let Ok(callees) = self.find_callees(id, 100) {
                for (node, edge) in callees {
                    let key = format!("{}->{}", id, node.id);
                    if seen.insert(key) {
                        result.push((node, edge));
                    }
                }
            }
        }

        Ok(result)
    }
}
