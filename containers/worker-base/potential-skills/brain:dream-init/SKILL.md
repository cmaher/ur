---
name: brain:dream-init
description: Use when a brain's DREAM.md is missing or has no config block, `ur dream config` fails, brain:dream reports it cannot run, or the project's sources of truth changed — a new repo, a new ticket system, a different meaning of "done" — and the dream config needs creating or updating.
---

# Brain Dream Init

Builds or updates `<brain>/DREAM.md`, the only place `brain:dream` and `ur dream` learn anything
about this project. Format: `dream-md-schema.md` in the `brain:dream` skill's directory.

**Writes only under `<brain>`.** Never write project details into any skill file, repo file, or
anywhere outside `<brain>`.

## Brain root and tools

`<brain>` is `$UR_BRAIN` when set (ur sets it to `/brain` in workers that have a brain). Otherwise
it is the `brain` field of `ur dream path -p <key>`, where `<key>` is this skill's argument, or the
project in `ur project list` whose `repo` matches this directory's `origin` remote; if none or
several match, ask.

Run `ur dream` from your usual working directory, not from `<brain>` (pass `-p <key>` on the host;
in a worker it is pinned). Run git from inside `<brain>` (`cd <brain>` first), never `git -C`.

`<brain>` should be its own git repository (`<brain>/.git` exists); if not, tell the user to run
`brain:init` first.

## Instructions

1. **Read what exists**: `<brain>/DREAM.md` (if it has a config block, you are updating it — show
   the user what would change, don't silently rewrite; an older table-only `DREAM.md` or the init
   stub is converted: carry every value over, then confirm with the user), `<brain>/CLAUDE.md`,
   and each index it points to.
2. **Inspect, don't assume**:
   - Top-level folders of `<brain>` and what each holds → draft `[[folders]]`, with a role and a
     `promote_to` for scratch folders.
   - Scan brain text for ref-like strings: issue keys (`ABC-123` shapes), PR references
     (`org/repo#N`, PR URLs, bare `PR #N`), branch names, internal ticket ids. Group by shape and
     count them; each shape is a candidate pattern (several patterns can feed one kind).
   - Which commands exist in this environment (`command -v`, or `test -x` for an absolute path).
     Only propose check commands that exist; for others, ask the user how to check that kind. A
     kind nobody can check is still kept, with no `check`, so its refs get recorded as `unknown`.
   - Repos: the workspace's git remotes, plus repos that refs name. No local clone → local path
     `—`, and commit-message search is not a discovery source for that repo.
   - Generated or vendored files in the brain (headers like "generated", "do not edit", dump
     files) → draft `off_limits`.
3. **Ask the user**, one topic at a time, with your draft as the default:
   - For each ref kind: the patterns, the canonical `id`, the read-only check command (as argv),
     how to read the state from its output, and which states mean done and not done. Anything else
     is `unknown`.
   - Is discovery allowed, from which sources, and with what call budget?
   - Quiet days, and how many durable docs to review per run (default 1, intervals 30 days to
     start, between 7 and 180).
   - Anything that must never be edited.
4. **Write `<brain>/DREAM.md`**: the `## Config` TOML block plus the prose sections (repos,
   discovery sources). Values the user didn't give stay out — omit a key rather than guess.
5. **Validate**: `ur dream config` must succeed. Then, for each kind with a check, run
   `ur dream try <kind> <a real ref from the brain>` and `ur dream try <kind> <a made-up ref>`, and
   show both outputs before accepting the kind: the real one must come out done or not done, and
   the made-up one `unknown` (`done: null`), not "not done". Fix the config until both hold.
6. **Add the frontmatter rule** to `<brain>/CLAUDE.md` if absent: new scratch streams start with
   `status`, `refs` (using the ref kinds defined in `DREAM.md`), and `last-verified` frontmatter.
   Recording refs when work starts is what keeps `brain:dream` cheap.
7. **Commit** only the files you wrote: `git add DREAM.md CLAUDE.md` then
   `git commit -m "brain:dream-init: configure dream"`. Use the user's normal git config; never
   push.
8. **Report** what you wrote and suggest running `brain:dream` to see a first report. Don't fix
   brain content or backfill frontmatter on existing streams here — `brain:dream` proposes both.
   Mention problems you noticed so the user knows to expect them.

## Red flags

- Accepting a ref kind whose `ur dream try` outputs you haven't shown for a real and a made-up id.
- A check command that writes, comments, or changes state.
- Writing helper scripts into the brain: the mechanical work is `ur dream`'s.
- Copying ref patterns, repo names or commands into anything outside `<brain>`.
