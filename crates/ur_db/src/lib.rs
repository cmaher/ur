mod schema;

pub mod backup;
pub mod query;
pub mod ticket;

pub use backup::BackupManager;
pub use query::{DispatchableTicket, MetadataMatchTicket, QueryManager};
pub use ticket::{
    ActivityEntry, CreateTicketParams, ListTicketFilters, MetadataEntry, Ticket, TicketDetail,
    UpdateTicketFields,
};

use cozo::{DbInstance, NamedRows, ScriptMutability};
use schema::RELATION_STATEMENTS;

/// Primary entry point for the CozoDB ticket database.
///
/// Holds a CozoDB instance (internally Arc'd, so Clone is cheap) and ensures
/// all six relations exist on startup. ur-server injects this at startup and
/// passes it to managers that need database access.
#[derive(Clone)]
pub struct DatabaseManager {
    db: DbInstance,
}

impl DatabaseManager {
    /// Create a DatabaseManager backed by an SQLite file on disk.
    ///
    /// Creates all relations on first startup. Safe to call on an existing
    /// database -- CozoDB's `:create` is a no-op if the relation already exists.
    pub fn create_with_sqlite(path: &std::path::Path) -> Result<Self, String> {
        let path_str = path.to_str().ok_or("Invalid UTF-8 in database path")?;
        let db = DbInstance::new("sqlite", path_str, "").map_err(|e| e.to_string())?;
        let manager = Self { db };
        manager.ensure_schema()?;
        Ok(manager)
    }

    /// Create a DatabaseManager with an in-memory CozoDB instance.
    ///
    /// Useful for testing. All data is lost when the instance is dropped.
    pub fn create_in_memory() -> Result<Self, String> {
        let db = DbInstance::new("mem", "", "").map_err(|e| e.to_string())?;
        let manager = Self { db };
        manager.ensure_schema()?;
        Ok(manager)
    }

    /// Open an existing CozoDB SQLite database without creating relations.
    ///
    /// Use this to open a backup or previously-created database file where
    /// relations already exist (e.g., after a restore).
    pub fn open_sqlite(path: &std::path::Path) -> Result<Self, String> {
        let path_str = path.to_str().ok_or("Invalid UTF-8 in database path")?;
        let db = DbInstance::new("sqlite", path_str, "").map_err(|e| e.to_string())?;
        Ok(Self { db })
    }

    /// Wrap a raw CozoDB instance without creating relations.
    ///
    /// The caller is responsible for ensuring the database has the expected schema.
    pub fn from_raw(db: DbInstance) -> Self {
        Self { db }
    }

    /// Access the underlying CozoDB database instance.
    pub fn db(&self) -> &DbInstance {
        &self.db
    }

    /// Run a CozoScript query and return the result.
    pub fn run(&self, script: &str) -> Result<NamedRows, String> {
        self.db
            .run_script(script, Default::default(), ScriptMutability::Mutable)
            .map_err(|e| e.to_string())
    }

    /// Create all six relations (ticket, ticket_meta, blocks, relates_to,
    /// activity, activity_meta). Uses `:create` which is a no-op if the
    /// relation already exists with the same schema.
    fn ensure_schema(&self) -> Result<(), String> {
        for stmt in RELATION_STATEMENTS {
            self.db
                .run_script(stmt, Default::default(), ScriptMutability::Mutable)
                .map_err(|e| format!("Failed to create relation: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod ticket_tests;
