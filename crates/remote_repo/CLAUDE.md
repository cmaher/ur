# remote_repo

Async trait abstraction for remote repository operations (PRs, checks, comments). `GhBackend` implements the trait by routing `gh` CLI commands through builderd's gRPC exec interface. All commands are executed on the host via `BuilderDaemonServiceClient` and results are parsed from JSON.
