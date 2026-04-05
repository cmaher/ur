// UiEventRepo: read and delete operations for the ui_events ephemeral buffer.

use sqlx::PgPool;

use crate::model::UiEventRow;

#[derive(Clone)]
pub struct UiEventRepo {
    pool: PgPool,
}

impl UiEventRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Select all UI events ordered by id ascending.
    pub async fn poll_ui_events(&self) -> Result<Vec<UiEventRow>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (i64, String, String, String)>(
            "SELECT id, entity_type, entity_id, created_at FROM ui_events ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(id, entity_type, entity_id, created_at)| UiEventRow {
                id,
                entity_type,
                entity_id,
                created_at,
            })
            .collect())
    }

    /// Delete all UI events with id <= max_id.
    pub async fn delete_ui_events(&self, max_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM ui_events WHERE id <= ?")
            .bind(max_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}
