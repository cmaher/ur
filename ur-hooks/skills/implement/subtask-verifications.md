Before committing, verify the codebase has no compiler errors or warnings.

1. Read `.bacon-locations` in the workspace root.
2. If the file exists and is non-empty, check every line for `error` or `warning` kinds.
   - If ANY errors or warnings are present, fix ALL of them before proceeding.
   - Re-read `.bacon-locations` after fixes to confirm it is clean.
3. If `.bacon-locations` does not exist or is empty, fall back to running:
   ```
   cargo make clippy
   ```
   Fix any errors or warnings reported, then re-run to confirm clean output.

Do NOT commit until all diagnostics are clean.
