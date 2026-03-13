use crate::ticket::escape_cozo;
use crate::DatabaseManager;

/// A ticket that is dispatchable (ready for an agent to work on).
#[derive(Debug, Clone)]
pub struct DispatchableTicket {
    pub id: String,
    pub title: String,
    pub priority: i64,
}

impl DatabaseManager {
    /// Add a blocking dependency: `blocker_id` blocks `blocked_id`.
    ///
    /// Runs cycle detection before inserting. Returns an error if:
    /// - Either ticket does not exist
    /// - The edge would create a cycle in the dependency DAG
    /// - The blocker and blocked are the same ticket
    pub fn add_block(&self, blocker_id: &str, blocked_id: &str) -> Result<(), String> {
        if blocker_id == blocked_id {
            return Err(format!(
                "Cannot block a ticket on itself: {blocker_id}"
            ));
        }

        // Verify both tickets exist
        self.verify_ticket_exists(blocker_id)?;
        self.verify_ticket_exists(blocked_id)?;

        // Cycle detection: check if blocked_id can already reach blocker_id
        // through existing edges. If so, adding blocker_id -> blocked_id creates a cycle.
        if self.would_create_cycle(blocker_id, blocked_id)? {
            return Err(format!(
                "Adding block {blocker_id} -> {blocked_id} would create a cycle"
            ));
        }

        let script = format!(
            "?[blocker_id, blocked_id] <- [['{b}', '{d}']]
            :put blocks {{blocker_id, blocked_id}}",
            b = escape_cozo(blocker_id),
            d = escape_cozo(blocked_id),
        );
        self.run(&script)?;
        Ok(())
    }

    /// Remove a blocking dependency between two tickets.
    ///
    /// Silently succeeds if the edge did not exist. Returns an error if
    /// either ticket does not exist.
    pub fn remove_block(&self, blocker_id: &str, blocked_id: &str) -> Result<(), String> {
        self.verify_ticket_exists(blocker_id)?;
        self.verify_ticket_exists(blocked_id)?;

        let script = format!(
            "?[blocker_id, blocked_id] <- [['{b}', '{d}']]
            :rm blocks {{blocker_id, blocked_id}}",
            b = escape_cozo(blocker_id),
            d = escape_cozo(blocked_id),
        );
        self.run(&script)?;
        Ok(())
    }

    /// Add a soft informational link between two tickets.
    ///
    /// Links are non-directional conceptually but stored with left/right ordering.
    /// Returns an error if either ticket does not exist.
    pub fn add_link(&self, left_id: &str, right_id: &str) -> Result<(), String> {
        self.verify_ticket_exists(left_id)?;
        self.verify_ticket_exists(right_id)?;

        let script = format!(
            "?[left_id, right_id] <- [['{l}', '{r}']]
            :put relates_to {{left_id, right_id}}",
            l = escape_cozo(left_id),
            r = escape_cozo(right_id),
        );
        self.run(&script)?;
        Ok(())
    }

    /// Remove a soft informational link between two tickets.
    ///
    /// Silently succeeds if the link did not exist. Returns an error if
    /// either ticket does not exist.
    pub fn remove_link(&self, left_id: &str, right_id: &str) -> Result<(), String> {
        self.verify_ticket_exists(left_id)?;
        self.verify_ticket_exists(right_id)?;

        let script = format!(
            "?[left_id, right_id] <- [['{l}', '{r}']]
            :rm relates_to {{left_id, right_id}}",
            l = escape_cozo(left_id),
            r = escape_cozo(right_id),
        );
        self.run(&script)?;
        Ok(())
    }

    /// Find dispatchable tickets under a given epic: children with dispatchable type
    /// (task, bug), status=open, and no incoming `blocks` edges from open (non-closed) tickets.
    ///
    /// Parent-child relationships do NOT count as blocking -- only explicit `blocks` edges.
    pub fn dispatchable_tickets(
        &self,
        epic_id: &str,
    ) -> Result<Vec<DispatchableTicket>, String> {
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

            :order priority, id
            "#,
            epic_id = escape_cozo(epic_id),
        );
        let result = self.run(&script)?;
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

    /// Check if adding a blocks edge would create a cycle in the dependency DAG.
    ///
    /// Uses CozoDB's recursive rule support for transitive closure: checks whether
    /// `blocked_id` can already reach `blocker_id` through existing edges.
    fn would_create_cycle(&self, blocker_id: &str, blocked_id: &str) -> Result<bool, String> {
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
            "#,
            blocker_id = escape_cozo(blocker_id),
            blocked_id = escape_cozo(blocked_id),
        );
        let result = self.run(&script)?;
        let found = result.rows[0][0].get_bool().unwrap();
        Ok(found)
    }

    /// Verify that a ticket exists, returning an error if not found.
    fn verify_ticket_exists(&self, id: &str) -> Result<(), String> {
        let script = format!(
            "?[id] := *ticket{{id}}, id = '{eid}'",
            eid = escape_cozo(id)
        );
        let result = self.run(&script)?;
        if result.rows.is_empty() {
            return Err(format!("Ticket not found: {id}"));
        }
        Ok(())
    }
}
