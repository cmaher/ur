After all subagents complete, run full CI verification before setting the epic's `pr_summary`:

1. Run `cargo make ci-fmt-fix` (silently auto-formats, then runs clippy, build, tests). Fix any failures and re-run until clean.
2. `ci-fmt-fix` may have reformatted files. Check for unstaged changes and, if any, stage and commit them before proceeding.

If a failure is in code from a subagent's ticket, fix it directly rather than re-dispatching.
