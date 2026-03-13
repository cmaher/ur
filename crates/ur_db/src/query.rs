use crate::dependency::DispatchableTicket;
use crate::DatabaseManager;

/// Manages Datalog queries against the CozoDB ticket database.
///
/// Provides structured query methods for dispatch, DAG traversal, epic rollup,
/// cycle detection, and metadata filtering.
#[derive(Clone)]
pub struct QueryManager {
    db: DatabaseManager,
}

impl QueryManager {
    /// Create a new QueryManager wrapping the given DatabaseManager.
    pub fn new(db: DatabaseManager) -> Self {
        Self { db }
    }

    /// Access the underlying DatabaseManager.
    pub fn db(&self) -> &DatabaseManager {
        &self.db
    }

    /// Find dispatchable tickets under a given epic: children with dispatchable type
    /// (task, bug), status=open, and no incoming `blocks` edges from open tickets.
    pub fn dispatchable_tickets(&self, epic_id: &str) -> Result<Vec<DispatchableTicket>, String> {
        let script = format!(
            r#"
            # Rule: ticket IDs that are blocked by at least one open ticket
            blocked_by_open[blocked_id] :=
                *blocks{{blocker_id, blocked_id}},
                *ticket{{id: blocker_id, status}},
                status != "closed"

            # Main query: children of epic, dispatchable type, open, not blocked
            ?[id, title, priority] :=
                *ticket{{id, type, status, priority, parent_id, title}},
                parent_id = "{epic_id}",
                status = "open",
                type in ["task", "bug"],
                not blocked_by_open[id]

            :order id
            "#
        );
        let result = self.db.run(&script)?;
        Ok(result
            .rows
            .iter()
            .map(|r| DispatchableTicket {
                id: r[0].get_str().unwrap().to_string(),
                title: r[1].get_str().unwrap().to_string(),
                priority: r[2].get_int().unwrap(),
            })
            .collect())
    }

    /// Compute the transitive closure of tickets that transitively block a given ticket.
    /// Returns all tickets that must be completed before the target can start.
    pub fn transitive_blockers(&self, ticket_id: &str) -> Result<Vec<String>, String> {
        let script = format!(
            r#"
            # Recursive transitive closure of the blocks relation (upward: who blocks me?)
            trans_blocker[ancestor] :=
                *blocks{{blocker_id: ancestor, blocked_id: "{ticket_id}"}}
            trans_blocker[ancestor] :=
                *blocks{{blocker_id: ancestor, blocked_id: mid}},
                trans_blocker[mid]

            ?[id] := trans_blocker[id]
            :order id
            "#
        );
        let result = self.db.run(&script)?;
        Ok(result
            .rows
            .iter()
            .map(|r| r[0].get_str().unwrap().to_string())
            .collect())
    }

    /// Compute the transitive closure of tickets that a given ticket transitively blocks.
    /// Returns all tickets that are downstream dependents.
    pub fn transitive_dependents(&self, ticket_id: &str) -> Result<Vec<String>, String> {
        let script = format!(
            r#"
            trans_dependent[descendant] :=
                *blocks{{blocker_id: "{ticket_id}", blocked_id: descendant}}
            trans_dependent[descendant] :=
                *blocks{{blocker_id: mid, blocked_id: descendant}},
                trans_dependent[mid]

            ?[id] := trans_dependent[id]
            :order id
            "#
        );
        let result = self.db.run(&script)?;
        Ok(result
            .rows
            .iter()
            .map(|r| r[0].get_str().unwrap().to_string())
            .collect())
    }

    /// Check if all children of an epic are closed.
    pub fn epic_all_children_closed(&self, epic_id: &str) -> Result<bool, String> {
        let script = format!(
            r#"
            ?[id] :=
                *ticket{{id, parent_id, status}},
                parent_id = "{epic_id}",
                status != "closed"
            "#
        );
        let result = self.db.run(&script)?;
        Ok(result.rows.is_empty())
    }

    /// Detect whether adding a blocks edge from `blocker_id` to `blocked_id` would
    /// create a cycle in the dependency DAG.
    ///
    /// Returns true if a cycle would be created (i.e., the edge should be rejected).
    pub fn would_create_cycle(&self, blocker_id: &str, blocked_id: &str) -> Result<bool, String> {
        if blocker_id == blocked_id {
            return Ok(true);
        }

        let script = format!(
            r#"
            # Can blocked_id reach blocker_id through existing edges?
            # If yes, adding blocker_id -> blocked_id would create a cycle.
            reachable[node] :=
                *blocks{{blocker_id: "{blocked_id}", blocked_id: node}}
            reachable[node] :=
                *blocks{{blocker_id: mid, blocked_id: node}},
                reachable[mid]

            ?[found] := reachable["{blocker_id}"], found = true
            ?[found] := not reachable["{blocker_id}"], found = false
            "#
        );
        let result = self.db.run(&script)?;
        let found = result.rows[0][0].get_bool().unwrap();
        Ok(found)
    }

    /// Find all tickets matching a specific metadata key-value pair.
    pub fn tickets_by_metadata(
        &self,
        key: &str,
        value: &str,
    ) -> Result<Vec<MetadataMatchTicket>, String> {
        let script = format!(
            r#"
            ?[id, title, type, status] :=
                *ticket_meta{{ticket_id, key, value}},
                key = "{key}",
                value = "{value}",
                *ticket{{id, title, type, status}},
                id = ticket_id
            :order id
            "#
        );
        let result = self.db.run(&script)?;
        Ok(result
            .rows
            .iter()
            .map(|r| MetadataMatchTicket {
                id: r[0].get_str().unwrap().to_string(),
                title: r[1].get_str().unwrap().to_string(),
                ticket_type: r[2].get_str().unwrap().to_string(),
                status: r[3].get_str().unwrap().to_string(),
            })
            .collect())
    }

    /// Find all tickets that have a specific metadata key (any value).
    pub fn tickets_with_metadata_key(&self, key: &str) -> Result<Vec<MetadataMatchTicket>, String> {
        let script = format!(
            r#"
            ?[id, title, type, status] :=
                *ticket_meta{{ticket_id, key}},
                key = "{key}",
                *ticket{{id, title, type, status}},
                id = ticket_id
            :order id
            "#
        );
        let result = self.db.run(&script)?;
        Ok(result
            .rows
            .iter()
            .map(|r| MetadataMatchTicket {
                id: r[0].get_str().unwrap().to_string(),
                title: r[1].get_str().unwrap().to_string(),
                ticket_type: r[2].get_str().unwrap().to_string(),
                status: r[3].get_str().unwrap().to_string(),
            })
            .collect())
    }
}

/// A ticket matched by a metadata query.
#[derive(Debug, Clone)]
pub struct MetadataMatchTicket {
    pub id: String,
    pub title: String,
    pub ticket_type: String,
    pub status: String,
}
