# urui v2 TEA Architecture

## Overview

The v2 TUI uses The Elm Architecture (TEA) to manage all state and side effects. The core loop is: receive a `Msg`, pass it with the current `Model` to a pure `update` function that returns a new `Model` and a list of `Cmd` values, execute those commands via the `CmdRunner`, and render the new model with the `view` function. All state lives in `Model`; all mutations flow through `update`; all I/O is expressed as `Cmd` values.

## TEA Loop

```
                    ┌──────────────────────────────────────┐
                    │         tea_loop (mod.rs)             │
                    │                                      │
                    │  msg_rx.recv()                        │
                    │      │                                │
                    │      ▼                                │
                    │  ┌──────────┐                         │
                    │  │  update  │  pure: (Model, Msg)     │
                    │  │          │  → (Model, Vec<Cmd>)    │
                    │  └────┬─────┘                         │
                    │       │                               │
                    │       ├── model = new_model           │
                    │       │                               │
                    │       ▼                               │
                    │  ┌──────────────┐                     │
                    │  │  CmdRunner   │  execute_all(cmds)  │
                    │  │  (impure)    │  spawns async tasks  │
                    │  └──────┬───────┘                     │
                    │         │                             │
                    │         │  sends Msg back via msg_tx  │
                    │         │                             │
                    │         ▼                             │
                    │  ┌──────────┐                         │
                    │  │   view   │  pure: (&Model, Frame)  │
                    │  │          │  draws widgets           │
                    │  └──────────┘                         │
                    └──────────────────────────────────────┘
```

### Startup

1. `run_v2()` sets up the terminal and installs a panic hook that restores it.
2. `tea_loop()` loads config, creates the message channel (`mpsc::unbounded_channel<Msg>`), and builds a `CmdRunner`.
3. Two background tasks are spawned:
   - **Crossterm reader** (`spawn_blocking`): polls for terminal key events every 50ms, sends `Msg::KeyPressed`.
   - **Tick timer** (`tokio::spawn`): sends `Msg::Tick` every 5 seconds for periodic housekeeping.
4. The initial `Model` is created via `Model::initial()`, which pushes a `GlobalHandler` onto the input stack.
5. A `Cmd::SubscribeUiEvents` is executed to connect to the server's event stream.
6. The loop enters: `while let Some(msg) = msg_rx.recv().await`.

### Loop Iteration

Each iteration:
1. Receive a `Msg` from the channel.
2. Call `update(model, msg)` to get `(new_model, cmds)`.
3. If `model.should_quit` is true, break.
4. Call `cmd_runner.execute_all(cmds)` to run side effects.
5. Call `terminal.draw(|frame| view(&model, frame))` to render.

## Msg Design

All state changes flow through `Msg` variants. The `update` function pattern-matches on them to produce a new `Model` and `Cmd` list.

| Variant | Source | Purpose |
|---------|--------|---------|
| `KeyPressed(KeyEvent)` | Crossterm reader task | Terminal keyboard input |
| `Tick` | Tick timer task | Periodic housekeeping (throttle flush) |
| `Quit` | Input handlers, update logic | Request application exit |
| `Data(Box<DataMsg>)` | CmdRunner fetch tasks | Async data arrived from gRPC |
| `Nav(NavMsg)` | Input handlers | Tab switching, page push/pop |
| `UiEvent(Vec<UiEventItem>)` | CmdRunner UI event stream | Server-side data changed |

### DataMsg

Carries results from gRPC fetches. Each variant corresponds to a `FetchCmd` and holds a `Result`:

| Variant | Data on Success |
|---------|-----------------|
| `TicketsLoaded` | `(Vec<Ticket>, i32)` -- tickets + total count |
| `DetailLoaded` | `(GetTicketResponse, Vec<Ticket>, i32)` -- detail + children + count |
| `FlowsLoaded` | `(Vec<WorkflowInfo>, i32)` -- workflows + total count |
| `WorkersLoaded` | `Vec<WorkerSummary>` |
| `ActivitiesLoaded` | `{ ticket_id, Vec<ActivityEntry> }` |

### NavMsg

Controls navigation state:

