# ur-o15mo / ur-m9o5z: worker-callable session control — policy and trust model

Epic ur-o15mo lets an agent drive other agents' sessions. Batch A (ur-6wkmt) gave the host CLI
`ur worker read|send|keys|interrupt` and `--wait-idle`. Batch B (ur-pkl6j: ur-m9o5z, ur-ucb4f,
ur-9zj01, ur-0lq89) lets an agent running *inside* a ur worker (container or local, any agent
type) do the same to other workers, under the policy below. This page records decisions already
made; it does not reopen them.

## Trust model: a guardrail, not a security boundary

The policy stops well-behaved agents from driving the wrong workers by accident through the
sanctioned tool path (`ur-workertools worker ...`). It does **not** stop a hostile or
prompt-injected worker. Container workers already have host-user code execution through the
default hostexec build tools and can read container env with `docker inspect` (isolation is
ur-sv5ny), so any server-side check can be bypassed. Not defended:

- hostile or prompt-injected workers;
- direct calls to a peer's workerd (workerd has no inbound auth) or to the host gRPC port
  (no auth; the host-token attempt was reverted, see ur-8q8gi);
- abuse between same-project peers;
- secrets visible on a peer's screen or transcript.

No workerd inbound auth and no host-port auth are added in this batch.

## Policy

| Rule | Decision |
|------|----------|
| Caller identity | The existing worker auth interceptor (`ur-worker-id` + `ur-worker-secret`). The handler loads the caller's row by that id; an unknown caller is `Unauthenticated`, never a fallback to allow. |
| Who may drive | Only a caller whose stored worker row has strategy `manual` (`-m manual`, or a custom mode with `base = "manual"`). Read from the server's row, never from the request. Everyone else gets `PermissionDenied: only manual workers may drive other workers`. |
| Targets | Running workers in the caller's project (by process id or full worker id). Anything else, including another project's worker, is `NotFound` with the same message as a missing worker. A caller with no project sees no peers. |
| Worker-callable verbs | `list`, `read` (transcript and `--screen`), `send` (submit / `--no-submit` / `--wait-idle`), `keys`, `interrupt`, `wait-idle`. Launching or stopping peers through `ur-workertools` is not added (follow-up ur-ge03l). |
| `WorkerStop` (worker port, ur-9zj01) | Any worker may stop itself (workerd's exit sink does this). Stopping another worker needs a manual caller and a same-project target. |
| `WorkerLaunch` (worker port, ur-9zj01) | The launch's effective project must be the caller's project. Manual callers may launch; design callers keep the `ur-workertools workflow dispatch` path (see below); code callers are denied. |
| Host CLI | Unchanged: full access on the host port. |
| Agent guidance | The `drive-workers` skill (`containers/worker-base/potential-skills/drive-workers/`), installed for manual-strategy workers only: send → wait-idle → read, `--screen` when stuck, never answer a peer's permission prompts. |
| Workflow interaction | Allowed. A peer can type into a worker mid-step (e.g. while Implement is in flight); the workflow coordinator is not told. Known limit. |
| Loop / runaway | None beyond the audit log (follow-up ur-5f3gm: depth and fan-out limits). |
| Audit | Every worker-initiated call logs caller worker id, target, and verb at info level (`worker peer access`). |

Existing self-directed worker calls are unaffected: workerd's self-stop on agent exit sends
`WorkerStop` with its own worker id, and design workers' `workflow dispatch` reaches the
server as workerd's `WorkerLaunch` for the dispatched ticket. That launch is another worker,
but the dispatch flow predates this policy and the default design mode depends on it, so
design callers keep same-project launch.

## Flow

```
agent (in worker A)
  │ ur-workertools worker read B           ($UR_SERVER_ADDR, ur-worker-id/secret headers)
  ▼
ur-server worker port ── worker_auth_interceptor: id/secret match a worker row?  else Unauthenticated
  │
  ▼
WorkerCoreServiceHandler ── PeerAccess (crates/server/src/grpc_peer.rs), shared by every verb
  │   1. load caller row by ur-worker-id                       else Unauthenticated
  │   2. caller strategy == manual (self-stop / design launch)  else PermissionDenied
  │   3. target running + same project                         else NotFound
  │   4. info!(caller, target, verb)
  ▼
grpc_session.rs shared verb bodies (same validation/defaults as the host path)
  │   WorkerdAddressResolver::tracked_client_for(target)
  ▼
target workerd (container) / builderd routing to the local worker's daemon
```

Every failure is a gRPC `Status` that `ur-workertools` prints (`worker: <message>`, or the
`ur` JSON error envelope with `--output json`) with `ur`'s non-zero exit codes; a timed-out
wait exits 10.

## Proto changes

None. The worker port reuses the host-facing messages (`WorkerList`, `CaptureWorkerPane`,
`ReadWorkerTranscript`, `SendWorkerMessage`, `SendWorkerKeys`, `InterruptWorker`,
`WaitWorkerIdle`); `WorkerCoreServiceHandler` now implements them instead of answering
`Unimplemented`. Workers are addressed by opaque worker-id strings.

## Known limits

- Guardrail only (above). Hostexec `ur worker list|describe|dir` still reaches the unfiltered
  host port (follow-up ur-yr6vq).
- A peer can drive a worker in the middle of a workflow step.
- No depth or fan-out limits: two manual workers can drive each other indefinitely.
- Screens and transcripts can contain secrets; any same-project manual worker can read them.

## Next: cross-machine

The next use case is driving workers on other machines (design ticket ur-mtw62). Nothing here
builds it, and the types stay transport-neutral (worker ids are opaque strings, nothing assumes
the target lives on this server). Before it ships:

- Network exposure makes real authentication and TLS mandatory on the host-facing path. The
  reverted host-token change (59f9d83) is a starting point.
- The manual-only and same-project rules key on the local worker row. Across machines they need
  an identity story: who vouches for a remote caller's mode and project, and how a remote
  target's project is known.
