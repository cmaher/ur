## Prefer existing constants over literals

Before writing a string or numeric literal, check whether the codebase already derives that value from a named constant or enum. If a sibling function in the same file or package produces the value via something like `SomeEnum_MAGIC_VALUE.String()` or a declared `const`, use that instead of a bare `"MAGIC_VALUE"`. Hardcoded literals drift out of sync when the canonical value changes and hide the link to their source of truth.

## Hoist loop-invariant work

If a computation or validation inside a loop has the same inputs on every iteration, move it above the loop. It then runs once and reads as the precondition it actually is, instead of implying a per-element variation that does not exist.

## Match the surrounding code

New code should read like the code already around it. Mirror the naming, error-handling, and construction idioms of adjacent functions in the same file before introducing a different pattern.