| Variant | Behavior |
|---------|----------|
| `TabSwitch(TabId)` | Switch to tab; if already active, pop to root |
| `TabNext` | Cycle to next tab in display order |
| `Push(PageId)` | Push page onto active tab's stack |
| `Pop` | Pop top page (no-op at root) |
| `Goto(PageId)` | Push if not already current page |

## Cmd Design

Commands are returned by `update` to express side effects. The `CmdRunner` executes them outside the pure core.

| Variant | Effect |
|---------|--------|
| `None` | No-op (filtered out by `Cmd::batch`) |
| `Batch(Vec<Cmd>)` | Execute multiple commands concurrently |
| `Quit` | Send `Msg::Quit` through the channel |
| `Fetch(FetchCmd)` | Spawn async gRPC call, result arrives as `Msg::Data` |
| `SubscribeUiEvents` | Spawn long-lived stream task, events arrive as `Msg::UiEvent` |

### Cmd::batch

`Cmd::batch(cmds)` filters out `None` variants, unwraps single-element batches, and wraps multiple commands in `Batch`. This keeps the update function clean -- it can return `vec![Cmd::None, some_cmd]` without worrying about filtering.

### FetchCmd

Each variant maps to a specific gRPC endpoint:

| Variant | gRPC Call | Result Msg |
|---------|-----------|------------|
| `Tickets { page_size, offset, include_children, statuses }` | `ListTickets` | `DataMsg::TicketsLoaded` |
| `TicketDetail { ticket_id, child_page_size, child_offset, child_status_filter }` | `GetTicket` + `ListTickets` (concurrent) | `DataMsg::DetailLoaded` |
| `Flows { page_size, offset }` | `ListWorkflows` | `DataMsg::FlowsLoaded` |
| `Workers` | `WorkerList` | `DataMsg::WorkersLoaded` |
| `Activities { ticket_id, author_filter }` | `GetTicket` | `DataMsg::ActivitiesLoaded` |

## CmdRunner

The `CmdRunner` is the boundary between the pure TEA core and the impure world of I/O. It holds:

- `msg_tx`: channel sender for feeding results back as `Msg` values
- `port`: gRPC server port
- `project_filter`: optional project key for scoping queries

### Execution

- `Cmd::None`: ignored.
- `Cmd::Quit`: synchronously sends `Msg::Quit`.
- `Cmd::Batch`: iterates and executes each sub-command.
- `Cmd::Fetch(fetch)`: spawns a `tokio::spawn` task that makes a gRPC call and sends the result as `Msg::Data`.
- `Cmd::SubscribeUiEvents`: spawns a long-lived task that connects to `SubscribeUiEvents` RPC and forwards each `UiEventBatch` as `Msg::UiEvent`.

### Error Handling

All gRPC errors are captured as `Err(String)` in the `DataMsg` result variants. The `CmdRunner` never panics on fetch failure -- errors flow back through the message channel and are stored in `LoadState::Error` in the model.

## Input Stack

The input stack (`InputStack`) is an ordered list of `InputHandler` trait objects. It controls how keyboard events are routed and what footer commands are displayed.

### InputHandler Trait

```rust
trait InputHandler {
    fn handle_key(&self, key: KeyEvent) -> InputResult;
    fn footer_commands(&self) -> Vec<FooterCommand>;
    fn name(&self) -> &str;
}
```

`InputResult` is either:
- `Capture(Msg)` -- the handler consumed the key and produced a message.
- `Bubble` -- the handler did not handle this key; try the next one.

### Dispatch (Capture/Bubble)

On each `Msg::KeyPressed`, `update` calls `input_stack.dispatch(key)`:

1. Walk handlers from top (last pushed) to bottom (first pushed).
2. The first handler to return `Capture(msg)` wins -- that `msg` is fed back through `update()` recursively.
3. If all handlers return `Bubble`, the key is ignored.

### Push/Pop Lifecycle

Handlers are pushed when components mount and popped when they unmount:

- `Component::init()` returns `Vec<Box<dyn InputHandler>>` to push onto the stack.
- `Component::teardown()` returns the count of handlers to pop.
- The `NavigationModel` coordinates push/pop during page transitions.

### Priority Ordering

The bottom of the stack holds global handlers (always active). Page-specific handlers are pushed on top, so they get first chance at keys. This means:

- Page handlers can override global shortcuts.
- If a page handler bubbles, global handlers (like Ctrl+C for quit) still work.

