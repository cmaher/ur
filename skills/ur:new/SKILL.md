---
name: ur:new
description: Use when starting a brand-new project that ur should manage — creates the local git repo, a private GitHub repo, the ur project entry and the project's brain, and ties them together. Also use when an existing directory or repo has only some of those pieces and the rest need adding.
---

# ur: New Project

Sets up everything a new ur-managed project needs, in an order where each step can read what the
previous one made:

```
local repo  ->  GitHub repo (origin)  ->  ur project entry  ->  brain  ->  dream config
```

Every step is **additive**: check whether the piece already exists, leave it alone if it does, and
create it only if it is missing. Never overwrite a file, re-initialize a repo, or replace a remote.

This skill sets up the project's plumbing. It does not scaffold application code.

## 1. Settle the facts

Ask only for what the request doesn't already give; use the default otherwise.

| Fact | Default |
|------|---------|
| Name | none — must come from the user |
| Directory | `<name>` beside the user's other project clones |
| GitHub owner | the account `gh auth status` reports |
| Visibility | private. Public only when the user says so |
| ur key | the repo name (what `ur project add` derives). Ticket ids are prefixed with it, so a short key is worth asking about when the name is long |
| Default branch | `main` |
| Worker image | `ur-worker` |

Then check nothing is in the way, and stop to ask if anything is:

```bash
ls <dir>                               # should not exist, or be the directory to adopt
gh repo view <owner>/<name>            # should fail: "Could not resolve to a Repository"
ur project list --output json          # no project with this key or this repo
```

## 2. Local repo

```bash
git -C <dir> init -b main
```

Write a `README.md` (the name and one or two sentences on what it is) and a `.gitignore` for the
stack, then commit just those. If the directory is already a git repo with commits, skip this step.

## 3. GitHub repo

```bash
gh repo create <owner>/<name> --private --source <dir> --remote origin --push \
  --description "<one line>"
```

This creates the repo, adds it as `origin` and pushes the current branch. If `origin` already
exists, do not create or change anything: confirm it is the repo the user means and move on.

## 4. ur project

`ur project add` reads the directory's `origin` remote, so it runs after step 3.

```bash
ur project add <dir> --output json                     # key derived from the repo name
ur project add <dir> --key <key> --name "<display>"    # when the key differs from the name
```

It writes `[projects.<key>]` to `ur.toml` and reloads the server. For anything beyond the defaults
(mounts, hostexec grants, pool limit, theme), edit that entry afterwards: see the `ur:config` skill.

Read the brain path back rather than assuming it:

```bash
ur project list --output json          # this project's "brain" field
```

## 5. Brain

Run the `brain:init` skill with the project key, then `brain:dream-init`. Follow them as written.
Points that matter for a brand-new project:

- The brain is its own git repository, separate from the project repo, and gets no remote.
- Say in the brain's top-level `CLAUDE.md` what the project is, where its clone lives and where its
  remote is.
- A new brain has no refs to scan, so `brain:dream-init` has little to inspect. Draft the config
  from the project's facts (the `<key>-` ticket prefix, the one repo, its default branch) and
  confirm it with the user.
- `ur dream try <kind> <made-up ref> -p <key>` must come out `unknown`. A real ref can only be
  tried once the project has a ticket; say so in the report instead of skipping it silently.

If the conversation already produced decisions about the project (its purpose, stack, a design),
write them into the brain now, in `working/` unless the user confirmed them as settled, and add each
file to that folder's index.

## 6. Report

State what was created and what was already present: the directory, the GitHub URL and its
visibility, the ur key, the brain path and the commits made in it. List anything left undone and
why.

## Running under a command sandbox

Several of these commands write outside the working directory or need stored credentials, and fail
inside a sandbox with "Operation not permitted" or an auth error:

| Command | Why it needs to run outside the sandbox |
|---------|------------------------------------------|
| `git init` | copies hook templates into the new `.git` |
| `gh ...` | reads the stored GitHub login |
| `ur ...` | writes its log file and `ur.toml` |
| git inside the brain | the brain lives under the ur config directory |

## Red flags

- Creating a public repo, or pushing to an existing remote, without the user saying so.
- Running `ur project add` before `origin` exists: the entry gets no repo.
- Adding a remote to the brain, or putting brain files in the project repo.
- Writing application code or a build scaffold. That is a separate task, after the design.
- Overwriting an existing README, `.gitignore`, `ur.toml` entry or brain file.

$ARGUMENTS
