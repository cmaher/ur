// GraphManager: dependency graph operations using petgraph.

use std::collections::HashMap;

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
