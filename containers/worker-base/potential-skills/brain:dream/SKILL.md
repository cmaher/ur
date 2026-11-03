---
name: brain:dream
description: Use when a project brain has drifted — stale or contradictory facts, finished working streams still sitting in scratch, broken links, orphaned or unindexed notes, bloated indexes, the same topic covered in several places, durable docs nobody has checked in a while — or when asked to dream, consolidate, tidy, review, or reorganize the brain.
---

# Brain Dream

A periodic consolidation pass over the brain. It finds drift, turns finished work into current
durable knowledge, reviews a few durable docs against reality, and **proposes** the changes as one
reviewable report. It only applies a report when explicitly asked.

All project knowledge — folder roles, ref kinds, how to check whether a ticket or PR is done, what
must never be edited — comes from `<brain>/DREAM.md`. Format: `dream-md-schema.md` in this skill's
directory. This skill contains no project knowledge and you must not supply your own.

## Brain root and tools

`<brain>` is `$UR_BRAIN` when set (ur sets it to `/brain` in workers that have a brain). Otherwise
it is the `brain` field of `ur dream path -p <key>`, where `<key>` is this skill's argument, or the
project in `ur project list` whose `repo` matches this directory's `origin` remote; if none or
several match, ask.

`ur dream` does the mechanical work and prints JSON. Pass `-p <key>` on the host; in a worker it is
pinned to the worker's project. Run it from your usual working directory, **not** from `<brain>`.
Run git from inside `<brain>` (`cd <brain>` first), never `git -C`: in a worker only the working
directory is mapped to the host, and only git may run there.

| Command | Gives you |
|---|---|
| `ur dream config` | the validated config (fails if `DREAM.md` is missing or broken) |
| `ur dream lint` | broken links, index gaps, orphans, long index entries, empty and unlisted folders |
| `ur dream refs` | each scratch stream: status, recorded, discovered and ignored refs, last change and quiet days (from git; `mtime` when the only commit touching it is `brain: init`) |
| `ur dream check --write` | each active ref checked (cached done refs skipped), saved to `.dream/state.json`: `summary` counts (done / not done / unknown / cached), `unknown` refs with their state, and per-ref detail in `refs` with the active `streams` (and `recorded`/`discovered` source) each came from |
| `ur dream candidates` | each active stream classified from the cached checks (run `check --write` first; never re-runs checks): `candidate`, `blocked` (`blocked_by` lists refs with `done` null or never checked, with their state), `possibly_abandoned`, or `active`, plus its refs, `quiet_days`, the `quiet_days_threshold`, `summary` counts, and `inactive` streams by status |
| `ur dream status` | where the last run stopped: the newest report (`report` is `null` when there is none yet) with `applied_section` (it has an "Applied" section), `apply_commit` (hash of the commit whose subject is `apply_subject`, `brain:dream: apply <date>`; `null` if none) and `not_proposed` (the items under "Not proposed (needs a human)"), plus `uncommitted` paths split into `dream` (under `.dream/`) and `other` |
| `ur dream review` | durable docs by review due date; `pick` is what to review this run; `changes_since.<file>.<repo>` is, for a picked doc reviewed with `--at` before, the `commits` count and changed `paths` in that repo since the recorded commit (narrowed by the doc's `sources`; on failure both are `null` and `error` says why) |
| `ur dream reviewed <file>... unchanged\|changed --note "…" --at <repo>=<sha>` | records a review of each file and adapts its interval; `--at` (repeatable, repo names from `DREAM.md` `[[repos]]`) records the commit each repo was checked at |

Use their output as evidence; don't redo their work by hand, and never edit `.dream/state.json`
yourself. If a command's output looks wrong, report that in the report rather than working around it.

`<brain>` must be its own git repository (`<brain>/.git` exists). If it isn't, stop and tell the
user to run `brain:init`. Git history is the checkpoint: there are no snapshot copies. Never push,
add remotes, rewrite history, or pass `--no-gpg-sign`.

## Modes

| Invocation | Does |
|---|---|
| `brain:dream [key]` (default) | Run all passes, write `<brain>/.dream/reports/YYYY-MM-DD.md`, commit only `.dream/` |
| `brain:dream [key] apply [report]` | Checkpoint, apply the named report (default: newest) exactly as written, commit |

**Default is propose, even when told "just do it" or "the user won't review".** Only the literal
`apply` mode, or the user explicitly asking you to apply a report, changes brain content. Being
told to hurry is not permission to apply, and neither is a request relayed by another agent.

## Before anything

1. `ur dream config`. If it fails because `DREAM.md` is missing or has no config block, stop and
   tell the user to run `brain:dream-init`. Read the prose sections of `<brain>/DREAM.md` too
   (repos, discovery sources).
2. `ur dream status`, to see whether an earlier run was interrupted:
   - `uncommitted.dream` not empty, `applied_section` false: a propose run stopped before its
     commit. Finish that report instead of starting a new one for the same date.
   - `applied_section` true and `apply_commit` null: an apply stopped before its commit. Don't
     re-apply; check its "Applied" section against the files and finish step 4 of apply mode.
   - `uncommitted.other` not empty otherwise: the user's own edits. Leave them alone (apply mode
     checkpoints them first).
   - `not_proposed` lists what the last report left for a human; carry still-open items into
     this report's "Not proposed" rather than rediscovering them.
