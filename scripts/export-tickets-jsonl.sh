#!/usr/bin/env bash
# Emit a JSONL stream equivalent to `ur ticket export`, read directly from the
# pre-split `ur` Postgres DB. Use when the running server predates the
# TicketExport RPC (PR #318) and the DB has not yet been migrated to the
# ticket_db / workflow_db split.
#
# Usage: export-tickets-jsonl.sh [output-path]   (default: tickets.jsonl)

set -euo pipefail

OUT=${1:-tickets.jsonl}
CONTAINER=${UR_POSTGRES_CONTAINER:-ur-postgres}
DB=${UR_DB:-ur}
USER_=${UR_DB_USER:-ur}

# JSON is assembled by hand via format() + to_json so the output is compact
# (no spaces around colons/commas) — byte-matching what `serde_json::to_string`
# produces on the Rust side. json_build_object() works but inserts whitespace.
#
# For edges we intentionally emit a duplicate "kind" key (outer record kind
# "edge" plus the edge's own kind column), matching
# crates/ur/src/ticket/export.rs::inject_kind.
#
# bool columns are rendered via CASE because to_json(bool) → 'true'/'false'
# without quotes, which is what we want but needs explicit branching when used
# with format().

docker exec -i "$CONTAINER" psql -U "$USER_" -d "$DB" -AtX -v ON_ERROR_STOP=1 <<'SQL' > "$OUT"
-- ticket.parent_id has a non-deferrable FK to ticket.id, so we must emit
-- parents before children. Walk the tree with a recursive CTE and order by
-- depth so the server's sequential INSERTs succeed.
WITH RECURSIVE tree AS (
  SELECT id, 0 AS depth FROM ticket WHERE parent_id IS NULL
  UNION ALL
  SELECT t.id, tr.depth + 1 FROM ticket t JOIN tree tr ON t.parent_id = tr.id
)
SELECT format(
  '{"kind":"ticket","id":%s,"project":%s,"type":%s,"status":%s,"lifecycle_status":%s,"lifecycle_managed":%s,"priority":%s,"parent_id":%s,"title":%s,"body":%s,"branch":%s,"created_at":%s,"updated_at":%s}',
  to_json(t.id)::text,
  to_json(t.project)::text,
  to_json(t.type)::text,
  to_json(t.status)::text,
  to_json(t.lifecycle_status)::text,
  CASE WHEN t.lifecycle_managed THEN 'true' ELSE 'false' END,
  t.priority::text,
  COALESCE(to_json(t.parent_id)::text, 'null'),
  to_json(t.title)::text,
  to_json(t.body)::text,
  COALESCE(to_json(t.branch)::text, 'null'),
  to_json(t.created_at)::text,
  to_json(t.updated_at)::text
)
FROM ticket t JOIN tree tr USING (id)
ORDER BY tr.depth ASC, t.id ASC;

SELECT format(
  '{"kind":"edge","source_id":%s,"target_id":%s,"kind":%s}',
  to_json(source_id)::text,
  to_json(target_id)::text,
  to_json(kind)::text
)
FROM edge ORDER BY source_id ASC, target_id ASC, kind ASC;

SELECT format(
  '{"kind":"meta","entity_id":%s,"entity_type":%s,"key":%s,"value":%s}',
  to_json(entity_id)::text,
  to_json(entity_type)::text,
  to_json(key)::text,
  to_json(value)::text
)
FROM meta ORDER BY entity_type ASC, entity_id ASC, key ASC;

SELECT format(
  '{"kind":"activity","id":%s,"ticket_id":%s,"timestamp":%s,"author":%s,"message":%s}',
  to_json(id)::text,
  to_json(ticket_id)::text,
  to_json("timestamp")::text,
  to_json(author)::text,
  to_json(message)::text
)
FROM activity ORDER BY ticket_id ASC, "timestamp" ASC, id ASC;

SELECT format(
  '{"kind":"ticket_comment","comment_id":%s,"ticket_id":%s,"pr_number":%s,"gh_repo":%s,"reply_posted":%s,"created_at":%s}',
  to_json(comment_id)::text,
  to_json(ticket_id)::text,
  pr_number::text,
  to_json(gh_repo)::text,
  CASE WHEN reply_posted THEN 'true' ELSE 'false' END,
  to_json(created_at)::text
)
FROM ticket_comments ORDER BY ticket_id ASC, comment_id ASC;
SQL

echo "Wrote $(wc -l < "$OUT" | tr -d ' ') records to $OUT"
