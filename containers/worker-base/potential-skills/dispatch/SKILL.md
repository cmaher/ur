---
name: dispatch
description: Dispatch implementation workers for designed tickets.
---

# Dispatch Implementation Workers

Run `workertools workflow dispatch` to dispatch implementation workers for tickets that are ready.

## Steps

1. Run `workertools workflow dispatch` in bash.
2. If the command succeeds, report the result to the user.
3. If the command fails with "No ticket set":
   a. Find the ticket ID from the current git branch: run `git branch --show-current` to get the branch name.
   b. The branch name follows the convention `{project}-{hash}-{suffix}` (e.g., `ur-abc12-xyzw`). Extract the ticket ID as the first two segments joined by a hyphen (e.g., `ur-abc12`).
   c. Verify the ticket exists: run `ur ticket show <extracted-id> --output json`. If it doesn't exist, report the error to the user and stop.
   d. Set the ticket: run `workertools workflow set-ticket <extracted-id>`.
   e. Retry: run `workertools workflow dispatch`.
   f. If the retry fails, report the error to the user.
4. If the command fails with any other error, report the error output to the user and suggest corrective action based on the error message.