### Footer Commands

`input_stack.footer_commands()` collects commands from all handlers bottom-to-top. Global commands appear first, then page-specific ones layer on top. Each `FooterCommand` has a `common` flag that controls rendering position (left vs. right side of the footer bar).

### GlobalHandler

Always at the bottom of the stack. Handles:

| Key | Action |
|-----|--------|
| Ctrl+C | `Msg::Quit` |
| Tab | `Msg::Nav(NavMsg::TabNext)` |
| Esc | `Msg::Nav(NavMsg::Pop)` |

## Component Trait

Components are views over model slices that participate in the navigation lifecycle. They do not own mutable state -- the `Model` holds all data.

```rust
trait Component {
    fn init(&self, model: &Model) -> (Vec<Box<dyn InputHandler>>, Vec<Cmd>);
    fn teardown(&self, model: &Model) -> usize;
    fn update(&self, model: Model, msg: &Msg) -> (Model, Vec<Cmd>);
    fn render(&self, model: &Model, frame: &mut Frame, area: Rect);
    fn name(&self) -> &str;
}
```

### Lifecycle

1. **init**: Called when a component is pushed onto the navigation stack. Returns input handlers to push onto the input stack (last element ends up on top) and initial commands (typically data fetches).
2. **update**: Processes messages relevant to this component. Default is a no-op passthrough.
3. **render**: Draws the component into a given area. Pure function from model to widgets.
4. **teardown**: Called when a component is popped. Returns the number of input handlers that were pushed during `init` and should now be popped.

## Navigation

### TabId and PageId

Three top-level tabs, each with its own page stack:

| TabId | Root PageId | Label |
|-------|-------------|-------|
| `Tickets` | `TicketList` | "Tickets" |
| `Flows` | `FlowList` | "Flows" |
| `Workers` | `WorkerList` | "Workers" |

Detail pages are pushed on top of root pages:

| PageId | Description |
|--------|-------------|
| `TicketList` | Root ticket list |
| `TicketDetail { ticket_id }` | Detail for a specific ticket |
| `FlowList` | Root flows list |
| `WorkerList` | Root workers list |

### NavigationModel

Holds `active_tab: TabId` and `tab_stacks: HashMap<TabId, Vec<PageId>>`. Each tab always has at least one entry (its root page).

#### Tab Switching

- **Different tab**: `active_tab` changes. The previous tab's stack is preserved (stacks are independent).
- **Same tab**: Pops to root -- tears down each stacked page in reverse order, then re-inits the root page.

#### Page Push

`push(page, model)` appends the page to the active tab's stack and calls `init_page()` which triggers data fetches (e.g., `start_ticket_detail_fetch`).

#### Page Pop

`pop(model)` removes the top page if the stack has more than one entry:
1. Teardown the popped page (pop input handlers).
2. Init the newly revealed page (trigger data re-fetch).

#### Goto

`goto(page, model)` pushes the page if it is not already the current page. No-op if the target is already on top.

### Page Initialization

`init_page()` maps each `PageId` to its startup logic:

| PageId | Init Action |
|--------|-------------|
| `TicketList` | `start_ticket_list_fetch` -- sets `LoadState::Loading`, returns `Cmd::Fetch(Tickets)` |
| `TicketDetail` | `start_ticket_detail_fetch` -- creates `TicketDetailModel`, returns batch of detail + activities fetches |
| `FlowList` | `start_flow_list_fetch` -- sets `LoadState::Loading`, returns `Cmd::Fetch(Flows)` |
| `WorkerList` | `start_worker_list_fetch` -- sets `LoadState::Loading`, returns `Cmd::Fetch(Workers)` |

## Data Fetching

### Flow

```
Component init / UI event flush
        │
        │  returns Cmd::Fetch(FetchCmd)
        ▼
   CmdRunner.execute_fetch()
        │
        │  tokio::spawn async gRPC call
        ▼
   gRPC server responds
        │
        │  sends Msg::Data(DataMsg) via msg_tx
        ▼
   update(model, Msg::Data(data_msg))
        │
        │  updates LoadState in sub-model
        ▼
   view() renders from updated LoadState
```

### LoadState

Each sub-model tracks its data with `LoadState<T>`:

