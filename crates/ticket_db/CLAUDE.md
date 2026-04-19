# ticket_db

Postgres-backed ticket database crate. Owns the ticket lifecycle schema: tickets, activities, metadata, edges, and dependency graph.

## Responsibilities

- Migrations for the ticket domain (ticket, activity, meta, edge tables).
- `TicketRepo` — CRUD operations for tickets, activities, and metadata.
- `GraphManager` — dependency graph operations using petgraph, loaded from Postgres.

## Conventions

- All database access is async via sqlx with a `PgPool`.
- Managers implement `Clone` and accept dependencies via constructor (dependency injection).
- Never modify existing migration files — always add new ones for schema changes.
