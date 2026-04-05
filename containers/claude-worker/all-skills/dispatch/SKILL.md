---
name: dispatch
description: |
  Triggers when user types /dispatch to launch implementation workers for designed tickets.
  TRIGGER when: user says /dispatch or asks to dispatch implementation workers for tickets that have been designed.
  DO NOT TRIGGER when: user wants to design, implement directly, or manage tickets manually.
---

# Dispatch Implementation Workers

Run `workertools workflow dispatch` to dispatch implementation workers for tickets that are ready.

## Steps

1. Run `workertools workflow dispatch` in bash.
2. If the command succeeds, report the result to the user.
3. If the command fails, report the error output to the user and suggest corrective action based on the error message.
