# workflow_db

Postgres-backed workflow database crate. Owns the workflow lifecycle schema: workflow state, events, intents, and comments.

## Responsibilities

- Migrations for the workflow domain (workflow, workflow_event, intent, comment tables).
- `WorkflowRepo` — CRUD operations for workflow state, events, intents, and comments.

## Conventions

- All database access is async via sqlx with a `PgPool`.
- Managers implement `Clone` and accept dependencies via constructor (dependency injection).
- Never modify existing migration files — always add new ones for schema changes.
