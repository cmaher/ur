# Migration: Single-DB to Two-DB Architecture

This runbook walks through upgrading an existing Ur installation from the old single-database architecture (one `ur` database, `[db]` config section, `ur_db` crate) to the new two-database architecture (`ur_tickets` + `ur_workflow`, `[ticket_db]`/`[workflow_db]` config sections, `ticket_db` + `workflow_db` crates).

## Background

The old architecture stored all tables (tickets, workflows, workers, slots, etc.) in a single Postgres database (default name: `ur`). The new architecture splits this into two databases:

- `ur_tickets` — ticket, edge, activity, meta, slot, worker, worker_slot, ticket_comments, ui_events
- `ur_workflow` — workflow, workflow_event, workflow_intent, workflow_comments, workflow_events, ui_events

## Prerequisites

- `ur-server` must be stopped before beginning
- You must have access to the `ur-postgres` container (or the Postgres host directly)
- The backup path must be writable and have sufficient disk space for two `pg_dump` exports

## Step-by-Step Upgrade

### Step 1: Stop the Server

```bash
ur stop
```

Verify no `ur-server` processes remain:

```bash
docker ps | grep ur-server
```

### Step 2: Export the Existing Database

Run a full export of the old single database before making any changes:

```bash
docker exec ur-postgres pg_dump -Fc -f /backup/pre-migration-ur.pgdump ur
```

Verify the dump was created and is non-zero:

```bash
docker exec ur-postgres ls -lh /backup/pre-migration-ur.pgdump
```

Keep this file until the migration is fully verified. It is your rollback point.

### Step 3: Upgrade the `ur-server` Binary

Pull the new `ur-server` image or build and deploy the new server binary. The new server:

- Reads `[ticket_db]` and `[workflow_db]` config sections instead of `[db]`
- Connects to `ur_tickets` and `ur_workflow` databases
- Creates both databases automatically on first start via the init SQL script

Do not start the server yet.

### Step 4: Update `ur.toml`

Replace the old `[db]` section with the two new sections. Example:

**Before:**
```toml
[db]
host     = "ur-postgres"
port     = 5432
user     = "ur"
password = "ur"
name     = "ur"

[db.backup]
path             = "/backup"
interval_minutes = 30
retain_count     = 3
```

**After:**
```toml
[ticket_db]
host     = "ur-postgres"
port     = 5432
user     = "ur"
# password set via UR_TICKET_DB_PASSWORD env var, or:
password = "ur"
name     = "ur_tickets"

[ticket_db.backup]
path             = "/backup"
interval_minutes = 30
retain_count     = 3

[workflow_db]
host     = "ur-postgres"
port     = 5432
user     = "ur"
# password set via UR_WORKFLOW_DB_PASSWORD env var, or:
password = "ur"
name     = "ur_workflow"

[workflow_db.backup]
path             = "/backup"
interval_minutes = 30
retain_count     = 3
```

### Step 5: Create the New Databases

The new server will create `ur_tickets` and `ur_workflow` automatically when it first starts by running the init SQL script. You can also create them manually:

```bash
docker exec ur-postgres psql -U ur -c "CREATE DATABASE ur_tickets OWNER ur;"
docker exec ur-postgres psql -U ur -c "CREATE DATABASE ur_workflow OWNER ur;"
```

If the databases already exist from a previous partial migration attempt, drop and recreate them:

```bash
docker exec ur-postgres psql -U ur -c "DROP DATABASE IF EXISTS ur_tickets;"
docker exec ur-postgres psql -U ur -c "DROP DATABASE IF EXISTS ur_workflow;"
docker exec ur-postgres psql -U ur -c "CREATE DATABASE ur_tickets OWNER ur;"
docker exec ur-postgres psql -U ur -c "CREATE DATABASE ur_workflow OWNER ur;"
```

### Step 6: Start the Server (Schema Migration)

Start the server once to let it run its sqlx migrations and create the new schema:

```bash
ur start
```

The server will:
1. Connect to `ur_tickets` and `ur_workflow`
2. Run all embedded migrations for each crate
3. Create all tables, indexes, and triggers

Verify the server started cleanly by checking the logs. Stop it again immediately before importing data:

```bash
ur stop
```

### Step 7: Import Data into the New Databases

Use `pg_restore` with `--table` flags to extract specific tables from the old dump and import them into the correct new databases.

**Import ticket-domain tables into `ur_tickets`:**

```bash
docker exec ur-postgres pg_restore \
  --clean --if-exists \
  --no-owner --no-acl \
  -d ur_tickets \
  -t ticket \
  -t edge \
  -t meta \
  -t activity \
  -t slot \
  -t worker \
  -t worker_slot \
  -t ticket_comments \
  /backup/pre-migration-ur.pgdump
```

**Import workflow-domain tables into `ur_workflow`:**

```bash
docker exec ur-postgres pg_restore \
  --clean --if-exists \
  --no-owner --no-acl \
  -d ur_workflow \
  -t workflow \
  -t workflow_event \
  -t workflow_intent \
  -t workflow_comments \
  -t workflow_events \
  /backup/pre-migration-ur.pgdump
```

Note: The `ui_events` table is an ephemeral buffer and does not need to be imported. Its contents are discarded between restarts.

### Step 8: Verify the Import

Spot-check row counts in the new databases against the old database:

```bash
# Ticket counts
docker exec ur-postgres psql -U ur -d ur -c "SELECT COUNT(*) FROM ticket;"
docker exec ur-postgres psql -U ur -d ur_tickets -c "SELECT COUNT(*) FROM ticket;"

# Workflow counts
docker exec ur-postgres psql -U ur -d ur -c "SELECT COUNT(*) FROM workflow;"
docker exec ur-postgres psql -U ur -d ur_workflow -c "SELECT COUNT(*) FROM workflow;"
```

Both counts should match.

### Step 9: Start the Server

```bash
ur start
```

Verify that `ur ticket list` returns your expected tickets and that workflows are intact.

### Step 10: Drop the Old Database

Once you have confirmed the migration is successful and the system is operating normally:

```bash
docker exec ur-postgres psql -U ur -c "DROP DATABASE ur;"
```

Keep the `pre-migration-ur.pgdump` export for at least one backup cycle as a safety net before deleting it.

## Rollback

If anything goes wrong before Step 10 (dropping the old DB), you can roll back:

1. Stop the server: `ur stop`
2. Restore the old config (`[db]` section) in `ur.toml`
3. Deploy the old `ur-server` binary
4. Start the server: `ur start`

If you have already dropped the old database, restore it from the export:

```bash
docker exec ur-postgres psql -U ur -c "CREATE DATABASE ur OWNER ur;"
docker exec ur-postgres pg_restore \
  --clean --if-exists \
  --no-owner --no-acl \
  -d ur \
  /backup/pre-migration-ur.pgdump
```

Then follow the full rollback procedure above.

## Troubleshooting

**Server fails to connect:** Check that `[ticket_db]` and `[workflow_db]` sections are present in `ur.toml` and that the database names match. Verify the databases exist:

```bash
docker exec ur-postgres psql -U ur -l
```

**sqlx migration checksum error:** You modified an existing migration file. Restore the original file from version control — never edit applied migrations.

**Row count mismatch after import:** Some tables in the old dump may have had constraints that prevented import. Check `pg_restore` output for errors and re-run the import after fixing any issues.

**`ui_events` import errors:** Skip this table — it is ephemeral and should not be imported.
