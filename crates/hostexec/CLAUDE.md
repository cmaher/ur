# hostexec

Hostexec policy library: decides whether a worker's host-exec request is allowed and what builderd runs. Depends only on `ur_config` and `ur_rpc` — never on the server, `ticket_db` or `workflow_db`, so a per-machine edge can enforce the same policy.

- `config` — `HostExecConfigManager` (built-in commands, embedded `default_scripts/*.lua`, `$UR_CONFIG/hostexec/` overrides)
- `discovery` — `LuaDiscoveryManager` (finds and validates user Lua transforms)
- `lua_transform` — `LuaTransformManager` sandbox, `WorkerContext`, `TransformResult`
- `script_registry` — `ScriptRegistry` (per-project `hostexec_scripts` allowlist)
- `shim` — `materialize_shim`
- `paths` — container CWD / script path / `/brain` mapping to builderd paths
- `error` — `HostExecError` and its `tonic::Status` mapping
- `forward` — `forward_to_builderd` (streams a transformed request to builderd)

The gRPC handler (`HostExecServiceHandler`) and worker request-context resolution stay in `crates/server/src/grpc_hostexec.rs`. See `docs/codeflows/host-exec-flow.md`.
