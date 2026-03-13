use cozo::{DbInstance, ScriptMutability};

/// Manages the CozoDB database instance and schema lifecycle.
#[derive(Clone)]
pub struct SchemaManager {
    db: DbInstance,
}

impl SchemaManager {
    /// Create a new SchemaManager wrapping the given database instance.
    pub fn new(db: DbInstance) -> Self {
        Self { db }
    }

    /// Access the underlying database instance.
    pub fn db(&self) -> &DbInstance {
        &self.db
    }

    /// Create an in-memory CozoDB instance with all relations defined.
    pub fn create_in_memory() -> Result<Self, String> {
        let db = DbInstance::new("mem", "", "").map_err(|e| e.to_string())?;
        let manager = Self { db };
        manager.create_relations()?;
        Ok(manager)
    }

    /// Create a CozoDB instance backed by an SQLite file on disk with all relations defined.
    pub fn create_with_sqlite(path: &std::path::Path) -> Result<Self, String> {
        let path_str = path.to_str().ok_or("Invalid UTF-8 in database path")?;
        let db = DbInstance::new("sqlite", path_str, "").map_err(|e| e.to_string())?;
        let manager = Self { db };
        manager.create_relations()?;
        Ok(manager)
    }

    /// Open an existing CozoDB SQLite database without creating relations.
    /// Use this to open a backup or previously-created database file.
    pub fn open_sqlite(path: &std::path::Path) -> Result<Self, String> {
        let path_str = path.to_str().ok_or("Invalid UTF-8 in database path")?;
        let db = DbInstance::new("sqlite", path_str, "").map_err(|e| e.to_string())?;
        Ok(Self { db })
    }

    /// Define all six relations in the database.
    fn create_relations(&self) -> Result<(), String> {
        let statements = [
            // ticket: primary entity, keyed by id
            r#":create ticket {
                id: String
                =>
                type: String,
                status: String,
                priority: Int,
                parent_id: String,
                title: String,
                body: String,
                created_at: String,
                updated_at: String
            }"#,
            // ticket_meta: flexible key-value metadata per ticket
            r#":create ticket_meta {
                ticket_id: String,
                key: String
                =>
                value: String
            }"#,
            // blocks: hard dependency edges forming the dispatch DAG
            r#":create blocks {
                blocker_id: String,
                blocked_id: String
            }"#,
            // relates_to: soft informational links between tickets
            r#":create relates_to {
                left_id: String,
                right_id: String
            }"#,
            // activity: timestamped updates on tickets
            r#":create activity {
                id: String
                =>
                ticket_id: String,
                timestamp: String,
                author: String,
                message: String
            }"#,
            // activity_meta: flexible key-value metadata per activity
            r#":create activity_meta {
                activity_id: String,
                key: String
                =>
                value: String
            }"#,
        ];

        for stmt in &statements {
            self.db
                .run_script(stmt, Default::default(), ScriptMutability::Mutable)
                .map_err(|e| format!("Failed to create relation: {e}"))?;
        }

        Ok(())
    }

    /// Run a CozoScript query and return the result.
    pub fn run(&self, script: &str) -> Result<cozo::NamedRows, String> {
        self.db
            .run_script(script, Default::default(), ScriptMutability::Mutable)
            .map_err(|e| e.to_string())
    }
}