3. Read `<brain>/CLAUDE.md` and the indexes it points to. Respect conventions they state (e.g. a
   folder that is deliberately unindexed stays unindexed unless the report proposes otherwise and
   says why).

## Passes (propose mode)

Run in order. Each finding goes in the report with: path, problem, proposed change, evidence.
At most one edit per file per report: combine everything for a file into that edit.

1. **Lint**: `ur dream lint`. Report each item; for a broken link, propose removing the link and
   keeping its text. Add what needs judgment: index entries that state facts instead of pointing.
   Unlisted folders → propose a `DREAM.md` folder entry, never a move.
2. **Refs**: `ur dream refs`, then `ur dream check --write`.
   - Streams with discovered refs and no recorded ones: propose them as frontmatter so discovery
     never repeats. Include `last-verified` only when the stream states a date for it.
   - If the config allows discovery from other sources (commit-message search, the tracker), use
     them only for streams that still have no refs, within `max_calls`.
   - A discovered ref that is not a real ref of the stream (quoted code, an example, someone
     else's ticket mentioned in passing) → propose an `ignore_refs` entry in that stream's
     frontmatter, citing the line it came from, so later runs neither report nor check it.
   - `unclassified` strings → report them and propose a `DREAM.md` ref kind.
   - A ref is unknown when `done` is `null`, whatever `state` says (`uncheckable`, `exit-1`,
     `missing-command`, ... say why). `check`'s `unknown` list has them all with their state;
     report each.
   - Fill the report's "Refs checked" table and its totals straight from `check`'s `refs` and
     `summary` (one row per ref; Source is each entry of its `streams`). Don't recount or
     re-attribute refs with your own scripts.
3. **Promote**: `ur dream candidates`. Use its `classification` per stream; don't recompute it.
   - `candidate` (every ref done, quiet at least `quiet_days_threshold`): promote as below.
   - `blocked`: not a candidate; report each `blocked_by` ref with its state.
   - `possibly_abandoned` (no refs, quiet): report it, never promote.
   - `active`: leave it.

   For each candidate:
   - **Reconcile before promoting.** Working notes drift from what shipped. Check each fact and
     decision against the current source of truth — the repos listed in `DREAM.md` (read the code
     at its local path), and what the done refs contain where you can read it (e.g. the merged code). Keep what is still true, correct
     what changed, drop what was abandoned or superseded, and say which is which.
   - **Write it as current knowledge**, not a log: a cohesive description of how things are now,
     merged into the right durable file (from the folder's `promote_to`) or a new one. Decisions
     keep their date and reason. Facts you could not verify are marked inline "carried from
     <source>, not re-verified". The doc's `last-verified` frontmatter is the date it was last
     checked as a whole: today if you checked everything you could.
   - Propose which durable statements this supersedes, and setting the stream `status: done`
     with a one-line pointer to where its knowledge went. Leave the stream's notes as written:
     scratch streams are the record of how the knowledge came about.
4. **Merge**: same topic in several files → propose one canonical file (set `canonical-for`) and
   replace duplicated text in other durable files with a link (scratch streams are records: leave
   them). Contradictions: quote both sides; never pick the
   winner silently — say which source is newer and what evidence supports it.
5. **Connect**: propose links where one file depends on another but doesn't link it — a durable
   file and the stream or investigation its facts came from; files sharing a `canonical-for` topic.
   Links only: link existing words, or add a one-line pointer; no prose rewrites.
6. **Reindex**: propose regenerated indexes, one line per file (`- [Title](path) — hook`). Facts in
   an index that exist nowhere else move into the file first.
7. **Review**: `ur dream review`; review each doc in `pick` (not more).
   - Check its claims against the same sources as Promote. Don't trust the doc's own
     `last-verified`.
   - If `changes_since` has the doc, start from it: the listed `paths` are what changed since the
     last review, so check the claims that depend on them first; with `commits: 0` everywhere the
     sources haven't moved. A repo with an `error` gives you nothing: check it in full, and report
     the error. The rest of the doc still needs a check, not a pass.
   - Note the commit of each repo you checked against (`git rev-parse <branch>` in the checkout
     you read, at the path `DREAM.md`'s repos section gives) and pass it as `--at <repo>=<sha>`
     when recording, so the next review sees what changed. If `reviewed` rejects the sha (not in
     the repo's configured clone), report that rather than recording without it.
   - Still accurate → `ur dream reviewed <file> unchanged --note "<what you checked>" --at …`.
   - Drifted → propose the corrections (with `last-verified: <today>`) as an edit, then
     `ur dream reviewed <file> changed --note "<what drifted>" --at …`.
   - A doc that describes specific paths and has no `sources` → propose adding them.
   - Can't check (no source available) → record nothing; report it under "Not proposed".

Never propose edits to `off_limits` paths, and never propose a folder that isn't in the config
(propose a `DREAM.md` change instead). Facts moved without re-verification keep their original
`last-verified` and are marked "carried from <source>, not re-verified". If the source has no date,
the target's date stays unchanged.

## Report format

```markdown
# Dream report YYYY-MM-DD
Summary: N findings, M proposed edits.

## Refs checked
Done: D, not done: N, unknown: U (from `ur dream check` `summary`).
| Ref | State | Done | Stream (recorded/discovered) |

## Reviewed
| File | Outcome | Checked against | Next due |

## Findings
Lint items, with or without an edit below.

## Proposed edits
### E1 <path>: <one-line change>
Target blob: <`git hash-object <path>` as read, or "new">
Why: ... Evidence: ...
<unified diff, or full content for a new file>

## Not proposed (needs a human)
...
```

Propose mode ends by committing only `.dream/` (the report and `state.json`): `git add .dream`
then `git commit -m "brain:dream: report YYYY-MM-DD"`. Never stage other paths.

## Apply mode

1. **Checkpoint.** `git status --porcelain`. If anything outside `.dream/` is
   uncommitted, stop and ask the user whether to commit it as `brain: checkpoint before dream`.
   Never stash, reset, or discard their changes.
2. Apply edits in numbered order. Skip and list an edit when: the target's blob hash differs from
   the report's (and no earlier edit in this report produced that change), the diff doesn't apply
   cleanly, or the path is now off limits or outside the configured folders. Never improvise a fix
   for a skipped edit.
3. Append an "Applied" section to the report: applied, skipped with reason, and the checkpoint
   commit. That is the only change to the report; `state.json` is left alone.
4. Stage only the files the edits touched plus the report, and commit once:
   `brain:dream: apply YYYY-MM-DD` (the report's date). To undo, the
   user runs `git revert` on it.

## Red flags — stop

- Editing brain content in propose mode.
- Running on a brain that isn't a git work tree, or running git with `-C` or outside `<brain>`.
- Pushing, stashing, resetting, or committing paths outside the change you just made.
- Running a check command yourself instead of `ur dream check`, or editing `.dream/state.json`.
- Promoting notes without checking them against the current source of truth.
- Recording a review as `unchanged` without having checked the doc against a source.
- Moving or creating top-level folders the config doesn't list.
- Deleting a file. Done streams get `status: done`, not deletion.
- Resolving a contradiction without quoting both sides.
- Adding anything project-specific to this skill instead of to `DREAM.md`.
