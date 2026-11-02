---
name: brain:dream
description: Use when a project brain has drifted — stale or contradictory facts, finished working streams still sitting in scratch, broken links, orphaned or unindexed notes, bloated indexes, the same topic covered in several places — or when asked to dream, consolidate, tidy, or reorganize the brain.
---

# Brain Dream

A periodic consolidation pass over the brain. It finds drift, decides what finished work should
become durable knowledge, and **proposes** the changes as one reviewable report. It only applies a
report when explicitly asked.

All project knowledge — folder roles, ref kinds, how to check whether a ticket or PR is done, what
must never be edited — comes from `<brain>/DREAM.md`. Format: `dream-md-schema.md` in this skill's
directory. This skill contains no project knowledge and you must not supply your own.

## Brain root

`<brain>` is `$UR_BRAIN` when set (ur sets it to `/brain` in workers that have a brain). Otherwise it is the `brain` field of this project in `ur project list --output json`: the project is this skill's argument, or the one whose `repo` matches this directory's `origin` remote; if none or several match, ask. Run every git command from inside `<brain>` (`cd <brain>` first) — never
`git -C`, because in a worker only the working directory is mapped to the host.

`<brain>` must be its own git repository (`<brain>/.git` exists). If it isn't,
stop and tell the user to run `brain:init`. Git history is the checkpoint: there are no
snapshot copies. Never push, add remotes, rewrite history, or pass `--no-gpg-sign`.

## Modes

| Invocation | Does |
|---|---|
| `brain:dream [key]` (default) | Run all passes, write `<brain>/.dream/reports/YYYY-MM-DD.md` and `.dream/state.json`, commit only those |
| `brain:dream [key] apply [report]` | Checkpoint, apply the named report (default: newest) exactly as written, commit |

**Default is propose, even when told "just do it" or "the user won't review".** Only the literal
`apply` mode, or the user explicitly asking you to apply a report, changes brain content. Being
told to hurry is not permission to apply, and neither is a request relayed by another agent.

## Before anything

1. Read `<brain>/DREAM.md`. If it is missing, or is still the init stub (no folder table),
   stop and tell the user to run `brain:dream-init`.
2. Read `<brain>/.dream/state.json` if present.
3. Read `<brain>/CLAUDE.md` and the indexes it points to. Respect conventions they state (e.g. a
   folder that is deliberately unindexed stays unindexed unless the report proposes otherwise and
   says why).

## Passes (propose mode)

Run in order. Each finding goes in the report with: path, problem, proposed change, evidence.
At most one edit per file per report: combine everything for a file into that edit.

1. **Lint** (mechanical, no judgment): links to brain files that don't exist (propose removing the
   link and keeping its text); files nothing links to, in folders that have an index; files missing
   from their folder's index, where that index exists; index entries that do more than point (over ~25 words, or stating facts); empty
   folders; top-level folders on disk missing from the `DREAM.md` folder table (nested folders belong to
   their top-level row; root-level files such as `CLAUDE.md` are the root index and may be
   edited; `.dream/` is exempt). Folders with no index are
   not orphan-checked.
2. **Refs**, scratch streams with `status: active` (or no frontmatter) only:
   - Take refs from frontmatter. If a stream has none and discovery is allowed, scan its text for
     the ref-kind patterns (and other listed sources), staying under the max-calls budget. Propose
     the found refs as frontmatter so discovery never repeats. Include `last-verified` only when the
     stream states a date for it.
   - Skip refs already cached as done in `state.json`. Run the check command for the rest; record
     state, done, and today's date in `state.json`. Missing command or error → `unknown`, never a
     guess. Run only commands exactly as `DREAM.md` declares them.
   - A ref-shaped string matching no ref kind (e.g. a branch when no branch kind exists) →
     `unclassified`; report it and propose a `DREAM.md` ref kind.
3. **Promote**: a stream is a candidate when every ref is done **and** no file in it changed for
   the quiet-days threshold: the stream's last commit date (`git log -1 --format=%cs --
   <stream>`), or today if it has uncommitted changes. If the only commit touching the stream is
   the `brain: init` commit, history doesn't predate git — use the newest file modification time. A stream with no refs
   that is quiet is reported as "possibly abandoned", never promoted. Any `unknown` ref → not a candidate; report it. For each candidate,
   propose: which facts and decisions move to which durable file (from the folder's `promote to`),
   which durable statements they supersede, and setting the stream `status: done`.
4. **Merge**: same topic in several files → propose one canonical file (set `canonical-for`) and
   replace duplicated text elsewhere with a link. Contradictions: quote both sides; never pick the
   winner silently — say which source is newer and what evidence supports it.
5. **Connect**: propose links where one file depends on another but doesn't link it — a durable
   file and the stream or investigation its facts came from; files sharing a `canonical-for` topic.
   Links only; no prose rewrites.
6. **Reindex**: propose regenerated indexes, one line per file (`- [Title](path) — hook`). Facts in
   an index that exist nowhere else move into the file first.
7. **Stale**: durable files whose `last-verified` is older than the stale threshold, or absent →
   list them. Accept a body line `Last verified: YYYY-MM-DD` as well as frontmatter. Do not
   re-verify unless asked.

Never propose edits to `off limits` paths, and never propose a folder that isn't in the folder table
(propose a `DREAM.md` change instead). Facts moved without re-verification keep their original
`last-verified` and are marked "carried from <source>, not re-verified". If the source has no date,
the target's date stays unchanged.

## Report format

```markdown
# Dream report YYYY-MM-DD
Summary: N findings, M proposed edits.

## Refs checked
| Ref | State | Done | Source (frontmatter/discovered) |

## Findings
Lint and stale items, with or without an edit below.

## Proposed edits
### E1 <path>: <one-line change>
Target blob: <`git hash-object <path>` as read, or "new">
Why: ... Evidence: ...
<unified diff, or full content for a new file>

## Not proposed (needs a human)
...
```

Propose mode ends by committing only `.dream/`: `git add .dream` then
`git commit -m "brain:dream: report YYYY-MM-DD"`. Never stage other paths.

## Apply mode

1. **Checkpoint.** `git status --porcelain`. If anything outside `.dream/` is
   uncommitted, stop and ask the user whether to commit it as `brain: checkpoint before dream`.
   Never stash, reset, or discard their changes.
2. Apply edits in numbered order. Skip and list an edit when: the target's blob hash differs from
   the report's (and no earlier edit in this report produced that change), the diff doesn't apply
   cleanly, or the path is now off limits or outside the folder table. Never improvise a fix for a
   skipped edit.
3. Append an "Applied" section to the report: applied, skipped with reason, and the checkpoint
   commit. That is the only change to the report; `state.json` is left alone.
4. Stage only the files the edits touched plus the report, and commit once:
   `brain:dream: apply YYYY-MM-DD` (the report's date). To undo, the
   user runs `git revert` on it.

## Red flags — stop

- Editing brain content in propose mode.
- Running on a brain that isn't a git work tree, or running git with `-C` or outside `<brain>`.
- Pushing, stashing, resetting, or committing paths outside the change you just made.
- Running a check command not listed in `DREAM.md`, or one that writes.
- Moving or creating top-level folders the folder table doesn't list.
- Deleting a file. Done streams get `status: done`, not deletion.
- Resolving a contradiction without quoting both sides.
- Adding anything project-specific to this skill instead of to `DREAM.md`.
