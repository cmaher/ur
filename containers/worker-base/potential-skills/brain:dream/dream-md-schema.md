# DREAM.md format

`<brain>/DREAM.md` holds everything project-specific that `brain:dream` needs: prose for the agent,
and one ```` ```toml ```` config block that `ur dream` reads. The skill and `ur dream` know nothing
about any project; if a fact about repos, ticket systems, or "done" is not in this file, they don't
know it.

Every value below is a placeholder. Real values are written by `brain:dream-init` into the brain,
never into a skill.

`ur dream config` validates the block (unknown keys, bad regexes, templates a pattern can't fill)
and prints it. Run it after every edit.

````markdown
# Dream config

## Config

```toml
off_limits = ["reference/generated/**"]      # globs; dream never proposes edits here

[[folders]]
path = "reference"
role = "durable"          # durable | scratch | episodic | procedure | orientation

[[folders]]
path = "working"
role = "scratch"
promote_to = "reference"

[[folders]]
path = "investigation"
role = "episodic"

[[ref_kinds]]
kind = "ticket"
id = "{key}"                                   # canonical id, from pattern groups
patterns = ['\b(?P<key>EXAMPLE-\d+)\b']
check = ["example-tracker", "show", "{key}"]   # read-only argv; {group} placeholders
state = '^status: (\w+)$'                      # regex over stdout; group 1 is the state
done = ["closed"]
not_done = ["open", "in_progress"]

[[ref_kinds]]
kind = "pr"
id = "{repo}#{n}"
check = ["example-forge", "pr-state", "example-org/{repo}#{n}"]
done = ["MERGED"]                              # no `state`: the first line of stdout
not_done = ["OPEN"]

[[ref_kinds.patterns]]
regex = '\bexample-org/(?P<repo>[\w-]+)#(?P<n>\d+)'

[[ref_kinds.patterns]]
regex = '\bPR #(?P<n>\d+)'                     # bare "PR #N" means the main repo
set = { repo = "example-repo" }

[[ref_kinds]]
kind = "branch"
patterns = ['\bexample-repo:feature/[\w-]+']   # no check: recorded, always `unknown`

[unclassified]
pattern = '\b[A-Z][A-Z0-9]{1,9}-\d+\b'         # ref-shaped strings no kind claims
ignore = ["ISO", "RFC"]

[discovery]
allowed = true
max_calls = 20

[thresholds]
quiet_days = 14      # days without change before `ur dream candidates` calls a stream quiet

[review]
per_run = 1          # durable docs reviewed per dream run
initial_days = 30
min_days = 7
max_days = 180

[[repos]]             # source repos durable docs are reviewed against
name = "example-repo"
path = "~/src/example-repo"   # host path of a clone: absolute, ~/, or relative to the brain
branch = "origin/master"      # default; changes are listed up to it, never fetched
```

## Repos

| Name | Remote | Worker path |
|---|---|---|
| `example-repo` | `example-org/example-repo` | `/workspace` |

Names match `[[repos]]` in the config block, which holds each clone's host path. A repo `name`
is letters, digits, `_` and `-` only (the key form `sources` frontmatter items take); anything
else fails `ur dream config`.

## Discovery sources

Text scan, and commit-message search in repos with a local path.
````

## Ref kinds

- Patterns use Rust `regex` syntax: no lookaround or backreferences. Named groups are
  `(?P<name>...)`.
- Patterns are tried in config order; a match overlapping an earlier match is skipped, so list
  specific patterns (a full URL) before looser ones (a bare `PR #N`). Keep patterns narrow: a broad
  pattern like `\w+:\w+` matches ordinary prose.
- `id` and every `check` placeholder must be filled by each pattern's named groups or its `set`.
- `check` is an argv, run without a shell. Don't put `{group}` inside an `sh -c` string: group
  values come from brain text. If a shell is needed, pass values as positional arguments
  (`["sh", "-c", "tool \"$1\"", "_", "{key}"]`).
- In `state`, `{group}` is replaced by the escaped group value, e.g. `'^{key}\s+\S+\s+(.+)$'` for
  output that starts with the id. Other braces (`\d{4}`) are left alone.
- The result is done only if the state is listed in `done`, not done only if in `not_done`.
  Anything else — another state, a non-zero exit, a timeout, a missing command, no check — is
  unknown (`done: null`; `state` says why, e.g. `uncheckable`, `exit-1`) and stays for a human to
  judge. A kind with no check still gets its refs recorded.
- Check commands run on the host: from a worker, `ur dream` runs through hostexec.

## Stream frontmatter

Each scratch stream's entry file (its `README.md`, or the single file if the stream is one file)
carries:

```yaml
---
status: active        # active | done | archived
refs:
  - ticket: EXAMPLE-123
  - pr: example-org/example-repo#45
ignore_refs:          # optional: ref-shaped text here that is not a ref
  - ticket: EXAMPLE-999
last-verified: 2026-01-15
canonical-for: [example-topic]   # optional
---
```

Recorded ref values are parsed with the kind's patterns, so any form a pattern matches works.
`ignore_refs` entries use the same form; a matching discovered ref is left out of
`discovered_refs` (listed under `ignored_refs` instead) and is never checked. Ignoring is per
stream: the same ref discovered in another stream is still reported there.
Durable docs may carry `last-verified` and `canonical-for`, and `sources`: the paths in each repo
the doc describes, so `ur dream review`'s `changes_since` lists only changes under them.

```yaml
---
sources:              # optional; a repo with no entries is diffed whole
  - example-repo: crates/server
  - example-repo: docs/architecture.md
---
```

## Repos and review commits

`ur dream reviewed <file>... unchanged|changed --at <repo>=<sha>` records the commit of each
`[[repos]]` entry the docs were checked against. Each `--at` is resolved to a full commit in the
repo's `path` when recorded; an unknown repo name, a missing clone, or a sha that isn't a commit
there fails the command and records nothing.

`ur dream review` then gives, for each picked doc with a recorded commit, per repo:
`changes_since.<file>.<repo> = { since, reviewed_on, branch, sources, commits, paths }`, from
`git rev-list --count` and `git log --name-only` over `<since>..<branch>` (limited to `sources`)
in the clone. `since` is the newest review that recorded that repo. Nothing is fetched, so fetch
the clone first if it should be current. If a repo's diff fails (clone gone, sha no longer
present, repo removed from the config, a `sources` path that exists at neither `since` nor
`branch`) that repo's `commits` and `paths` are `null` and `error`
says why; the rest of the output is unaffected.

## `<brain>/.dream/` layout

Committed to the brain's git repository. There are no snapshot copies; git history is the
checkpoint.

```
<brain>/.dream/
  state.json              written by `ur dream check --write` and `ur dream reviewed`:
                            refs:    { "<kind>:<id>": { state, done, checked_at } }
                            reviews: { "<path>": { last_reviewed, interval_days,
                                                   history: [{ date, outcome, note,
                                                              at: { <repo>: <sha> } }] } }
                            last_run
  reports/YYYY-MM-DD.md   proposed change sets, plus an "Applied" section once applied
```

`reviews` is the durable-doc review log. An `unchanged` review doubles the doc's interval and a
`changed` one halves it, within `[min_days, max_days]`: docs that keep matching reality are
reviewed less often, and `changed_reviews` in `ur dream review` shows which ones drift.
