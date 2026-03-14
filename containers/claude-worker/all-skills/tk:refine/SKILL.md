---
name: tk:refine
description: Use when tickets from brainstorming need implementation-level detail — enriches ticket bodies with exact file paths, code, commands, and TDD steps
---

# Ticket Refinement

Take tickets produced by brainstorming and enrich them with everything an implementing agent needs: exact file paths, code, test commands, and TDD steps. The detail goes into the ticket body, not separate doc files.

**Announce at start:** "I'm using the tk:refine skill to add implementation detail to the tickets."

## Inputs

`$ARGUMENTS` should be an epic ticket ID. If empty, ask the user which epic to refine.

## Workflow

### 1. Load the Epic

```bash
tk show <epic-id>
```

Read the epic's design document (its body). This is the spec — the source of truth for what to build.

### 2. List Child Tickets

```bash
tk list --parent <epic-id>
```

If no children exist, stop and tell the user to run brainstorming first.

### 3. Map the File Structure

Before refining individual tickets, map out which files will be created or modified across the entire epic. This is where decomposition decisions get locked in.

- Design units with clear boundaries and well-defined interfaces. Each file should have one clear responsibility.
- Prefer smaller, focused files over large ones that do too much.
- Files that change together should live together. Split by responsibility, not by technical layer.
- In existing codebases, follow established patterns.

Present the file structure to the user for approval before proceeding.

### 4. Refine Each Ticket

For each child ticket, read its current body, then rewrite it with implementation detail. Edit the ticket's `.md` file directly (below the YAML frontmatter).

**Ticket body structure:**

```markdown
## Goal

[One sentence — what this ticket accomplishes]

## Files

- Create: `exact/path/to/file.rs`
- Modify: `exact/path/to/existing.rs:123-145`
- Test: `tests/exact/path/to/test.rs`

## Steps

- [ ] **Step 1: Write the failing test**

\```rust
#[test]
fn test_specific_behavior() {
    let result = function(input);
    assert_eq!(result, expected);
}
\```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test test_specific_behavior -- --nocapture`
Expected: FAIL — `function` not found

- [ ] **Step 3: Write minimal implementation**

\```rust
pub fn function(input: Type) -> Output {
    expected
}
\```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test test_specific_behavior -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

\```bash
git add tests/path/test.rs src/path/file.rs
git commit -m "feat: add specific feature"
\```
```

### 5. Wire Dependencies

After all tickets are refined, verify dependencies are correct:

```bash
tk dep tree <epic-id>
```

Add any missing `tk dep` relationships that became apparent during refinement.

### 6. Done

Tell the user the tickets are refined and ready. Do NOT suggest next steps, invoke implementation skills, or mention coding.

## Refinement Standards

- **Exact file paths always** — no "somewhere in src/"
- **Complete code in steps** — not "add validation" or "similar to above"
- **Exact commands with expected output** — the agent should know what success looks like
- **TDD by default** — test first, implement second, verify, commit
- **Each step is one action (2-5 minutes)** — if a step takes longer, break it down
- **Frequent commits** — one commit per logical unit of work
- **DRY, YAGNI** — only what the spec calls for, no extras

## Scope Check

If a ticket covers multiple independent concerns, split it into separate tickets before refining. Each ticket should be completable by a single agent in one session.

$ARGUMENTS
