---
name: dispatch
description: Dispatch implementation workers for designed tickets.
---

# Dispatch Implementation Workers

Run `workertools workflow dispatch` to dispatch implementation workers for tickets that are ready.

## Steps

1. Run `workertools workflow dispatch` in bash.
2. If the command succeeds, report the result to the user.
3. If the command fails, report the error output to the user and suggest corrective action based on the error message.
