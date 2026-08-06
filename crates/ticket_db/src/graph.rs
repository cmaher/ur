// GraphManager: dependency graph operations using petgraph.

use std::collections::{HashMap, HashSet};

use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::Dfs;
use sqlx::PgPool;

#[derive(Clone)]
pub struct GraphManager {
    pool: PgPool,
}

struct BuiltGraph {
    graph: DiGraph<String, ()>,
    node_map: HashMap<String, NodeIndex>,
}

impl GraphManager {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Returns all transitive blockers of the given ticket (upstream DFS over
    /// reverse "blocks" edges). Results are returned regardless of ticket status.
    pub async fn transitive_blockers(&self, ticket_id: &str) -> Result<Vec<String>, sqlx::Error> {
        let built = self.build_graph().await?;
        let Some(&start) = built.node_map.get(ticket_id) else {
            return Ok(vec![]);
        };

        // Blockers are upstream: if A blocks B, the edge in the graph is A -> B.
        // To find what blocks `ticket_id`, we traverse incoming edges (reverse graph).
        let reversed = petgraph::visit::Reversed(&built.graph);
        let mut dfs = Dfs::new(reversed, start);
        let mut result = Vec::new();

        // Skip the start node itself.
        dfs.next(reversed);

        while let Some(node) = dfs.next(reversed) {
            result.push(built.graph[node].clone());
        }

        Ok(result)
    }

    /// Returns all transitive dependents of the given ticket (downstream DFS
    /// over "blocks" edges). Results are returned regardless of ticket status.
    pub async fn transitive_dependents(&self, ticket_id: &str) -> Result<Vec<String>, sqlx::Error> {
        let built = self.build_graph().await?;
        let Some(&start) = built.node_map.get(ticket_id) else {
            return Ok(vec![]);
        };

        let mut dfs = Dfs::new(&built.graph, start);
        let mut result = Vec::new();

        // Skip the start node itself.
        dfs.next(&built.graph);

        while let Some(node) = dfs.next(&built.graph) {
            result.push(built.graph[node].clone());
        }

        Ok(result)
    }

    /// Returns true if adding a "blocks" edge from `source` to `target` would
    /// create a cycle in the dependency graph.
    pub async fn would_create_cycle(
        &self,
        source: &str,
        target: &str,
    ) -> Result<bool, sqlx::Error> {
        if source == target {
            return Ok(true);
        }

        let built = self.build_graph().await?;

        let (Some(&target_idx), Some(&source_idx)) =
            (built.node_map.get(target), built.node_map.get(source))
        else {
            // If either node doesn't exist, no cycle is possible.
            return Ok(false);
        };

        // Adding source -> target creates a cycle iff there is already a path
        // from target to source (i.e., source is reachable from target).
        Ok(petgraph::algo::has_path_connecting(
            &built.graph,
            target_idx,
            source_idx,
            None,
        ))
    }

    /// Returns the subset of `ids` that have at least one transitive blocker
    /// which is not closed.
    ///
    /// Performs exactly one `build_graph()` and one open-status query regardless
    /// of how many ids are passed. Returns an empty set for empty input without
    /// hitting the DB.
    pub async fn blocked_among(&self, ids: &[String]) -> Result<HashSet<String>, sqlx::Error> {
        if ids.is_empty() {
            return Ok(HashSet::new());
        }
        let built = self.build_graph().await?;
        let open: HashSet<String> =
            sqlx::query_scalar::<_, String>("SELECT id FROM ticket WHERE status != 'closed'")
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .collect();
        Ok(compute_blocked(&built, &open, ids))
    }

    async fn build_graph(&self) -> Result<BuiltGraph, sqlx::Error> {
        let edges = sqlx::query_as::<_, (String, String)>(
            "SELECT source_id, target_id FROM edge WHERE kind = 'blocks'",
        )
        .fetch_all(&self.pool)
        .await?;

        let ticket_ids = sqlx::query_scalar::<_, String>("SELECT id FROM ticket")
            .fetch_all(&self.pool)
            .await?;

        let mut graph = DiGraph::<String, ()>::new();
        let mut node_map = HashMap::new();

        for id in ticket_ids {
            let idx = graph.add_node(id.clone());
            node_map.insert(id, idx);
        }

        for (source_id, target_id) in &edges {
            if let (Some(&src), Some(&tgt)) = (node_map.get(source_id), node_map.get(target_id)) {
                graph.add_edge(src, tgt, ());
            }
        }

        Ok(BuiltGraph { graph, node_map })
    }
}

/// For each id in `ids`, reverse-DFS the blocks graph and report whether any
/// transitive blocker is in `open`. Ids absent from the graph are never blocked.
fn compute_blocked(built: &BuiltGraph, open: &HashSet<String>, ids: &[String]) -> HashSet<String> {
    let mut blocked = HashSet::new();
    for id in ids {
        let Some(&start) = built.node_map.get(id.as_str()) else {
            continue;
        };
        let reversed = petgraph::visit::Reversed(&built.graph);
        let mut dfs = Dfs::new(reversed, start);
        // Skip the start node itself.
        dfs.next(reversed);
        while let Some(node) = dfs.next(reversed) {
            let blocker_id = &built.graph[node];
            if open.contains(blocker_id.as_str()) {
                blocked.insert(id.clone());
                break;
            }
        }
    }
    blocked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_test_graph(ticket_ids: &[&str], edges: &[(&str, &str)]) -> BuiltGraph {
        let mut graph = DiGraph::<String, ()>::new();
        let mut node_map = HashMap::new();
        for &id in ticket_ids {
            let idx = graph.add_node(id.to_owned());
            node_map.insert(id.to_owned(), idx);
        }
        for &(src, tgt) in edges {
            if let (Some(&s), Some(&t)) = (node_map.get(src), node_map.get(tgt)) {
                graph.add_edge(s, t, ());
            }
        }
        BuiltGraph { graph, node_map }
    }

    fn open_set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn compute_blocked_no_blockers() {
        let built = build_test_graph(&["a", "b"], &[]);
        let open = open_set(&["a", "b"]);
        let result = compute_blocked(&built, &open, &["b".to_owned()]);
        assert!(result.is_empty());
    }

    #[test]
    fn compute_blocked_direct_open_blocker() {
        // a blocks b
        let built = build_test_graph(&["a", "b"], &[("a", "b")]);
        let open = open_set(&["a", "b"]);
        let result = compute_blocked(&built, &open, &["b".to_owned()]);
        assert!(result.contains("b"));
    }

    #[test]
    fn compute_blocked_direct_closed_blocker() {
        // a blocks b, but a is closed
        let built = build_test_graph(&["a", "b"], &[("a", "b")]);
        let open = open_set(&["b"]); // a not in open set → closed
        let result = compute_blocked(&built, &open, &["b".to_owned()]);
        assert!(result.is_empty());
    }

    #[test]
    fn compute_blocked_transitive_chain_closed_intermediate() {
        // a blocks b blocks c; b is closed, a is open → c is still blocked by a
        let built = build_test_graph(&["a", "b", "c"], &[("a", "b"), ("b", "c")]);
        let open = open_set(&["a", "c"]); // b is closed
        let result = compute_blocked(&built, &open, &["c".to_owned()]);
        assert!(
            result.contains("c"),
            "c should be blocked by open a transitively"
        );
    }

    #[test]
    fn compute_blocked_id_not_in_graph() {
        let built = build_test_graph(&["a"], &[]);
        let open = open_set(&["a"]);
        let result = compute_blocked(&built, &open, &["nonexistent".to_owned()]);
        assert!(result.is_empty());
    }

    #[test]
    fn compute_blocked_empty_ids() {
        let built = build_test_graph(&["a"], &[]);
        let open = open_set(&["a"]);
        let result = compute_blocked(&built, &open, &[]);
        assert!(result.is_empty());
    }
}
