// KnowledgeRepo: CRUD operations for knowledge docs and tags.

use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::model::{Knowledge, KnowledgeFilter, KnowledgeSummary, KnowledgeUpdate, NewKnowledge};

const MAX_DESCRIPTION_LEN: usize = 120;

#[derive(Clone)]
pub struct KnowledgeRepo {
    pool: SqlitePool,
}

impl KnowledgeRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, input: &NewKnowledge) -> Result<Knowledge, sqlx::Error> {
        validate_description(&input.description)?;

        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO knowledge (id, project, title, description, body, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&input.project)
        .bind(&input.title)
        .bind(&input.description)
        .bind(&input.body)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        let tags = normalize_tags(&input.tags);
        insert_tags(&self.pool, &id, &tags).await?;

        Ok(Knowledge {
            id,
            project: input.project.clone(),
            title: input.title.clone(),
            description: input.description.clone(),
            body: input.body.clone(),
            tags,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub async fn get(&self, id: &str) -> Result<Option<Knowledge>, sqlx::Error> {
        let row = sqlx::query_as::<
            _,
            (
                String,
                Option<String>,
                String,
                String,
                String,
                String,
                String,
            ),
        >(
            "SELECT id, project, title, description, body, created_at, updated_at
             FROM knowledge WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        let Some((kid, project, title, description, body, created_at, updated_at)) = row else {
            return Ok(None);
        };

        let tags = fetch_tags(&self.pool, &kid).await?;

        Ok(Some(Knowledge {
            id: kid,
            project,
            title,
            description,
            body,
            tags,
            created_at,
            updated_at,
        }))
    }

    pub async fn update(
        &self,
        id: &str,
        update: &KnowledgeUpdate,
    ) -> Result<Knowledge, sqlx::Error> {
        let existing = self.get(id).await?.ok_or(sqlx::Error::RowNotFound)?;

        let title = update.title.as_deref().unwrap_or(&existing.title);
        let description = update
            .description
            .as_deref()
            .unwrap_or(&existing.description);
        validate_description(description)?;
        let body = update.body.as_deref().unwrap_or(&existing.body);
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "UPDATE knowledge SET title = ?, description = ?, body = ?, updated_at = ? WHERE id = ?",
        )
        .bind(title)
        .bind(description)
        .bind(body)
        .bind(&now)
        .bind(id)
        .execute(&self.pool)
        .await?;

        let tags = if let Some(ref new_tags) = update.tags {
            let normalized = normalize_tags(new_tags);
            replace_tags(&self.pool, id, &normalized).await?;
            normalized
        } else {
            existing.tags
        };

        Ok(Knowledge {
            id: existing.id,
            project: existing.project,
            title: title.to_owned(),
            description: description.to_owned(),
            body: body.to_owned(),
            tags,
            created_at: existing.created_at,
            updated_at: now,
        })
    }

    pub async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        // Tags are cascade-deleted via FK.
        let result = sqlx::query("DELETE FROM knowledge WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn list(
        &self,
        filter: &KnowledgeFilter,
    ) -> Result<Vec<KnowledgeSummary>, sqlx::Error> {
        let (query, binds) = build_list_query(filter);

        let mut q = sqlx::query_as::<_, (String, String, String)>(sqlx::AssertSqlSafe(query));
        for bind in &binds {
            q = q.bind(bind);
        }

        let rows = q.fetch_all(&self.pool).await?;

        let mut summaries = Vec::with_capacity(rows.len());
        for (id, title, description) in rows {
            let tags = fetch_tags(&self.pool, &id).await?;
            summaries.push(KnowledgeSummary {
                id,
                title,
                description,
                tags,
            });
        }

        Ok(summaries)
    }

    pub async fn list_tags(&self) -> Result<Vec<String>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String,)>(
            "SELECT DISTINCT tag FROM knowledge_tag ORDER BY tag ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(tag,)| tag).collect())
    }
}

fn build_list_query(filter: &KnowledgeFilter) -> (String, Vec<String>) {
    let mut binds: Vec<String> = Vec::new();

    let base = if filter.tag.is_some() {
        "SELECT DISTINCT k.id, k.title, k.description FROM knowledge k \
         INNER JOIN knowledge_tag kt ON kt.knowledge_id = k.id WHERE 1=1"
            .to_owned()
    } else {
        "SELECT k.id, k.title, k.description FROM knowledge k WHERE 1=1".to_owned()
    };

    let mut query = base;

    if let Some(ref project) = filter.project {
        query.push_str(" AND k.project = ?");
        binds.push(project.clone());
    }

    if filter.shared {
        query.push_str(" AND k.project IS NULL");
    }

    if let Some(ref tag) = filter.tag {
        query.push_str(" AND kt.tag = ?");
        binds.push(tag.clone());
    }

    query.push_str(" ORDER BY k.title ASC");

    (query, binds)
}

fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for tag in tags {
        let normalized = tag.trim().to_lowercase();
        if !normalized.is_empty() && seen.insert(normalized.clone()) {
            result.push(normalized);
        }
    }
    result.sort();
    result
}

async fn insert_tags(
    pool: &SqlitePool,
    knowledge_id: &str,
    tags: &[String],
) -> Result<(), sqlx::Error> {
    for tag in tags {
        sqlx::query("INSERT INTO knowledge_tag (knowledge_id, tag) VALUES (?, ?)")
            .bind(knowledge_id)
            .bind(tag)
            .execute(pool)
            .await?;
    }
    Ok(())
}

async fn replace_tags(
    pool: &SqlitePool,
    knowledge_id: &str,
    tags: &[String],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM knowledge_tag WHERE knowledge_id = ?")
        .bind(knowledge_id)
        .execute(pool)
        .await?;
    insert_tags(pool, knowledge_id, tags).await
}

async fn fetch_tags(pool: &SqlitePool, knowledge_id: &str) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String,)>(
        "SELECT tag FROM knowledge_tag WHERE knowledge_id = ? ORDER BY tag ASC",
    )
    .bind(knowledge_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(tag,)| tag).collect())
}

fn validate_description(description: &str) -> Result<(), sqlx::Error> {
    if description.len() > MAX_DESCRIPTION_LEN {
        return Err(sqlx::Error::Protocol(format!(
            "description exceeds maximum length of {MAX_DESCRIPTION_LEN} characters"
        )));
    }
    Ok(())
}
