use crate::db::Database;
use crate::errors::Result;
use crate::types::{Edge, EdgeKind, Node};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Outgoing,
    Incoming,
}

pub struct GraphTraverser {
    db: Database,
}

impl GraphTraverser {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// BFS from a node — used for impact analysis
    pub fn bfs(
        &self,
        start_id: &str,
        edge_kind: EdgeKind,
        direction: Direction,
        max_depth: u32,
    ) -> Result<Vec<BfsLevel>> {
        let mut visited = std::collections::HashSet::new();
        let mut current_level = vec![start_id.to_string()];
        visited.insert(start_id.to_string());

        let mut levels = Vec::new();

        for depth in 0..=max_depth {
            if current_level.is_empty() {
                break;
            }

            let mut next_level = Vec::new();
            let mut nodes_at_depth = Vec::new();

            for node_id in &current_level {
                if let Ok(Some(node)) = self.db.get_node_by_id(node_id) {
                    nodes_at_depth.push(node);
                }

                let edges: Vec<Edge> = if direction == Direction::Outgoing {
                    self.db
                        .get_edges_for_node(node_id)?
                        .into_iter()
                        .filter(|e| e.kind == edge_kind && e.source == *node_id)
                        .collect()
                } else {
                    self.db
                        .get_edges_for_node(node_id)?
                        .into_iter()
                        .filter(|e| e.kind == edge_kind && e.target == *node_id)
                        .collect()
                };

                for edge in edges {
                    let target: String = if direction == Direction::Outgoing {
                        edge.target.clone()
                    } else {
                        edge.source.clone()
                    };
                    let _ = &edge;

                    if !visited.contains(&target) {
                        visited.insert(target.clone());
                        next_level.push(target);
                    }
                }
            }

            levels.push(BfsLevel {
                depth,
                nodes: nodes_at_depth,
            });

            current_level = next_level;
        }

        Ok(levels)
    }

    /// Impact radius: all nodes affected within N hops
    pub fn impact_radius(&self, node_id: &str, depth: u32) -> Result<ImpactResult> {
        let mut all_nodes = Vec::new();
        let mut all_edges = Vec::new();
        let mut visited_nodes = std::collections::HashSet::new();
        let mut visited_edges = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();

        if let Some(root) = self.db.get_node_by_id(node_id)? {
            visited_nodes.insert(root.id.clone());
            all_nodes.push(root);
        }
        queue.push_back((node_id.to_string(), 0));

        while let Some((current_id, current_depth)) = queue.pop_front() {
            if current_depth >= depth {
                continue;
            }

            for (caller, edge) in self.callers(&current_id, 1_000)? {
                let edge_key = format!("{}->{}:{}", edge.source, edge.target, edge.kind.as_str());
                if visited_edges.insert(edge_key) {
                    all_edges.push(edge);
                }

                if visited_nodes.insert(caller.id.clone()) {
                    queue.push_back((caller.id.clone(), current_depth + 1));
                    all_nodes.push(caller);
                }
            }
        }

        Ok(ImpactResult {
            root_id: node_id.to_string(),
            depth,
            nodes: all_nodes,
            edges: all_edges,
        })
    }

    /// Callers: who calls this node?
    pub fn callers(&self, node_id: &str, limit: u32) -> Result<Vec<(Node, Edge)>> {
        self.db.find_callers(node_id, limit)
    }

    /// References: what indexed symbols reference this node?
    pub fn references_to(&self, node_id: &str, limit: u32) -> Result<Vec<(Node, Edge)>> {
        self.db.find_references_to(node_id, limit)
    }

    /// Callees: what does this node call?
    pub fn callees(&self, node_id: &str, limit: u32) -> Result<Vec<(Node, Edge)>> {
        self.db.find_callees(node_id, limit)
    }

    /// Find shortest path between two nodes using bounded BFS.
    ///
    /// Regex extraction can leave call targets unresolved as symbol names rather
    /// than node IDs. Normalize every traversed endpoint to a node ID when
    /// possible so traversal stays finite and CLI output can resolve names.
    pub fn shortest_path(&self, from_id: &str, to_id: &str) -> Result<Option<Vec<PathHop>>> {
        if from_id == to_id {
            return Ok(Some(vec![PathHop {
                node_id: from_id.to_string(),
                edge: None,
            }]));
        }

        let max_depth = 32usize;
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();

        visited.insert(from_id.to_string());
        queue.push_back(vec![PathHop {
            node_id: from_id.to_string(),
            edge: None,
        }]);

        while let Some(path) = queue.pop_front() {
            if path.len() > max_depth + 1 {
                continue;
            }

            let current_id = match path.last() {
                Some(hop) => &hop.node_id,
                None => continue,
            };

            for edge in self
                .db
                .get_edges_for_node(current_id)?
                .into_iter()
                .filter(|e| e.kind == EdgeKind::Calls)
            {
                if edge.source != *current_id {
                    continue;
                }

                let next_id = self.resolve_endpoint(&edge.target)?;
                if !visited.insert(next_id.clone()) {
                    continue;
                }

                let mut next_path = path.clone();
                next_path.push(PathHop {
                    node_id: next_id.clone(),
                    edge: Some(edge),
                });

                if next_id == to_id {
                    return Ok(Some(next_path));
                }

                queue.push_back(next_path);
            }
        }

        Ok(None)
    }

    fn resolve_endpoint(&self, endpoint: &str) -> Result<String> {
        if self.db.get_node_by_id(endpoint)?.is_some() {
            return Ok(endpoint.to_string());
        }
        if let Some(node) = self.db.get_node_by_name_any(endpoint)? {
            return Ok(node.id);
        }
        Ok(endpoint.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct BfsLevel {
    pub depth: u32,
    pub nodes: Vec<Node>,
}

#[derive(Debug, Clone)]
pub struct ImpactResult {
    pub root_id: String,
    pub depth: u32,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone)]
pub struct PathHop {
    pub node_id: String,
    pub edge: Option<Edge>,
}
