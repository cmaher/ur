# Activities Viewer for Ticket Detail Page

Ticket: ur-qbe6m

## Overview

This document designs the UX for viewing ticket activities on the `TicketDetailScreen`. Activities are timestamped notes appended by workers and humans via `ur ticket add-activity`. The `GetTicketResponse` already returns them as `repeated ActivityEntry activities` (fields: `id`, `timestamp`, `author`, `message`).

## Decision: Separate Screen, Not Inline

Activities live on a **dedicated `TicketActivitiesScreen`**, pushed from `TicketDetailScreen` via a new `Action::OpenActivities` key binding (proposed: `A` / Shift-A).

**Rationale:**

- The detail page already has three competing sections: header (1 row), body preview (5 rows), child table (remainder). Adding activities inline would require shrinking the child table, which is the primary navigation surface. Tickets with many activities (CI agent feedback loops produce dozens of entries) would overflow the available chrome.
- The body viewer is being built as its own screen by a parallel agent (`TicketBodyScreen`). Using the same pattern — push a dedicated screen — is consistent and avoids layout coupling between the two features.
- A separate screen can own its own scroll position and author filter state without complicating the `DataState` of the detail page.
- `GetTicketRequest` supports `activity_author_filter` as an optional field. A separate screen can carry that filter as local state and re-trigger fetches naturally.

## Screen: `TicketActivitiesScreen`

### Data

The activities are already included in `GetTicketResponse.activities` which is fetched by `DataManager::fetch_ticket_detail`. However, the activities screen will issue its own `GetTicket` call (a new `DataPayload::TicketActivities` variant) so it can:

1. Apply `activity_author_filter` independently.
2. Refresh without re-fetching child tickets.
3. Stay decoupled from `TicketDetailScreen` data lifetime.

### Layout

```
┌──────────────────────────────────────────────────────────────────┐
│ ur-abc  My Ticket Title [Open]         Activities (12)           │  ← header (1 row)
├──────────────────────────────────────────────────────────────────┤
│ Filter: [all authors ▼]                                          │  ← filter bar (1 row, only when filter active)
├──────────────────────────────────────────────────────────────────┤
│ ┌────────────────────────────────────────────────────────────┐   │
│ │ Timestamp            Author         Message                │   │  ← ThemedTable header
│ ├────────────────────────────────────────────────────────────┤   │
│ │ 2026-03-25 14:32:10  claude         Fixed linter errors    │   │
│ │ 2026-03-25 14:10:05  alice          Please also update ... │   │
│ │ 2026-03-25 13:58:40  claude         Starting implementa... │   │
│ └────────────────────────────────────────────────────────────┘   │
│ [footer: Esc=Back  r=Refresh  *=Filter  Q=Quit]                  │
└──────────────────────────────────────────────────────────────────┘
```

**Column widths (ThemedTable):**
- Timestamp: `Length(20)` — `YYYY-MM-DD HH:MM:SS`
- Author: `Length(18)` — truncated if longer
- Message: `Fill(1)` — remaining width, truncated to one line

Entries are displayed **newest-first** (reverse chronological), mirroring the flow history table in `FlowDetailScreen`.

### Scrolling

The `ThemedTable` widget does not scroll internally — the screen manages a `selected_row` index and a `page` / `page_size` pair, just like `TicketDetailScreen` does for children. However, because activities are not paginated server-side (the server returns all activities in one `GetTicket` call), pagination is **client-side**: the screen slices the `Vec<ActivityEntry>` using `page * page_size .. (page+1) * page_size`.

`j`/`k` (or arrow keys) navigate within the current page. `h`/`l` (or arrow keys) step pages. The footer shows `Page N/M (X activities)` the same way the child table does.

### Author Filter

Pressing `*` (the existing `Action::Filter` binding) cycles through a list of unique authors extracted from the last-fetched activities:
- **All** (default) — show every entry
- **alice** — show only Alice's entries
- **claude** — show only Claude's entries
- etc.

When a filter is active, the filter bar row (1 row) is shown beneath the header, showing `Filter: [author]`. The screen re-issues a `GetTicket` call with `activity_author_filter` set to the chosen author. The server already supports this field.

Pressing `*` again cycles to the next author; wrapping back to "All" clears the filter.

### Relationship to Body Viewer

`TicketBodyScreen` (parallel work) and `TicketActivitiesScreen` are both pushed from `TicketDetailScreen` as independent sibling screens. They share no data or state. The detail screen grows two new key bindings:

| Key | Action | Pushes |
|-----|--------|--------|
| `b` (existing `Action::OpenTicket`) | Open body viewer | `TicketBodyScreen` |
| `A` (new `Action::OpenActivities`) | Open activities | `TicketActivitiesScreen` |

`Action::OpenActivities` is a new variant added to the `Action` enum in `keymap.rs` and wired into `TicketDetailScreen::handle_action`.

### `DataPayload` Addition

A new variant `TicketActivities(Result<Vec<ActivityEntry>, String>)` is added to `DataPayload`. `DataManager` gains `fetch_ticket_activities(ticket_id, author_filter)` which issues `GetTicket` and sends this payload. `TicketActivitiesScreen::on_data` handles it.

### `Screen` Trait Downcast

Following the existing pattern, `Screen` gains:
```rust
fn as_any_ticket_activities(&self) -> Option<&TicketActivitiesScreen> { None }
fn as_any_ticket_activities_mut(&mut self) -> Option<&mut TicketActivitiesScreen> { None }
```

These are needed if `app.rs` must interact with the screen (e.g., to wire refresh on UI events for the ticket entity).

## File Map

| File | Change |
|------|--------|
| `crates/urui/src/keymap.rs` | Add `Action::OpenActivities`; bind to `A` (Shift+a) in default and `from_config` |
| `crates/urui/src/data.rs` | Add `DataPayload::TicketActivities`; add `DataManager::fetch_ticket_activities` |
| `crates/urui/src/pages/ticket_activities.rs` | New file: `TicketActivitiesScreen` |
| `crates/urui/src/pages/mod.rs` | Export `TicketActivitiesScreen` |
| `crates/urui/src/screen.rs` | Add downcast methods for `TicketActivitiesScreen` |
| `crates/urui/src/pages/ticket_detail.rs` | Handle `Action::OpenActivities` → push `TicketActivitiesScreen` |
| `crates/urui/src/app.rs` | Route `DataPayload::TicketActivities` to the active screen |

## Acceptance Criteria Mapping

- [x] Design document at `docs/plans/ur-qbe6m-activities-viewer.md` ← this file
- [ ] Implementation tickets created (see below)

## Open Questions

None — the design resolves all questions raised in the ticket:

- **Separate screen vs inline**: Separate screen.
- **Scrolling**: Client-side pagination via `page`/`page_size`, same as child table.
- **Filtering by author**: `*` key cycles authors; re-issues `GetTicket` with `activity_author_filter`.
- **Relationship to body viewer**: Independent sibling screens, both pushed from detail page.
