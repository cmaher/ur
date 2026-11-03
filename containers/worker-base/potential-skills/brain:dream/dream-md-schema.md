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
```

## Repos

| Name | Remote | Local path |
|---|---|---|
| `example-repo` | `example-org/example-repo` | `/workspace` |

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
Durable docs may carry `last-verified` and `canonical-for`.

## `<brain>/.dream/` layout

Committed to the brain's git repository. There are no snapshot copies; git history is the
checkpoint.

```
<brain>/.dream/
  state.json              written by `ur dream check --write` and `ur dream reviewed`:
                            refs:    { "<kind>:<id>": { state, done, checked_at } }
                            reviews: { "<path>": { last_reviewed, interval_days,
                                                   history: [{ date, outcome, note }] } }
                            last_run
  reports/YYYY-MM-DD.md   proposed change sets, plus an "Applied" section once applied
```

`reviews` is the durable-doc review log. An `unchanged` review doubles the doc's interval and a
`changed` one halves it, within `[min_days, max_days]`: docs that keep matching reality are
reviewed less often, and `changed_reviews` in `ur dream review` shows which ones drift.
