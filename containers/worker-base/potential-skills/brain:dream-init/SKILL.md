---
name: brain:dream-init
description: Use when a brain's DREAM.md is missing, brain:dream reports it cannot run, or the project's sources of truth changed — a new repo, a new ticket system, a different meaning of "done" — and the dream config needs creating or updating.
---

# Brain Dream Init

Builds or updates `<brain>/DREAM.md`, the only place `brain:dream` learns anything about this
project. Format: `dream-md-schema.md` in the `brain:dream` skill's directory.

**Writes only under `<brain>`.** Never write project details into any skill file, repo file, or
anywhere outside `<brain>`.

## Brain root

`<brain>` is `$UR_BRAIN` when set (ur sets it to `/brain` in workers that have a brain). Otherwise it is the `brain` field of this project in `ur project list --output json`: the project is this skill's argument, or the one whose `repo` matches this directory's `origin` remote; if none or several match, ask. Run every git command from inside `<brain>` (`cd <brain>` first) — never
`git -C`, because in a worker only the working directory is mapped to the host.

`<brain>` should be its own git repository (`<brain>/.git` exists); if not, tell the user to run `brain:init` first.

## Instructions

1. **Read what exists**: `<brain>/DREAM.md` (if it is configured, you are updating it — show the
   user what would change, don't silently rewrite; the empty init stub can simply be replaced), `<brain>/CLAUDE.md`, and each index it points to.
2. **Inspect, don't assume**:
   - Top-level folders of `<brain>` and what each holds → draft the folder table with a role and a
     `promote to` for scratch folders.
   - Scan brain text for ref-like strings: issue keys (`ABC-123` shapes), PR references
     (`org/repo#N`, PR URLs), branch names, internal ticket ids. Group by shape and count them.
   - Which commands exist in this environment (`command -v`, or `test -x` for an absolute path).
     Only propose check commands that exist; for others, ask the user how to check that kind. A
     kind nobody can check is still kept, with check `—`, so its refs get recorded and reported
     `unknown`.
   - Repos: the workspace's git remotes, plus repos that refs name. No local clone → local path
     `—`, and commit-message search is not a discovery source for that repo.
   - Generated or vendored files in the brain (headers like "generated", "do not edit", dump
     files) → draft `off limits`.
3. **Ask the user**, one topic at a time, with your draft as the default:
   - For each ref kind: the regex, the read-only check command, what output means done, and what
     means not done. Anything else (other output, unexpected exit code) is `unknown`. Run the
     command once against a real id from the brain and once against a made-up id, and show both
     outputs before accepting it: the made-up id must come out `unknown`, not "not done".
   - Is discovery allowed, from which sources, and with what call budget?
   - Quiet days and stale-verify days.
   - Anything that must never be edited.
4. **Write `<brain>/DREAM.md`**. Placeholders the user didn't fill
   stay out — omit a section rather than guess.
5. **Add the frontmatter rule** to `<brain>/CLAUDE.md` if absent: new scratch streams start with
   `status`, `refs` (using the ref kinds defined in `DREAM.md`), and `last-verified` frontmatter.
   Recording refs when work starts is what keeps `brain:dream` cheap.
6. **Commit** only the files you wrote: `git add DREAM.md CLAUDE.md` then
   `git commit -m "brain:dream-init: configure dream"`. Use the user's normal git
   config; never push.
7. **Report** what you wrote and suggest running `brain:dream` to see a first report. Don't fix
   brain content or backfill frontmatter on existing streams here — `brain:dream` proposes both.
   Mention problems you noticed so the user knows to expect them.

## Red flags

- Proposing a check command you haven't run once.
- A check command that writes, comments, or changes state.
- Copying ref patterns, repo names or commands into anything outside `<brain>`.
