---
name: trim
description: Use when a change is done but before it ships, to find code to delete or replace with code that already exists: helpers that duplicate an existing package, pass-through wrappers, guards and knobs nobody needs, copy-pasted test setup, and stale docs. Reports findings by default; pass `apply` to make the cuts.
---

# trim: make the diff smaller

## Overview

A trim pass looks at a finished diff and asks one question: **what can go?** Code is cut by deleting it, by replacing
it with code the repo or platform already has, or by folding copies into one. A trim pass does not hunt bugs or add
guards. Use `code-review` for that.

**Core principle:** every line the diff adds has to earn its place against what already exists. The most valuable
finding is freshly written code that duplicates something the codebase already provides, because the diff can't show
it. You have to go and look.

## Invocation

Parse `$ARGUMENTS` for:

- a range (`<base>..<head>`, a PR number, or a branch). The default is the working branch against its merge base
  with the default branch.
- `apply`: make the accepted cuts, verify, and commit. Without it, report only.
- any scope contracts you were given. Keep every guarantee they list; a cut that breaks one is not a trim.

## Step 1: Search for an existing home (required)

The diff alone can't show duplication of code that lives elsewhere. For each new function, type, interface or
constant the diff adds, list what it depends on (a client, an encoding, a status enum, a config type, a test fake).
Then search for code that already wraps that dependency:

1. **Shared packages for the dependency.** Look for a common, shared or util package built around the same client
   or library: `rg -l '<DependencyType>' --type <lang>`, then read the packages that import it most. A new
   interface with the same methods as an existing client wrapper, or a builder that encodes the same request,
   belongs in that wrapper.
2. **The same package and its siblings.** Helpers one directory over often already do the job.
3. **The test file itself.** A helper defined in the file but bypassed by inline copies is a finding.
4. **The platform.** Stdlib, framework, database or proxy features the code re-implements.

Record each hit as "new `X` duplicates existing `Y` at `path:line`". When the existing home is missing a small
piece (a method, an exported helper), the trim is to **add it there and use it**, not to keep a private copy.

## Step 2: Cut catalog

Check the diff against each category.

| Category | What to look for | Usual cut |
| --- | --- | --- |
| Duplicate of existing code | Step 1 hits: a new interface, builder, mapper or helper that matches one in a shared package | Use the existing one; move the missing piece there |
| Pass-through constructor or wrapper | A function whose body only copies its parameters into a struct literal or forwards them to one call (`newX(a, b, c) *X { return &X{a: a, b: b, c: c} }`), with no defaulting, validation or derivation | Inline it at its caller. Tests build the struct literal |
| One-implementation abstraction | An interface, option type or strategy with one implementation and no planned second one | Use the concrete type |
| Platform already does it | Re-implemented stdlib, framework, DB or proxy behavior | Delete it and rely on the platform |
| Guards for one root cause | Several checks that together compensate for one missing invariant | Fix the root cause once; drop the guards |
| Unneeded knobs | Config fields, flags or parameters with one value in practice | Make it a constant |
| Duplication across units | The same logic in two files or units of the change | One implementation, called from both |
| Dead code | Symbols, branches, imports or parameters nothing reaches after the change | Delete |
| Copy-pasted test setup | The same prelude (arrange plus first action) repeated 3+ times | One fixture helper, or a table test when cases differ only in inputs and expectations |
| Bypassed test helper | A helper exists, but some tests inline what it does | Use the helper everywhere |
| Duplicate tests | Two tests with the same setup and the same path, where one adds only an assertion | Merge them, keeping the union of assertions |
| Stale docs | Doc comments, READMEs or the PR description claiming behavior the code no longer has | Delete or correct the claim |

Not a trim: renames, reformatting, style preferences, or anything that changes behavior a contract guarantees.

## Step 3: Report

Number each finding, most lines saved first:

```
### #1 · duplicate · `path/to/feature/client.go:23-40`

**`fooClient` and its private interface re-implement the wrapper in `path/to/common/foo`.**

Add the one missing method to the shared wrapper, hold that wrapper instead, and delete the private interface and
request builder. Saves ~40 lines here.
```

Each finding names the code to remove, what replaces it (an existing symbol with its path, or "nothing"), and roughly
how many lines it saves. End with a total of the lines you expect to cut.

## Step 4: Apply (only with `apply`)

1. Make the cuts, smallest blast radius first. Move shared pieces before switching callers.
2. Run the project's verification (its CLAUDE.md, or the hooks below), plus the tests for every package you touched.
3. Commit once with a message listing each cut. Don't push.

@%UR_SKILL_HOOKS%/implement/subtask-verifications.md

## Red flags

| Thought | Reality |
| --- | --- |
| "The diff looks self-contained" | Duplication of existing code is invisible in the diff; run Step 1 |
| "It's only a few lines of wrapper" | A pass-through wrapper is a second name for the same thing; inline it |
| "The shared package lacks this, so keep a private copy" | Add the missing piece to the shared package |
| "Each test should be readable on its own" | Many copies of one prelude hide what each test changes; use a helper or a table |
| "While I'm here, I'll fix this bug" | Report it for `code-review`; trim only cuts |
| "This guard is cheap" | Ask what root cause it compensates for |

$ARGUMENTS
