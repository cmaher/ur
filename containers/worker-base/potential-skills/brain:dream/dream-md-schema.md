# DREAM.md format

`<brain>/DREAM.md` holds everything project-specific that `brain:dream` needs. The skill itself knows
nothing about any project; if a fact about repos, ticket systems, or "done" is not in this file, the
skill does not know it.

Every value below is a placeholder. Real values are written by `brain:dream-init` into the brain,
never into a skill.

````markdown
# Dream config

## Folders

| Path | Role | Promote to |
|---|---|---|
| `evergreen/` | durable | — |
| `working/` | scratch | `evergreen/` |
| `investigation/` | episodic | — |
| `procedures/` | procedure | — |

Roles: `durable` (reference facts, kept current), `scratch` (in-progress streams, one folder each),
`episodic` (dated, immutable records), `procedure` (step-by-step instructions), `orientation`
(short read-start-to-finish overviews). Dream never invents a folder that is not in this table.

## Ref kinds

| Kind | Pattern | Check command | Done when | Not done when |
|---|---|---|---|---|
| `ticket` | `EXAMPLE-\d+` | `example-tracker show {id}` | output contains `status: closed` | output contains `status: open` or `status: in_progress` |
| `pr` | `example-org/[\w-]+#\d+` | `example-forge pr-state {id}` | output is `MERGED` | output is `OPEN` |
| `branch` | `example-repo:feature/[\w-]+` | — | — | — |

`{id}` is replaced with the ref's id. A check command must be read-only; it may be an absolute
path. Any result matching neither column, a check of `—`, or a command not available in the current
environment → the ref is `unknown` (so a closed-unmerged PR above is `unknown`, for a human to
judge). Keeping an uncheckable kind still lets its refs be recorded. Keep patterns narrow: a broad
pattern like `\w+:\w+` matches ordinary prose.

## Repos

| Name | Remote | Local path |
|---|---|---|
| `example-repo` | `example-org/example-repo` | `/workspace` |

## Discovery

- allowed: yes
- sources: text scan, commit-message search in listed repos
- max calls per run: 20

## Thresholds

- quiet days: 14
- stale verify days: 90

## Off limits

- `evergreen/generated/**`
````

## Stream frontmatter

Each scratch stream's entry file (its `README.md`, or the single file if the stream is one file)
carries:

```yaml
---
status: active        # active | done | archived
refs:
  - ticket: EXAMPLE-123
  - pr: example-org/example-repo#45
last-verified: 2026-01-15
canonical-for: [example-topic]   # optional
---
```

Durable docs may carry `last-verified` and `canonical-for`.

## `<brain>/.dream/` layout

Committed to the brain's git repository. There are no snapshot copies; git history is the
checkpoint.

```
<brain>/.dream/
  state.json              { "refs": { "<kind>:<id>": { "state": "...", "done": bool,
                                                       "checked_at": "YYYY-MM-DD" } },
                            "last_run": "YYYY-MM-DD" }
  reports/YYYY-MM-DD.md   proposed change sets, plus an "Applied" section once applied
```