| State | Meaning |
|-------|---------|
| `NotLoaded` | Data not yet requested |
| `Loading` | Fetch in progress |
| `Loaded(T)` | Data available |
| `Error(String)` | Fetch failed |

The `start_*_fetch` helpers set the state to `Loading` before returning the `Cmd`. When the result arrives as `Msg::Data`, `handle_data` updates the state to `Loaded` or `Error`.

### Sub-Models

| Sub-Model | Location in Model | Data Type |
|-----------|-------------------|-----------|
| `TicketListModel` | `model.ticket_list` | `TicketListData { tickets, total_count }` |
| `TicketDetailModel` | `model.ticket_detail` (Option) | `TicketDetailData { detail, children, total_children }` + `TicketActivitiesData` |
| `FlowListModel` | `model.flow_list` | `FlowListData { workflows, total_count }` |
| `WorkerListModel` | `model.worker_list` | `WorkerListData { workers }` |

## UI Event Subscription and Throttling

### Event Stream

On startup, `Cmd::SubscribeUiEvents` spawns a long-lived task that:
1. Connects to the `SubscribeUiEvents` gRPC stream.
2. Converts each `UiEventBatch` into `Vec<UiEventItem>` (entity_type + entity_id).
3. Sends `Msg::UiEvent(items)` through the message channel.

### Entity-to-Tab Mapping

When a `Msg::UiEvent` arrives, `handle_ui_event` maps entity types to affected tabs:

| entity_type | Dirty Tabs |
|-------------|------------|
| `"ticket"` | `Tickets`, `Flows` |
| `"workflow"` | `Flows` |
| `"worker"` | `Workers` |
| unknown | none |

### Throttle (UiEventThrottle)

Rapid-fire UI events are batched via a cooldown mechanism:

1. `mark_dirty(tabs)` adds tabs to the dirty set.
2. `should_flush()` returns true if there are dirty tabs and either no cooldown is active or the cooldown (200ms) has elapsed.
3. `flush()` drains dirty tabs and restarts the cooldown timer.

### Flush Behavior

When the throttle flushes (on `Msg::UiEvent` or `Msg::Tick`):

- **Active tab**: Issues a direct `Cmd::Fetch` for the tab's data. If the Tickets tab is active and a `TicketDetailModel` exists, also re-fetches the detail and activities.
- **Inactive tabs**: Sets their `LoadState` to `NotLoaded` (invalidation). Data will be re-fetched when the user switches to that tab.

## Error Handling

Errors propagate through the `Result` variants in `DataMsg`:

1. gRPC call fails in `CmdRunner` fetch task.
2. Error is mapped to `Err(String)` via `.map_err(|e| e.to_string())`.
3. Sent as `Msg::Data(DataMsg::*Loaded(Err(error_string)))`.
4. `handle_data` in `update` stores the error in `LoadState::Error(e)`.
5. The `view` function can render error state from the `LoadState`.

The UI event stream logs disconnections at `warn` level but does not crash. The `CmdRunner` logs fetch failures at `error` level via tracing.

## Key Files

- `crates/urui/src/v2/mod.rs` -- Entry point, TEA loop, crossterm reader, tick timer
- `crates/urui/src/v2/msg.rs` -- `Msg`, `DataMsg`, `NavMsg`, `UiEventItem` definitions
- `crates/urui/src/v2/cmd.rs` -- `Cmd`, `FetchCmd` definitions, `Cmd::batch` helper
- `crates/urui/src/v2/cmd_runner.rs` -- `CmdRunner`, gRPC fetch functions, UI event stream consumer
- `crates/urui/src/v2/update.rs` -- Pure `update` function, message handlers, throttle logic, fetch starters
- `crates/urui/src/v2/model.rs` -- `Model`, sub-models, `LoadState<T>`, `UiEventThrottle`
- `crates/urui/src/v2/input.rs` -- `InputHandler` trait, `InputStack`, `GlobalHandler`, `FooterCommand`
- `crates/urui/src/v2/component.rs` -- `Component` trait (init/teardown/update/render lifecycle)
- `crates/urui/src/v2/navigation.rs` -- `TabId`, `PageId`, `NavigationModel`, page init/teardown
- `crates/urui/src/v2/view.rs` -- Root `view` function (pure model-to-UI rendering)
