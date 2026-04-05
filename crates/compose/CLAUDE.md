# compose

Typed Docker Compose model crate. Models Docker Compose files as Rust structs serialized via `serde_yaml`.

- `model.rs` — `ComposeFile`, `Service`, `Network`, `Healthcheck`, `DependsOn` structs with `IndexMap` for deterministic output
- `manager.rs` — `ComposeManager` wrapping `docker compose` CLI (up/down/is_running/recreate_service)
- `ComposeFile::base()` produces the base compose configuration for ur infrastructure services
- `ComposeFile::render()` serializes to YAML via serde_yaml
- All types derive `Serialize`, `Deserialize`, `Clone`, `Debug`
- Uses `IndexMap` (not `HashMap`) for deterministic YAML key ordering
