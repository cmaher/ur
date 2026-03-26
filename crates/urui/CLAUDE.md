# urui (Terminal UI)

Standalone TUI binary for the Ur coordination framework. Connects to `ur-server` via tonic gRPC and presents an interactive terminal interface for managing tickets, flows, and workers.

## Module Layout

- `main.rs` — Entry point: loads config, sets up/tears down terminal, runs the app loop
- `app.rs` — Top-level application struct and main event loop
- `context.rs` — `TuiContext` holding resolved theme, keymap, and gRPC channel
- `event.rs` — `EventManager` and `AppEvent` enum (key, tick, data-ready, resize)
- `theme.rs` — `Theme` struct with ratatui `Color` fields, built-in theme loading from build.rs
- `keymap.rs` — `Action` enum and key-to-action resolution from config
- `page.rs` — `TabId`, `FooterCommand`, `Banner`, `BannerVariant`, `StatusMessage`
- `screen.rs` — `Screen` trait, `ScreenResult` enum
- `data.rs` — `DataPayload` and async gRPC data-fetching helpers
- `pages/` — Individual page implementations (tickets, flows, etc.)
- `widgets/` — Reusable ratatui widget components

## Key Conventions

- Terminal setup (alternate screen, raw mode) and teardown happen in `main.rs` to guarantee cleanup even on panic.
- All gRPC data fetching is async and delivered via `AppEvent::DataReady` through the event channel.
- Theme colors are generated at compile time from `themes/themes.css` via `build.rs` (oklch to sRGB conversion).
- Config is loaded from `ur_config::Config` which reads `~/.ur/ur.toml`.

## Dependencies

- `ratatui` + `crossterm` for terminal rendering
- `tokio` for async runtime (rt-multi-thread, macros, time)
- `ur_config` for configuration loading (theme name, keymap, server port)
- `ur_rpc` with `retry` feature for gRPC server connection
