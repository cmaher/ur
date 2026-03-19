Before dispatching subagents, verify the codebase starts clean.

1. Read `.bacon-locations` in the workspace root.
2. If the file exists and has `error` or `warning` lines, fix them before dispatching.
3. If `.bacon-locations` does not exist or is empty, fall back to:
   ```
   cargo make clippy
   ```
   Fix any issues before dispatching.

Do NOT dispatch subagents into a broken codebase.
