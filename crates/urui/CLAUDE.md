# urui (Terminal UI)

Standalone TUI binary for the Ur coordination framework. Connects to `ur-server` via tonic gRPC and presents an interactive terminal interface for managing tickets, flows, and workers.

## Architecture: TEA (The Elm Architecture)

The application follows the TEA pattern — Model, Update, View — with side effects expressed as commands:

- **Model** (`model.rs`): A single `Model` struct holds all application state. It is owned by the main loop and passed by value to `update`, which returns a new `Model`. Sub-models (`TicketListModel`, `FlowListModel`, etc.) hold per-page state. `LoadState<T>` tracks async data lifecycle (NotLoaded → Loading → Loaded/Error).
- **Msg** (`msg.rs`): An enum of every possible event — key presses, data arrivals, navigation, overlay interactions, operation results. All state changes flow through a `Msg`.
- **Update** (`update.rs`): A pure function `update(model, msg) -> (Model, Vec<Cmd>)`. No I/O — all side effects are expressed as returned `Cmd` values.
- **View** (`view.rs`): A pure function `view(&Model, &mut Frame, &TuiContext)` that renders the current model to a ratatui frame. No mutation.
- **Cmd** (`cmd.rs`): An enum of side effects (gRPC fetches, subscriptions, ticket/flow/worker operations, editor spawning, desktop notifications). The update function never performs effects directly.
- **CmdRunner** (`cmd_runner.rs`): Executes `Cmd` values asynchronously and feeds results back as `Msg` variants through an `mpsc` channel.

### TEA Loop (`main.rs`)

The `tea_loop` function drives the cycle:
1. A crossterm reader thread sends `Msg::KeyPressed` events.
2. A tick timer sends periodic `Msg::Tick` for housekeeping.
3. On each message: call `update(model, msg)` to get a new model and commands.
4. Execute commands via `CmdRunner`; results re-enter as new `Msg` values.
5. Render via `view(&model, frame, &ctx)`.

Editor spawning (`SpawnEditor`, `EditTicket`) breaks out of the TEA loop temporarily — the terminal is restored, `$EDITOR` runs, then the loop resumes with parsed results.

## Module Layout

- `main.rs` — Entry point: CLI parsing, terminal setup/teardown, TEA loop, editor flow handling
- `model.rs` — `Model` (all application state), sub-models, `LoadState<T>`, `UiEventThrottle`
- `update.rs` — Pure update function: `update(Model, Msg) -> (Model, Vec<Cmd>)`
- `view.rs` — Pure view function: renders model to ratatui frame
- `cmd.rs` — `Cmd` enum (side effects) and `FetchCmd` (gRPC fetch variants)
- `cmd_runner.rs` — Executes `Cmd` values, spawns async tasks, sends results as `Msg`
- `msg.rs` — `Msg` enum, `NavMsg`, `OverlayMsg`, `DataMsg`, operation request/result enums
- `input.rs` — `InputStack` and `InputHandler` trait (focus stack for key event dispatch)
- `navigation.rs` — `NavigationModel`, `TabId`, `PageId` (tab switching, page stack)
- `context.rs` — `TuiContext` (theme, keymap, project list, config — read-only rendering context)
- `terminal.rs` — Terminal setup (alternate screen, raw mode) and restore helpers
- `theme.rs` — `Theme` struct with ratatui `Color` fields, built-in theme loading from `build.rs`
- `keymap.rs` — `Action` enum and key-to-action resolution from config
- `notifications.rs` — `NotificationModel`, desktop notification tracking for workflow transitions
- `overlay_update.rs` — Overlay state update logic (priority picker, filter menu, goto, settings, etc.)
- `create_ticket.rs` — Ticket creation/edit editor flow: template generation, frontmatter parsing

### Pages (`pages/`)

Each page implements the `Component` trait and handles a specific view:

- `tickets_list.rs` — Ticket list with filtering, pagination, and bulk actions
- `ticket_detail.rs` — Single ticket view with children table, activities, and body
- `ticket_activities.rs` — Full-screen activities viewer with author filtering
- `ticket_body.rs` — Full-screen markdown body viewer with scrolling
- `flows_list.rs` — Workflow list with pagination and actions
- `flow_detail.rs` — Single workflow detail view
- `workers_list.rs` — Worker list with kill action

### Components (`components/`)

Reusable rendering components (not navigation pages):

- `header.rs` — Top tab bar with active tab indicator
- `footer.rs` — Bottom key-info bar showing available commands
- `banner.rs` — Success/error notification banner with auto-dismiss
- `status.rs` — Status message display in the header area
- `table.rs` — Generic table widget with selection, pagination, and column rendering
- `ticket_table.rs` — Ticket-specific table component (reused on list and detail pages)
- `overlay.rs` — Overlay rendering utilities (centered modal boxes)
- `priority_picker.rs` — Priority selection overlay (0-4)
- `type_menu.rs` — Ticket type selection overlay
- `filter_menu.rs` — Ticket list filter overlay (status, priority, project, children)
- `goto_menu.rs` — Cross-entity navigation overlay
- `force_close_confirm.rs` — Confirmation dialog for force-closing tickets with open children
- `create_action_menu.rs` — Post-editor action menu (create, dispatch, edit, abandon)
- `text_input.rs` — Generic text input overlay (parameterized title, reused for project key and branch input)
- `title_input.rs` — Text input overlay for ticket title
- `settings_overlay.rs` — Settings overlay with theme picker
- `progress_bar.rs` — Progress bar widget

## Key Conventions

- Terminal setup (alternate screen, raw mode) and teardown happen in `main.rs` to guarantee cleanup even on panic.
- All gRPC data fetching is async, executed by `CmdRunner`, and results arrive as `Msg::Data` variants.
- Theme colors are generated at compile time from `themes/themes.css` via `build.rs` (oklch to sRGB conversion).
- Config is loaded from `ur_config::Config` which reads `~/.ur/ur.toml`.
- UI event throttling: server push events mark tabs dirty; a cooldown window batches rapid-fire events into periodic re-fetches.

## Footer Command Ordering

Footer commands returned by `footer_commands()` must follow a consistent ordering for the left side (`common: false`):

1. **Capital-letter shortcuts** (Shift+key) in alphabetical order (A, C, D, O, P, S, V, X, ...)
2. **Lowercase-letter shortcuts** in alphabetical order (a, d, f, ...)
3. **Space** (always before other non-letter keys)
4. **Non-letter keys** (Enter, `*`, etc.)

The right side (`common: true`) contains navigation/system commands and is not subject to this ordering.

## Dependencies

- `ratatui` + `crossterm` for terminal rendering
- `tokio` for async runtime (rt-multi-thread, macros, time)
- `ur_config` for configuration loading (theme name, keymap, server port)
- `ur_rpc` with `retry` feature for gRPC server connection
- `ur_markdown` for markdown rendering in ticket body views
- `clap` for CLI argument parsing
- `chrono` for timestamp formatting
- `toml_edit` for persisting theme selection to `ur.toml`
