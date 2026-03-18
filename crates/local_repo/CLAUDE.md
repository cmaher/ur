# local_repo

Async trait abstraction for local git repository operations (push, hooks, pool management). `GitBackend` implements the trait by routing `git` CLI commands through builderd's gRPC exec interface. All commands are executed on the host via `BuilderdClient` and results are parsed from command output.
