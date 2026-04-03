use std::time::Instant;

use super::cmd::{Cmd, FetchCmd};
use super::components::banner::BannerHandler;
use super::model::{
    BannerModel, FlowListData, LoadState, Model, StatusModel, TicketActivitiesData,
    TicketDetailData, TicketDetailModel, TicketListData, TicketTableModel, WorkerListData,
};
use super::msg::{
    DataMsg, FlowOpMsg, FlowOpResultMsg, Msg, NavMsg, OverlayMsg, TicketOpMsg, TicketOpResultMsg,
    UiEventItem, WorkerOpMsg, WorkerOpResultMsg,
};
use super::navigation::TabId;

/// Pure update function: given the current model and a message, produces a new
/// model and a list of commands to execute.
///
/// This function must remain pure — no I/O, no async, no side effects. All
/// effects are expressed as `Cmd` values.
pub fn update(model: Model, msg: Msg) -> (Model, Vec<Cmd>) {
    match msg {
        Msg::Quit => {
            let mut model = model;
            model.should_quit = true;
            (model, vec![Cmd::Quit])
        }
        Msg::KeyPressed(key) => handle_key(model, key),
        Msg::Tick => handle_tick(model),
        Msg::Data(data_msg) => handle_data(model, *data_msg),
        Msg::Nav(nav_msg) => handle_nav(model, nav_msg),
        Msg::UiEvent(items) => handle_ui_event(model, items),
        Msg::BannerShow { message, variant } => handle_banner_show(model, message, variant),
        Msg::BannerDismiss => handle_banner_dismiss(model),
        Msg::StatusShow(text) => handle_status_show(model, text),
        Msg::StatusClear => handle_status_clear(model),
        Msg::Overlay(overlay_msg) => super::overlay_update::handle_overlay(model, overlay_msg),
        Msg::TicketOp(op_msg) => handle_ticket_op(model, op_msg),
        Msg::TicketOpResult(result_msg) => handle_ticket_op_result(model, result_msg),
        Msg::FlowOp(op_msg) => handle_flow_op(model, op_msg),
        Msg::FlowOpResult(result_msg) => handle_flow_op_result(model, result_msg),
        Msg::WorkerOp(op_msg) => handle_worker_op(model, op_msg),
        Msg::WorkerOpResult(result_msg) => handle_worker_op_result(model, result_msg),
    }
}

/// Handle a key press event by dispatching through the input stack.
/// The input stack walks handlers top-to-bottom; the first capture wins.
/// If no handler captures the key, root page handlers get a chance.
/// If a handler captures the key and produces a message, that message is
/// fed back through update() recursively.
fn handle_key(model: Model, key: crossterm::event::KeyEvent) -> (Model, Vec<Cmd>) {
    match model.input_stack.dispatch(key) {
        Some(msg) => update(model, msg),
        None => dispatch_root_page_key(model, key),
    }
}

/// Dispatch a key event to the current root page's handler when the input
/// stack doesn't capture it. Root pages (TicketList, FlowList, WorkerList)
/// don't push handlers onto the stack because they're always present and
/// never torn down during tab switches.
fn dispatch_root_page_key(model: Model, key: crossterm::event::KeyEvent) -> (Model, Vec<Cmd>) {
    use super::input::InputResult;
    use super::pages::flow_detail::FlowDetailHandler;
    use super::pages::flows_list::FlowListHandler;
    use super::pages::ticket_detail::TicketDetailHandler;
    use super::pages::tickets_list::TicketListHandler;
    use super::pages::workers_list::WorkerListHandler;

    let handler: Option<&dyn super::input::InputHandler> =
        match model.navigation_model.current_page() {
            super::navigation::PageId::TicketList => Some(&TicketListHandler),
            super::navigation::PageId::TicketDetail { .. } => Some(&TicketDetailHandler),
            super::navigation::PageId::FlowList => Some(&FlowListHandler),
            super::navigation::PageId::FlowDetail { .. } => Some(&FlowDetailHandler),
            super::navigation::PageId::WorkerList => Some(&WorkerListHandler),
            _ => None,
        };

    match handler {
        Some(h) => match h.handle_key(key) {
            InputResult::Capture(msg) => update(model, msg),
            InputResult::Bubble => (model, vec![]),
        },
        None => (model, vec![]),
    }
}

/// Handle a navigation message by delegating to the NavigationModel.
///
/// Takes ownership of the NavigationModel to avoid double-borrow of `model`,
/// since navigation methods need `&mut Model` for input stack manipulation.
fn handle_nav(model: Model, nav_msg: NavMsg) -> (Model, Vec<Cmd>) {
    // dispatch_page_nav routes to the appropriate page handler or falls through
    // to global navigation. It always returns Some.
    dispatch_page_nav(model, nav_msg).expect("dispatch_page_nav always returns Some")
}

/// Dispatch a NavMsg to the appropriate page-specific handler, or handle
/// it as a global navigation action (tab switch, push, pop, goto).
///
/// Always returns `Some` — the outer `handle_nav` simply unwraps the result.
fn dispatch_page_nav(model: Model, nav_msg: NavMsg) -> Option<(Model, Vec<Cmd>)> {
    if matches!(
        nav_msg,
        NavMsg::ActivitiesNavigate { .. }
            | NavMsg::ActivitiesPageRight
            | NavMsg::ActivitiesPageLeft
            | NavMsg::ActivitiesCycleFilter
            | NavMsg::ActivitiesRefresh
    ) {
        return Some(handle_activities_nav(model, nav_msg));
    }

    if matches!(
        nav_msg,
        NavMsg::BodyScrollDown | NavMsg::BodyScrollUp | NavMsg::BodyPageDown | NavMsg::BodyPageUp
    ) {
        return Some(handle_body_nav(model, nav_msg));
    }

    if matches!(
        nav_msg,
        NavMsg::TicketTableNavigate { .. }
            | NavMsg::TicketTablePageRight
            | NavMsg::TicketTablePageLeft
            | NavMsg::TicketTableSelect
            | NavMsg::TicketListRefresh
            | NavMsg::TicketListPriority
            | NavMsg::TicketListClose
            | NavMsg::TicketListOpen
            | NavMsg::TicketListDispatch
            | NavMsg::TicketListGoto
            | NavMsg::TicketListCreate
            | NavMsg::TicketListEdit
            | NavMsg::TicketListType
    ) {
        return Some(super::pages::tickets_list::handle_ticket_table_nav(
            model, nav_msg,
        ));
    }

    if matches!(
        nav_msg,
        NavMsg::TicketDetailNavigate { .. }
            | NavMsg::TicketDetailPageRight
            | NavMsg::TicketDetailPageLeft
            | NavMsg::TicketDetailSelect
            | NavMsg::TicketDetailRefresh
            | NavMsg::TicketDetailPriority
            | NavMsg::TicketDetailClose
            | NavMsg::TicketDetailOpen
            | NavMsg::TicketDetailDispatch
            | NavMsg::TicketDetailDispatchAll
            | NavMsg::TicketDetailRedrive
            | NavMsg::TicketDetailGoto
            | NavMsg::TicketDetailToggleClosed
            | NavMsg::TicketDetailOpenDescription
            | NavMsg::TicketDetailOpenActivities
            | NavMsg::TicketDetailCreateChild
            | NavMsg::TicketDetailEdit
            | NavMsg::TicketDetailType
    ) {
        return Some(super::pages::ticket_detail::handle_ticket_detail_nav(
            model, nav_msg,
        ));
    }

    if matches!(
        nav_msg,
        NavMsg::FlowsNavigate { .. }
            | NavMsg::FlowsPageRight
            | NavMsg::FlowsPageLeft
            | NavMsg::FlowsSelect
            | NavMsg::FlowsRefresh
            | NavMsg::FlowsCancel
            | NavMsg::FlowsRedrive
            | NavMsg::FlowsGoto
    ) {
        return Some(super::pages::flows_list::handle_flows_nav(model, nav_msg));
    }

    if matches!(
        nav_msg,
        NavMsg::FlowDetailCancel | NavMsg::FlowDetailRedrive | NavMsg::FlowDetailGoto
    ) {
        return Some(super::pages::flow_detail::handle_flow_detail_nav(
            model, nav_msg,
        ));
    }

    if matches!(
        nav_msg,
        NavMsg::WorkersNavigate { .. }
            | NavMsg::WorkersPageRight
            | NavMsg::WorkersPageLeft
            | NavMsg::WorkersRefresh
            | NavMsg::WorkersKill
            | NavMsg::WorkersGoto
    ) {
        return Some(super::pages::workers_list::handle_workers_nav(
            model, nav_msg,
        ));
    }

    Some(handle_global_nav(model, nav_msg))
}

/// Handle global navigation actions: tab switching, pushing, popping, and goto.
fn handle_global_nav(mut model: Model, nav_msg: NavMsg) -> (Model, Vec<Cmd>) {
    let mut nav = std::mem::replace(
        &mut model.navigation_model,
        super::navigation::NavigationModel::initial(),
    );
    let cmds = match nav_msg {
        NavMsg::TabSwitch(tab) => nav.switch_tab(tab, &mut model),
        NavMsg::TabNext => {
            let next = nav.active_tab.next();
            nav.switch_tab(next, &mut model)
        }
        NavMsg::Push(page) => nav.push(page, &mut model),
        NavMsg::Pop => nav.pop(&mut model),
        NavMsg::Goto(page) => nav.goto(page, &mut model),
        // Already handled by page-specific dispatchers above
        _ => vec![],
    };
    model.navigation_model = nav;
    (model, cmds)
}

/// Handle navigation messages specific to the activities page.
fn handle_activities_nav(mut model: Model, nav_msg: NavMsg) -> (Model, Vec<Cmd>) {
    let Some(ref mut am) = model.ticket_activities else {
        return (model, vec![]);
    };

    match nav_msg {
        NavMsg::ActivitiesNavigate { delta } => {
            activities_navigate(am, delta);
        }
        NavMsg::ActivitiesPageRight => {
            activities_page_right(am);
        }
        NavMsg::ActivitiesPageLeft => {
            activities_page_left(am);
        }
        NavMsg::ActivitiesCycleFilter => {
            if let Some(cmd) = activities_cycle_filter(am) {
                return (model, vec![cmd]);
            }
        }
        NavMsg::ActivitiesRefresh => {
            let cmd = activities_refresh(am);
            return (model, vec![cmd]);
        }
        _ => {}
    }
    (model, vec![])
}

/// Navigate up/down within the current activities page.
fn activities_navigate(am: &mut super::model::TicketActivitiesModel, delta: i32) {
    if delta > 0 {
        let page_count = page_activity_count(am);
        if page_count > 0 && am.selected_row < page_count - 1 {
            am.selected_row += 1;
        }
    } else if am.selected_row > 0 {
        am.selected_row -= 1;
    }
}

/// Navigate to the next page in the activities table.
fn activities_page_right(am: &mut super::model::TicketActivitiesModel) {
    let total_pages = activities_total_pages(am);
    if am.current_page + 1 < total_pages {
        am.current_page += 1;
        am.selected_row = 0;
    }
}

/// Navigate to the previous page in the activities table.
fn activities_page_left(am: &mut super::model::TicketActivitiesModel) {
    if am.current_page > 0 {
        am.current_page -= 1;
        am.selected_row = 0;
    }
}

/// Cycle the author filter and return a fetch command if the filter changed.
fn activities_cycle_filter(am: &mut super::model::TicketActivitiesModel) -> Option<Cmd> {
    if am.authors.len() <= 1 {
        return None;
    }
    am.author_index = (am.author_index + 1) % am.authors.len();
    am.current_page = 0;
    am.selected_row = 0;
    am.data = LoadState::Loading;
    Some(activities_fetch_cmd(am))
}

/// Mark activities stale and return the fetch command to refresh.
fn activities_refresh(am: &mut super::model::TicketActivitiesModel) -> Cmd {
    am.data = LoadState::Loading;
    activities_fetch_cmd(am)
}

/// Build the fetch command for the current activities model state.
fn activities_fetch_cmd(am: &super::model::TicketActivitiesModel) -> Cmd {
    let author_filter = if am.author_index == 0 {
        None
    } else {
        am.authors.get(am.author_index).cloned()
    };
    Cmd::Fetch(FetchCmd::Activities {
        ticket_id: am.ticket_id.clone(),
        author_filter,
    })
}

/// Count activities on the current page for navigation clamping.
fn page_activity_count(am: &super::model::TicketActivitiesModel) -> usize {
    let total = am.data.data().map(|d| d.activities.len()).unwrap_or(0);
    let start = am.current_page * am.page_size;
    if start >= total {
        return 0;
    }
    (start + am.page_size).min(total) - start
}

/// Calculate total pages for the activities model.
fn activities_total_pages(am: &super::model::TicketActivitiesModel) -> usize {
    let total = am.data.data().map(|d| d.activities.len()).unwrap_or(0);
    if total == 0 || am.page_size == 0 {
        1
    } else {
        total.div_ceil(am.page_size)
    }
}

/// Handle navigation messages specific to the body/help pages.
fn handle_body_nav(mut model: Model, nav_msg: NavMsg) -> (Model, Vec<Cmd>) {
    use super::pages::help_page::{help_page_down, help_page_up, help_scroll_down, help_scroll_up};
    use super::pages::ticket_body::{
        body_page_down, body_page_up, body_scroll_down, body_scroll_up,
    };

    if model.help_page.is_some()
        && matches!(
            model.navigation_model.current_page(),
            super::navigation::PageId::HelpPage
        )
    {
        match nav_msg {
            NavMsg::BodyScrollDown => help_scroll_down(&mut model, 1),
            NavMsg::BodyScrollUp => help_scroll_up(&mut model, 1),
            NavMsg::BodyPageDown => help_page_down(&mut model),
            NavMsg::BodyPageUp => help_page_up(&mut model),
            _ => {}
        }
    } else {
        match nav_msg {
            NavMsg::BodyScrollDown => body_scroll_down(&mut model, 1),
            NavMsg::BodyScrollUp => body_scroll_up(&mut model, 1),
            NavMsg::BodyPageDown => body_page_down(&mut model),
            NavMsg::BodyPageUp => body_page_up(&mut model),
            _ => {}
        }
    }
    (model, vec![])
}

/// Handle a tick: auto-dismiss expired banners and flush dirty tabs.
fn handle_tick(mut model: Model) -> (Model, Vec<Cmd>) {
    let mut cmds = Vec::new();

    // Check for banner auto-dismiss (success banners expire after timeout).
    if model.banner.as_ref().is_some_and(|b| b.is_expired()) {
        model.banner = None;
        model.input_stack.pop();
    }

    if model.ui_event_throttle.should_flush() {
        cmds = flush_throttle(&mut model);
    }

    (model, cmds)
}

/// Handle a batch of UI events from the server's event stream.
///
/// Maps entity types to the tabs whose data needs refreshing and accumulates
/// them in the throttle. If no cooldown is active, flushes immediately.
fn handle_ui_event(mut model: Model, items: Vec<UiEventItem>) -> (Model, Vec<Cmd>) {
    let dirty_tabs = items
        .iter()
        .flat_map(|item| match item.entity_type.as_str() {
            "ticket" => [Some(TabId::Tickets), Some(TabId::Flows), None],
            "workflow" => [Some(TabId::Flows), None, None],
            "worker" => [Some(TabId::Workers), None, None],
            _ => [None, None, None],
        });
    model.ui_event_throttle.mark_dirty(dirty_tabs.flatten());

    let cmds = if model.ui_event_throttle.should_flush() {
        flush_throttle(&mut model)
    } else {
        vec![]
    };
    (model, cmds)
}

/// Flush the throttle: issue re-fetch commands for all dirty tabs.
///
/// For the active tab, issues a direct fetch command. For non-active tabs,
/// the data will be marked stale on the next tab switch (the LoadState is
/// set back to NotLoaded so it re-fetches when viewed).
fn flush_throttle(model: &mut Model) -> Vec<Cmd> {
    let dirty = model.ui_event_throttle.flush();
    if dirty.is_empty() {
        return vec![];
    }

    let active_tab = model.navigation_model.active_tab;
    let mut cmds = Vec::new();

    for tab in &dirty {
        if *tab == active_tab {
            cmds.push(fetch_cmd_for_tab(*tab, model));
        } else {
            invalidate_tab(*tab, model);
        }
    }
    cmds
}

/// Return the appropriate fetch `Cmd` for a given tab.
fn fetch_cmd_for_tab(tab: TabId, model: &Model) -> Cmd {
    match tab {
        TabId::Tickets => {
            let mut cmds = vec![super::pages::tickets_list::build_ticket_list_fetch_cmd(
                model,
            )];
            // If a ticket detail is open, also re-fetch it.
            if let Some(ref detail) = model.ticket_detail {
                cmds.push(Cmd::Fetch(FetchCmd::TicketDetail {
                    ticket_id: detail.ticket_id.clone(),
                    child_page_size: None,
                    child_offset: None,
                    child_status_filter: None,
                }));
                cmds.push(Cmd::Fetch(FetchCmd::Activities {
                    ticket_id: detail.ticket_id.clone(),
                    author_filter: None,
                }));
            }
            Cmd::batch(cmds)
        }
        TabId::Flows => Cmd::Fetch(FetchCmd::Flows {
            page_size: None,
            offset: None,
        }),
        TabId::Workers => Cmd::Fetch(FetchCmd::Workers),
        TabId::Help => Cmd::None, // static content, no fetch needed
    }
}

/// Set a non-active tab's data back to `NotLoaded` so it re-fetches when viewed.
fn invalidate_tab(tab: TabId, model: &mut Model) {
    match tab {
        TabId::Tickets => model.ticket_list.data = LoadState::NotLoaded,
        TabId::Flows => model.flow_list.data = LoadState::NotLoaded,
        TabId::Workers => model.worker_list.data = LoadState::NotLoaded,
        TabId::Help => {} // static content, nothing to invalidate
    }
}

/// Show a banner: set the model's banner state and push a BannerHandler onto the input stack.
fn handle_banner_show(
    mut model: Model,
    message: String,
    variant: super::components::banner::BannerVariant,
) -> (Model, Vec<Cmd>) {
    // If there's already a banner, pop its handler first.
    if model.banner.is_some() {
        model.input_stack.pop();
    }
    model.banner = Some(BannerModel {
        message,
        variant,
        created_at: Instant::now(),
    });
    model.input_stack.push(Box::new(BannerHandler));
    (model, vec![])
}

/// Dismiss the active banner: clear the model state and pop the BannerHandler.
fn handle_banner_dismiss(mut model: Model) -> (Model, Vec<Cmd>) {
    if model.banner.is_some() {
        model.banner = None;
        model.input_stack.pop();
    }
    (model, vec![])
}

/// Show a status message in the header area.
fn handle_status_show(mut model: Model, text: String) -> (Model, Vec<Cmd>) {
    model.status = Some(StatusModel { text });
    (model, vec![])
}

/// Clear the current status message.
fn handle_status_clear(mut model: Model) -> (Model, Vec<Cmd>) {
    model.status = None;
    (model, vec![])
}

/// Handle a ticket operation request: return the Cmd to execute and set a
/// status message while the operation is in flight.
fn handle_ticket_op(model: Model, op: TicketOpMsg) -> (Model, Vec<Cmd>) {
    let status_text = match &op {
        TicketOpMsg::Dispatch { ticket_id, .. } => format!("Dispatching {ticket_id}..."),
        TicketOpMsg::DispatchAll { ticket_id, .. } => format!("Dispatching {ticket_id}..."),
        TicketOpMsg::Close { ticket_id } => format!("Closing {ticket_id}..."),
        TicketOpMsg::ForceClose { ticket_id } => format!("Force-closing {ticket_id}..."),
        TicketOpMsg::SetPriority {
            ticket_id,
            priority,
        } => format!("Setting P{priority} on {ticket_id}..."),
        TicketOpMsg::SetType {
            ticket_id,
            ticket_type,
        } => format!("Setting type to {ticket_type} on {ticket_id}..."),
        TicketOpMsg::Create { pending } => format!("Creating ticket in {}...", pending.project),
        TicketOpMsg::CreateAndDispatch { pending, .. } => {
            format!("Creating and dispatching in {}...", pending.project)
        }
        TicketOpMsg::LaunchDesign { ticket_id, .. } => {
            format!("Launching design worker for {ticket_id}...")
        }
        TicketOpMsg::Redrive { ticket_id } => format!("Moving {ticket_id} to Verify..."),
        TicketOpMsg::Open { ticket_id } => format!("Reopening {ticket_id}..."),
        TicketOpMsg::UpdateFields { ticket_id, .. } => {
            format!("Updating {ticket_id}...")
        }
    };

    // Feed through update to set the status and issue the command.
    let (model, mut cmds) = update(model, Msg::StatusShow(status_text));
    cmds.push(Cmd::TicketOp(op));
    (model, cmds)
}

/// Handle a ticket operation result: clear status, show banner.
///
/// Success results show an auto-dismissing success banner.
/// Error results show a sticky error banner.
/// Priority-set success is silent (no banner) since the UI updates via events.
fn handle_ticket_op_result(model: Model, result_msg: TicketOpResultMsg) -> (Model, Vec<Cmd>) {
    use super::components::banner::BannerVariant;

    let (model, _) = update(model, Msg::StatusClear);

    let (result, silent_on_success, pending) = match result_msg {
        TicketOpResultMsg::Dispatched { result } => (result, false, None),
        TicketOpResultMsg::Closed { result } => (result, false, None),
        TicketOpResultMsg::ForceClosed { result } => (result, true, None),
        TicketOpResultMsg::PrioritySet { result } => (result, true, None),
        TicketOpResultMsg::TypeSet { result } => (result, true, None),
        TicketOpResultMsg::Created { result, pending } => (result, false, pending),
        TicketOpResultMsg::DesignLaunched { result } => (result, false, None),
        TicketOpResultMsg::Redriven { result } => (result, false, None),
        TicketOpResultMsg::Opened { result } => (result, false, None),
        TicketOpResultMsg::Updated { result } => (result, false, None),
    };

    match result {
        Ok(message) => {
            if silent_on_success {
                (model, vec![])
            } else {
                update(
                    model,
                    Msg::BannerShow {
                        message,
                        variant: BannerVariant::Success,
                    },
                )
            }
        }
        Err(message) => {
            let (model, mut cmds) = update(
                model,
                Msg::BannerShow {
                    message,
                    variant: BannerVariant::Error,
                },
            );
            if let Some(pending) = pending {
                let (model, overlay_cmds) = update(
                    model,
                    Msg::Overlay(OverlayMsg::OpenCreateActionMenu { pending }),
                );
                cmds.extend(overlay_cmds);
                (model, cmds)
            } else {
                (model, cmds)
            }
        }
    }
}

/// Handle a flow operation request: set status message and issue the command.
fn handle_flow_op(model: Model, op: FlowOpMsg) -> (Model, Vec<Cmd>) {
    let status_text = match &op {
        FlowOpMsg::Cancel { ticket_id } => format!("Cancelling workflow for {ticket_id}..."),
    };

    let (model, mut cmds) = update(model, Msg::StatusShow(status_text));
    cmds.push(Cmd::FlowOp(op));
    (model, cmds)
}

/// Handle a flow operation result: clear status, show banner.
fn handle_flow_op_result(model: Model, result_msg: FlowOpResultMsg) -> (Model, Vec<Cmd>) {
    use super::components::banner::BannerVariant;

    let (model, _) = update(model, Msg::StatusClear);

    let result = match result_msg {
        FlowOpResultMsg::Cancelled { result } => result,
    };

    match result {
        Ok(message) => update(
            model,
            Msg::BannerShow {
                message,
                variant: BannerVariant::Success,
            },
        ),
        Err(message) => update(
            model,
            Msg::BannerShow {
                message,
                variant: BannerVariant::Error,
            },
        ),
    }
}

/// Handle a worker operation request: set status message and issue the command.
fn handle_worker_op(model: Model, op: WorkerOpMsg) -> (Model, Vec<Cmd>) {
    let status_text = match &op {
        WorkerOpMsg::Kill { worker_id } => format!("Killing worker {worker_id}..."),
    };

    let (model, mut cmds) = update(model, Msg::StatusShow(status_text));
    cmds.push(Cmd::WorkerOp(op));
    (model, cmds)
}

/// Handle a worker operation result: clear status, show banner.
fn handle_worker_op_result(model: Model, result_msg: WorkerOpResultMsg) -> (Model, Vec<Cmd>) {
    use super::components::banner::BannerVariant;

    let (model, _) = update(model, Msg::StatusClear);

    let result = match result_msg {
        WorkerOpResultMsg::Killed { result } => result,
    };

    match result {
        Ok(message) => update(
            model,
            Msg::BannerShow {
                message,
                variant: BannerVariant::Success,
            },
        ),
        Err(message) => update(
            model,
            Msg::BannerShow {
                message,
                variant: BannerVariant::Error,
            },
        ),
    }
}

/// Handle a data message by updating the appropriate sub-model's `LoadState`.
fn handle_data(mut model: Model, data_msg: DataMsg) -> (Model, Vec<Cmd>) {
    match data_msg {
        DataMsg::TicketsLoaded(result) => {
            model.ticket_list.data = match result {
                Ok((tickets, total_count)) => {
                    let data = TicketListData {
                        tickets,
                        total_count,
                    };
                    super::pages::tickets_list::apply_tickets_data(&mut model, data.clone());
                    LoadState::Loaded(data)
                }
                Err(e) => LoadState::Error(e),
            };
        }
        DataMsg::DetailLoaded(result) => {
            if let Some(ref mut detail_model) = model.ticket_detail {
                detail_model.data = apply_detail_result(detail_model, *result);
            }
        }
        DataMsg::FlowsLoaded(result) => {
            model.flow_list.data = match result {
                Ok((workflows, total_count)) => {
                    let (notif_msgs, notif_cmds) =
                        model.notifications.process_flow_updates(&workflows);
                    let data = FlowListData {
                        workflows,
                        total_count,
                    };
                    super::pages::flows_list::apply_flows_data(&mut model, &data);
                    model.flow_list.data = LoadState::Loaded(data);
                    super::pages::flows_list::clamp_selection(&mut model);
                    // Process the last notification message (banner) through update.
                    // We only show the most recent one to avoid banner churn.
                    let mut cmds = notif_cmds;
                    if let Some(msg) = notif_msgs.into_iter().last() {
                        let (m, extra_cmds) = update(model, msg);
                        model = m;
                        cmds.extend(extra_cmds);
                    }
                    return (model, cmds);
                }
                Err(e) => LoadState::Error(e),
            };
        }
        DataMsg::WorkersLoaded(result) => {
            model.worker_list.data = match result {
                Ok(workers) => LoadState::Loaded(WorkerListData { workers }),
                Err(e) => LoadState::Error(e),
            };
            super::pages::workers_list::clamp_selection(&mut model);
        }
        DataMsg::WorkerStopped { worker_id, result } => {
            return super::pages::workers_list::handle_worker_stopped(model, worker_id, result);
        }
        DataMsg::ActivitiesLoaded { ticket_id, result } => {
            if let Some(ref mut detail_model) = model.ticket_detail
                && detail_model.ticket_id == ticket_id
            {
                detail_model.activities = match &result {
                    Ok(activities) => LoadState::Loaded(TicketActivitiesData {
                        activities: activities.clone(),
                    }),
                    Err(e) => LoadState::Error(e.clone()),
                };
            }
            // Also update the full-screen activities page model if active.
            super::pages::ticket_activities::handle_activities_data(&mut model, ticket_id, result);
        }
    }
    (model, vec![])
}

/// Apply a detail load result to the detail model, populating the children table
/// and clamping selection as needed.
fn apply_detail_result(
    detail_model: &mut TicketDetailModel,
    result: super::msg::DetailLoadResult,
) -> LoadState<TicketDetailData> {
    match result {
        Ok((detail, children, total_children)) => {
            detail_model.children_table.tickets = children.clone();
            detail_model.children_table.total_count = total_children;
            let count = detail_model.children_table.tickets.len();
            if count > 0 && detail_model.children_table.selected_row >= count {
                detail_model.children_table.selected_row = count - 1;
            }
            LoadState::Loaded(TicketDetailData {
                detail,
                children,
                total_children,
            })
        }
        Err(e) => LoadState::Error(e),
    }
}

/// Build a batch of fetch commands to refresh all page data.
/// Called on `NavPop` or explicit refresh to re-fetch potentially stale data.
#[cfg(test)]
pub fn refresh_all_cmd() -> Cmd {
    Cmd::batch(vec![
        Cmd::Fetch(FetchCmd::Tickets {
            page_size: None,
            offset: None,
            include_children: None,
            statuses: vec![],
        }),
        Cmd::Fetch(FetchCmd::Flows {
            page_size: None,
            offset: None,
        }),
        Cmd::Fetch(FetchCmd::Workers),
    ])
}

/// Set a sub-model to `Loading` and return the corresponding fetch command
/// for the ticket list page.
pub fn start_ticket_list_fetch(model: &mut Model) -> Cmd {
    model.ticket_list.data = LoadState::Loading;
    super::pages::tickets_list::build_ticket_list_fetch_cmd(model)
}

/// Set a sub-model to `Loading` and return the corresponding fetch command
/// for the ticket detail page.
pub fn start_ticket_detail_fetch(model: &mut Model, ticket_id: String) -> Cmd {
    // Preserve show_closed from a previous detail model if re-fetching same ticket.
    let show_closed = model
        .ticket_detail
        .as_ref()
        .filter(|d| d.ticket_id == ticket_id)
        .is_some_and(|d| d.show_closed);
    let child_status_filter = if show_closed {
        None
    } else {
        Some("open,in_progress".to_string())
    };
    model.ticket_detail = Some(TicketDetailModel {
        ticket_id: ticket_id.clone(),
        data: LoadState::Loading,
        activities: LoadState::Loading,
        children_table: TicketTableModel::empty(),
        show_closed,
    });
    Cmd::batch(vec![
        Cmd::Fetch(FetchCmd::TicketDetail {
            ticket_id: ticket_id.clone(),
            child_page_size: None,
            child_offset: None,
            child_status_filter,
        }),
        Cmd::Fetch(FetchCmd::Activities {
            ticket_id,
            author_filter: None,
        }),
    ])
}

/// Set a sub-model to `Loading` and return the corresponding fetch command
/// for the flows page.
pub fn start_flow_list_fetch(model: &mut Model) -> Cmd {
    model.flow_list.data = LoadState::Loading;
    Cmd::Fetch(FetchCmd::Flows {
        page_size: None,
        offset: None,
    })
}

/// Set a sub-model to `Loading` and return the corresponding fetch command
/// for the workers page.
pub fn start_worker_list_fetch(model: &mut Model) -> Cmd {
    model.worker_list.data = LoadState::Loading;
    Cmd::Fetch(FetchCmd::Workers)
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use ur_rpc::proto::ticket::GetTicketResponse;

    use super::*;

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn quit_message_sets_should_quit() {
        let model = Model::initial();
        let (new_model, cmds) = update(model, Msg::Quit);
        assert!(new_model.should_quit);
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Quit)));
    }

    #[test]
    fn ctrl_c_triggers_quit() {
        let model = Model::initial();
        let key = make_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let (new_model, cmds) = update(model, Msg::KeyPressed(key));
        assert!(new_model.should_quit);
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Quit)));
    }

    #[test]
    fn tick_is_noop_when_no_dirty_tabs() {
        let model = Model::initial();
        let (new_model, cmds) = update(model, Msg::Tick);
        assert!(!new_model.should_quit);
        assert!(cmds.is_empty());
    }

    #[test]
    fn regular_key_is_noop() {
        let model = Model::initial();
        let key = make_key(KeyCode::Char('a'), KeyModifiers::NONE);
        let (new_model, cmds) = update(model, Msg::KeyPressed(key));
        assert!(!new_model.should_quit);
        assert!(cmds.is_empty());
    }

    #[test]
    fn data_tickets_loaded_success() {
        let model = Model::initial();
        let msg = Msg::Data(Box::new(DataMsg::TicketsLoaded(Ok((vec![], 0)))));
        let (new_model, cmds) = update(model, msg);
        assert!(new_model.ticket_list.data.is_loaded());
        assert!(cmds.is_empty());
        let data = new_model.ticket_list.data.data().unwrap();
        assert!(data.tickets.is_empty());
        assert_eq!(data.total_count, 0);
    }

    #[test]
    fn data_tickets_loaded_error() {
        let model = Model::initial();
        let msg = Msg::Data(Box::new(DataMsg::TicketsLoaded(Err(
            "connection refused".into()
        ))));
        let (new_model, cmds) = update(model, msg);
        assert!(matches!(
            new_model.ticket_list.data,
            LoadState::Error(ref e) if e == "connection refused"
        ));
        assert!(cmds.is_empty());
    }

    #[test]
    fn data_flows_loaded_success() {
        let model = Model::initial();
        let msg = Msg::Data(Box::new(DataMsg::FlowsLoaded(Ok((vec![], 5)))));
        let (new_model, _) = update(model, msg);
        assert!(new_model.flow_list.data.is_loaded());
        assert_eq!(new_model.flow_list.data.data().unwrap().total_count, 5);
    }

    #[test]
    fn data_flows_loaded_error() {
        let model = Model::initial();
        let msg = Msg::Data(Box::new(DataMsg::FlowsLoaded(Err("timeout".into()))));
        let (new_model, _) = update(model, msg);
        assert!(matches!(new_model.flow_list.data, LoadState::Error(_)));
    }

    #[test]
    fn data_workers_loaded_success() {
        let model = Model::initial();
        let msg = Msg::Data(Box::new(DataMsg::WorkersLoaded(Ok(vec![]))));
        let (new_model, _) = update(model, msg);
        assert!(new_model.worker_list.data.is_loaded());
        assert!(
            new_model
                .worker_list
                .data
                .data()
                .unwrap()
                .workers
                .is_empty()
        );
    }

    #[test]
    fn data_workers_loaded_error() {
        let model = Model::initial();
        let msg = Msg::Data(Box::new(DataMsg::WorkersLoaded(Err("fail".into()))));
        let (new_model, _) = update(model, msg);
        assert!(matches!(new_model.worker_list.data, LoadState::Error(_)));
    }

    #[test]
    fn data_detail_loaded_updates_existing_detail_model() {
        let mut model = Model::initial();
        model.ticket_detail = Some(TicketDetailModel {
            ticket_id: "ur-abc".to_string(),
            data: LoadState::Loading,
            activities: LoadState::Loading,
            children_table: TicketTableModel::empty(),
            show_closed: false,
        });
        let msg = Msg::Data(Box::new(DataMsg::DetailLoaded(Box::new(Ok((
            GetTicketResponse::default(),
            vec![],
            3,
        ))))));
        let (new_model, _) = update(model, msg);
        let detail = new_model.ticket_detail.unwrap();
        assert!(detail.data.is_loaded());
        assert_eq!(detail.data.data().unwrap().total_children, 3);
    }

    #[test]
    fn data_detail_loaded_ignored_when_no_detail_model() {
        let model = Model::initial();
        assert!(model.ticket_detail.is_none());
        let msg = Msg::Data(Box::new(DataMsg::DetailLoaded(Box::new(Ok((
            GetTicketResponse::default(),
            vec![],
            0,
        ))))));
        let (new_model, _) = update(model, msg);
        // No detail model to update, so it remains None
        assert!(new_model.ticket_detail.is_none());
    }

    #[test]
    fn data_activities_loaded_matches_ticket_id() {
        let mut model = Model::initial();
        model.ticket_detail = Some(TicketDetailModel {
            ticket_id: "ur-xyz".to_string(),
            data: LoadState::Loading,
            activities: LoadState::Loading,
            children_table: TicketTableModel::empty(),
            show_closed: false,
        });
        let msg = Msg::Data(Box::new(DataMsg::ActivitiesLoaded {
            ticket_id: "ur-xyz".to_string(),
            result: Ok(vec![]),
        }));
        let (new_model, _) = update(model, msg);
        let detail = new_model.ticket_detail.unwrap();
        assert!(detail.activities.is_loaded());
    }

    #[test]
    fn data_activities_loaded_ignores_mismatched_ticket_id() {
        let mut model = Model::initial();
        model.ticket_detail = Some(TicketDetailModel {
            ticket_id: "ur-xyz".to_string(),
            data: LoadState::Loading,
            activities: LoadState::Loading,
            children_table: TicketTableModel::empty(),
            show_closed: false,
        });
        let msg = Msg::Data(Box::new(DataMsg::ActivitiesLoaded {
            ticket_id: "ur-other".to_string(),
            result: Ok(vec![]),
        }));
        let (new_model, _) = update(model, msg);
        let detail = new_model.ticket_detail.unwrap();
        // Should still be Loading because ticket IDs don't match
        assert!(detail.activities.is_loading());
    }

    #[test]
    fn start_ticket_list_fetch_sets_loading() {
        let mut model = Model::initial();
        let cmd = start_ticket_list_fetch(&mut model);
        assert!(model.ticket_list.data.is_loading());
        assert!(matches!(cmd, Cmd::Fetch(FetchCmd::Tickets { .. })));
    }

    #[test]
    fn start_ticket_detail_fetch_creates_detail_model() {
        let mut model = Model::initial();
        let cmd = start_ticket_detail_fetch(&mut model, "ur-abc".to_string());
        let detail = model.ticket_detail.unwrap();
        assert_eq!(detail.ticket_id, "ur-abc");
        assert!(detail.data.is_loading());
        assert!(detail.activities.is_loading());
        assert!(matches!(cmd, Cmd::Batch(_)));
    }

    #[test]
    fn start_flow_list_fetch_sets_loading() {
        let mut model = Model::initial();
        let cmd = start_flow_list_fetch(&mut model);
        assert!(model.flow_list.data.is_loading());
        assert!(matches!(cmd, Cmd::Fetch(FetchCmd::Flows { .. })));
    }

    #[test]
    fn start_worker_list_fetch_sets_loading() {
        let mut model = Model::initial();
        let cmd = start_worker_list_fetch(&mut model);
        assert!(model.worker_list.data.is_loading());
        assert!(matches!(cmd, Cmd::Fetch(FetchCmd::Workers)));
    }

    #[test]
    fn refresh_all_produces_batch() {
        let cmd = refresh_all_cmd();
        assert!(matches!(cmd, Cmd::Batch(_)));
    }

    #[test]
    fn nav_tab_switch_changes_active_tab() {
        use crate::navigation::TabId;
        let model = Model::initial();
        let (new_model, _cmds) = update(model, Msg::Nav(NavMsg::TabSwitch(TabId::Flows)));
        assert_eq!(new_model.navigation_model.active_tab, TabId::Flows);
    }

    #[test]
    fn nav_tab_next_cycles_tab() {
        use crate::navigation::TabId;
        let model = Model::initial();
        assert_eq!(model.navigation_model.active_tab, TabId::Tickets);
        let (new_model, _cmds) = update(model, Msg::Nav(NavMsg::TabNext));
        assert_eq!(new_model.navigation_model.active_tab, TabId::Flows);
    }

    #[test]
    fn nav_push_adds_page() {
        use crate::navigation::PageId;
        let model = Model::initial();
        let (new_model, cmds) = update(
            model,
            Msg::Nav(NavMsg::Push(PageId::TicketDetail {
                ticket_id: "ur-abc".into(),
            })),
        );
        assert_eq!(new_model.navigation_model.active_stack_depth(), 2);
        assert!(!cmds.is_empty());
    }

    #[test]
    fn nav_pop_removes_page() {
        use crate::navigation::PageId;
        let model = Model::initial();
        // First push a detail page
        let (model, _) = update(
            model,
            Msg::Nav(NavMsg::Push(PageId::TicketDetail {
                ticket_id: "ur-abc".into(),
            })),
        );
        assert_eq!(model.navigation_model.active_stack_depth(), 2);

        // Then pop it
        let (new_model, _cmds) = update(model, Msg::Nav(NavMsg::Pop));
        assert_eq!(new_model.navigation_model.active_stack_depth(), 1);
    }

    #[test]
    fn nav_goto_pushes_if_not_current() {
        use crate::navigation::PageId;
        let model = Model::initial();
        let (new_model, cmds) = update(
            model,
            Msg::Nav(NavMsg::Goto(PageId::TicketDetail {
                ticket_id: "ur-xyz".into(),
            })),
        );
        assert_eq!(new_model.navigation_model.active_stack_depth(), 2);
        assert!(!cmds.is_empty());
    }

    #[test]
    fn nav_goto_same_page_is_noop() {
        use crate::navigation::PageId;
        let model = Model::initial();
        let (new_model, cmds) = update(model, Msg::Nav(NavMsg::Goto(PageId::TicketList)));
        assert_eq!(new_model.navigation_model.active_stack_depth(), 1);
        assert!(cmds.is_empty());
    }

    #[test]
    fn ui_event_ticket_triggers_fetch_for_active_tab() {
        // Active tab is Tickets by default, so a ticket event should produce a fetch.
        let model = Model::initial();
        let msg = Msg::UiEvent(vec![UiEventItem {
            entity_type: "ticket".to_string(),
            entity_id: "ur-abc".to_string(),
        }]);
        let (new_model, cmds) = update(model, msg);
        // Throttle should have flushed (no prior cooldown).
        assert!(new_model.ui_event_throttle.dirty.is_empty());
        assert!(!cmds.is_empty());
    }

    #[test]
    fn ui_event_worker_on_tickets_tab_invalidates_workers() {
        // Active tab is Tickets. Worker event should invalidate workers tab (NotLoaded).
        let mut model = Model::initial();
        model.worker_list.data = LoadState::Loaded(WorkerListData { workers: vec![] });
        let msg = Msg::UiEvent(vec![UiEventItem {
            entity_type: "worker".to_string(),
            entity_id: "w-1".to_string(),
        }]);
        let (new_model, cmds) = update(model, msg);
        // Workers tab is not active, so it should be invalidated (NotLoaded).
        assert!(matches!(new_model.worker_list.data, LoadState::NotLoaded));
        // No fetch commands for the inactive tab.
        assert!(cmds.is_empty());
    }

    #[test]
    fn ui_event_throttle_accumulates_during_cooldown() {
        let model = Model::initial();
        // First event: flushes immediately (no cooldown).
        let msg = Msg::UiEvent(vec![UiEventItem {
            entity_type: "ticket".to_string(),
            entity_id: "ur-1".to_string(),
        }]);
        let (model, _) = update(model, msg);

        // Cooldown is now active. Second event should accumulate.
        let msg2 = Msg::UiEvent(vec![UiEventItem {
            entity_type: "worker".to_string(),
            entity_id: "w-2".to_string(),
        }]);
        let (new_model, cmds) = update(model, msg2);
        // Workers tab should be dirty but not flushed (in cooldown).
        assert!(new_model.ui_event_throttle.dirty.contains(&TabId::Workers));
        assert!(cmds.is_empty());
    }

    #[test]
    fn tick_flushes_throttle_after_cooldown() {
        use std::time::{Duration, Instant};

        let mut model = Model::initial();
        // Simulate dirty tabs with an expired cooldown.
        model.ui_event_throttle.mark_dirty([TabId::Tickets]);
        model.ui_event_throttle.cooldown_start = Some(Instant::now() - Duration::from_millis(300));

        let (new_model, cmds) = update(model, Msg::Tick);
        assert!(new_model.ui_event_throttle.dirty.is_empty());
        assert!(!cmds.is_empty());
    }

    #[test]
    fn tick_does_not_flush_during_cooldown() {
        use std::time::Instant;

        let mut model = Model::initial();
        // Simulate dirty tabs with an active cooldown.
        model.ui_event_throttle.mark_dirty([TabId::Tickets]);
        model.ui_event_throttle.cooldown_start = Some(Instant::now());

        let (new_model, cmds) = update(model, Msg::Tick);
        assert!(!new_model.ui_event_throttle.dirty.is_empty());
        assert!(cmds.is_empty());
    }

    #[test]
    fn ui_event_workflow_marks_flows_dirty() {
        let mut model = Model::initial();
        // Switch to Workers tab so flows are not active.
        model.navigation_model.active_tab = TabId::Workers;
        model.flow_list.data = LoadState::Loaded(FlowListData {
            workflows: vec![],
            total_count: 0,
        });

        let msg = Msg::UiEvent(vec![UiEventItem {
            entity_type: "workflow".to_string(),
            entity_id: "ur-flow".to_string(),
        }]);
        let (new_model, _) = update(model, msg);
        // Flows tab should be invalidated (NotLoaded) since it was not active.
        assert!(matches!(new_model.flow_list.data, LoadState::NotLoaded));
    }

    #[test]
    fn ui_event_unknown_type_is_noop() {
        let model = Model::initial();
        let msg = Msg::UiEvent(vec![UiEventItem {
            entity_type: "unknown".to_string(),
            entity_id: "x".to_string(),
        }]);
        let (new_model, cmds) = update(model, msg);
        assert!(new_model.ui_event_throttle.dirty.is_empty());
        assert!(cmds.is_empty());
    }

    #[test]
    fn ui_event_empty_batch_is_noop() {
        let model = Model::initial();
        let msg = Msg::UiEvent(vec![]);
        let (new_model, cmds) = update(model, msg);
        assert!(new_model.ui_event_throttle.dirty.is_empty());
        assert!(cmds.is_empty());
    }

    #[test]
    fn fetch_cmd_for_tab_includes_detail_refetch() {
        let mut model = Model::initial();
        model.ticket_detail = Some(TicketDetailModel {
            ticket_id: "ur-det".to_string(),
            data: LoadState::Loading,
            activities: LoadState::Loading,
            children_table: TicketTableModel::empty(),
            show_closed: false,
        });
        let cmd = fetch_cmd_for_tab(TabId::Tickets, &model);
        // Should be a Batch containing ticket list + detail + activities.
        assert!(matches!(cmd, Cmd::Batch(_)));
    }

    #[test]
    fn banner_show_sets_banner_and_pushes_handler() {
        use super::super::components::banner::BannerVariant;
        let model = Model::initial();
        let initial_stack_len = model.input_stack.len();
        let (new_model, cmds) = update(
            model,
            Msg::BannerShow {
                message: "Success!".into(),
                variant: BannerVariant::Success,
            },
        );
        assert!(new_model.banner.is_some());
        let banner = new_model.banner.as_ref().unwrap();
        assert_eq!(banner.message, "Success!");
        assert_eq!(banner.variant, BannerVariant::Success);
        assert_eq!(new_model.input_stack.len(), initial_stack_len + 1);
        assert!(cmds.is_empty());
    }

    #[test]
    fn banner_show_replaces_existing_banner() {
        use super::super::components::banner::BannerVariant;
        let model = Model::initial();
        let initial_stack_len = model.input_stack.len();

        // Show first banner
        let (model, _) = update(
            model,
            Msg::BannerShow {
                message: "First".into(),
                variant: BannerVariant::Success,
            },
        );
        assert_eq!(model.input_stack.len(), initial_stack_len + 1);

        // Show second banner (should replace, not stack)
        let (new_model, _) = update(
            model,
            Msg::BannerShow {
                message: "Second".into(),
                variant: BannerVariant::Error,
            },
        );
        assert_eq!(new_model.banner.as_ref().unwrap().message, "Second");
        assert_eq!(
            new_model.banner.as_ref().unwrap().variant,
            BannerVariant::Error
        );
        // Stack size should remain the same (popped old, pushed new)
        assert_eq!(new_model.input_stack.len(), initial_stack_len + 1);
    }

    #[test]
    fn banner_dismiss_clears_banner_and_pops_handler() {
        use super::super::components::banner::BannerVariant;
        let model = Model::initial();
        let initial_stack_len = model.input_stack.len();

        // Show a banner
        let (model, _) = update(
            model,
            Msg::BannerShow {
                message: "Test".into(),
                variant: BannerVariant::Success,
            },
        );
        assert!(model.banner.is_some());

        // Dismiss it
        let (new_model, cmds) = update(model, Msg::BannerDismiss);
        assert!(new_model.banner.is_none());
        assert_eq!(new_model.input_stack.len(), initial_stack_len);
        assert!(cmds.is_empty());
    }

    #[test]
    fn banner_dismiss_when_no_banner_is_noop() {
        let model = Model::initial();
        let initial_stack_len = model.input_stack.len();
        let (new_model, cmds) = update(model, Msg::BannerDismiss);
        assert!(new_model.banner.is_none());
        assert_eq!(new_model.input_stack.len(), initial_stack_len);
        assert!(cmds.is_empty());
    }

    #[test]
    fn banner_auto_dismiss_on_tick_for_expired_success() {
        use super::super::components::banner::BannerVariant;
        let mut model = Model::initial();
        // Set a success banner that was created 10 seconds ago (expired).
        model.banner = Some(BannerModel {
            message: "Old success".into(),
            variant: BannerVariant::Success,
            created_at: Instant::now() - std::time::Duration::from_secs(10),
        });
        model.input_stack.push(Box::new(BannerHandler));
        let initial_stack_len = model.input_stack.len();

        let (new_model, _) = update(model, Msg::Tick);
        assert!(new_model.banner.is_none());
        assert_eq!(new_model.input_stack.len(), initial_stack_len - 1);
    }

    #[test]
    fn banner_no_auto_dismiss_for_error() {
        use super::super::components::banner::BannerVariant;
        let mut model = Model::initial();
        // Set an error banner that was created 10 seconds ago.
        model.banner = Some(BannerModel {
            message: "Error!".into(),
            variant: BannerVariant::Error,
            created_at: Instant::now() - std::time::Duration::from_secs(10),
        });
        model.input_stack.push(Box::new(BannerHandler));

        let (new_model, _) = update(model, Msg::Tick);
        // Error banners are sticky — should not be dismissed.
        assert!(new_model.banner.is_some());
    }

    #[test]
    fn banner_no_auto_dismiss_for_fresh_success() {
        use super::super::components::banner::BannerVariant;
        let mut model = Model::initial();
        // Set a success banner that was just created (not expired).
        model.banner = Some(BannerModel {
            message: "Fresh".into(),
            variant: BannerVariant::Success,
            created_at: Instant::now(),
        });
        model.input_stack.push(Box::new(BannerHandler));

        let (new_model, _) = update(model, Msg::Tick);
        // Not expired yet — should remain.
        assert!(new_model.banner.is_some());
    }

    #[test]
    fn status_show_sets_status() {
        let model = Model::initial();
        let (new_model, cmds) = update(model, Msg::StatusShow("Loading...".into()));
        assert!(new_model.status.is_some());
        assert_eq!(new_model.status.as_ref().unwrap().text, "Loading...");
        assert!(cmds.is_empty());
    }

    #[test]
    fn status_show_replaces_existing() {
        let model = Model::initial();
        let (model, _) = update(model, Msg::StatusShow("First".into()));
        let (new_model, _) = update(model, Msg::StatusShow("Second".into()));
        assert_eq!(new_model.status.as_ref().unwrap().text, "Second");
    }

    #[test]
    fn status_clear_removes_status() {
        let model = Model::initial();
        let (model, _) = update(model, Msg::StatusShow("Active".into()));
        assert!(model.status.is_some());
        let (new_model, cmds) = update(model, Msg::StatusClear);
        assert!(new_model.status.is_none());
        assert!(cmds.is_empty());
    }

    #[test]
    fn status_clear_when_no_status_is_noop() {
        let model = Model::initial();
        let (new_model, cmds) = update(model, Msg::StatusClear);
        assert!(new_model.status.is_none());
        assert!(cmds.is_empty());
    }

    #[test]
    fn enter_key_dismisses_banner_via_handler() {
        use super::super::components::banner::BannerVariant;
        let model = Model::initial();
        // Show a banner (this pushes BannerHandler)
        let (model, _) = update(
            model,
            Msg::BannerShow {
                message: "Test".into(),
                variant: BannerVariant::Error,
            },
        );
        assert!(model.banner.is_some());

        // Press Enter — BannerHandler should capture it and produce BannerDismiss
        let key = make_key(KeyCode::Enter, KeyModifiers::NONE);
        let (new_model, _) = update(model, Msg::KeyPressed(key));
        assert!(new_model.banner.is_none());
    }

    #[test]
    fn esc_key_dismisses_banner_via_handler() {
        use super::super::components::banner::BannerVariant;
        let model = Model::initial();
        // Show a banner
        let (model, _) = update(
            model,
            Msg::BannerShow {
                message: "Test".into(),
                variant: BannerVariant::Success,
            },
        );

        // Press Esc — BannerHandler should capture it (not GlobalHandler's Esc→Pop)
        let key = make_key(KeyCode::Esc, KeyModifiers::NONE);
        let (new_model, _) = update(model, Msg::KeyPressed(key));
        assert!(new_model.banner.is_none());
    }

    // ── Ticket operation tests ──────────────────────────────────────

    #[test]
    fn ticket_op_dispatch_sets_status_and_produces_cmd() {
        use crate::msg::TicketOpMsg;
        let model = Model::initial();
        let (new_model, cmds) = update(
            model,
            Msg::TicketOp(TicketOpMsg::Dispatch {
                ticket_id: "ur-abc".into(),
                project_key: "ur".into(),
                image_id: "img".into(),
            }),
        );
        assert!(new_model.status.is_some());
        assert!(new_model.status.as_ref().unwrap().text.contains("ur-abc"));
        assert!(cmds.iter().any(|c| matches!(c, Cmd::TicketOp(_))));
    }

    #[test]
    fn ticket_op_close_sets_status_and_produces_cmd() {
        use crate::msg::TicketOpMsg;
        let model = Model::initial();
        let (new_model, cmds) = update(
            model,
            Msg::TicketOp(TicketOpMsg::Close {
                ticket_id: "ur-xyz".into(),
            }),
        );
        assert!(new_model.status.is_some());
        assert!(new_model.status.as_ref().unwrap().text.contains("Closing"));
        assert!(cmds.iter().any(|c| matches!(c, Cmd::TicketOp(_))));
    }

    #[test]
    fn ticket_op_force_close_sets_status() {
        use crate::msg::TicketOpMsg;
        let model = Model::initial();
        let (new_model, cmds) = update(
            model,
            Msg::TicketOp(TicketOpMsg::ForceClose {
                ticket_id: "ur-fc".into(),
            }),
        );
        assert!(new_model.status.is_some());
        assert!(
            new_model
                .status
                .as_ref()
                .unwrap()
                .text
                .contains("Force-closing")
        );
        assert!(cmds.iter().any(|c| matches!(c, Cmd::TicketOp(_))));
    }

    #[test]
    fn ticket_op_set_priority_sets_status() {
        use crate::msg::TicketOpMsg;
        let model = Model::initial();
        let (new_model, cmds) = update(
            model,
            Msg::TicketOp(TicketOpMsg::SetPriority {
                ticket_id: "ur-pri".into(),
                priority: 2,
            }),
        );
        assert!(new_model.status.is_some());
        assert!(new_model.status.as_ref().unwrap().text.contains("P2"));
        assert!(cmds.iter().any(|c| matches!(c, Cmd::TicketOp(_))));
    }

    #[test]
    fn ticket_op_create_sets_status() {
        use crate::msg::{PendingTicket, TicketOpMsg};
        let model = Model::initial();
        let (new_model, cmds) = update(
            model,
            Msg::TicketOp(TicketOpMsg::Create {
                pending: PendingTicket {
                    project: "ur".into(),
                    title: "Test ticket".into(),
                    ticket_type: "code".into(),
                    priority: 0,
                    body: String::new(),
                    parent_id: None,
                },
            }),
        );
        assert!(new_model.status.is_some());
        assert!(new_model.status.as_ref().unwrap().text.contains("Creating"));
        assert!(cmds.iter().any(|c| matches!(c, Cmd::TicketOp(_))));
    }

    #[test]
    fn ticket_op_launch_design_sets_status() {
        use crate::msg::TicketOpMsg;
        let model = Model::initial();
        let (new_model, cmds) = update(
            model,
            Msg::TicketOp(TicketOpMsg::LaunchDesign {
                ticket_id: "ur-des".into(),
                project_key: "ur".into(),
                image_id: "img".into(),
            }),
        );
        assert!(new_model.status.is_some());
        assert!(
            new_model
                .status
                .as_ref()
                .unwrap()
                .text
                .contains("design worker")
        );
        assert!(cmds.iter().any(|c| matches!(c, Cmd::TicketOp(_))));
    }

    #[test]
    fn ticket_op_redrive_sets_status() {
        use crate::msg::TicketOpMsg;
        let model = Model::initial();
        let (new_model, cmds) = update(
            model,
            Msg::TicketOp(TicketOpMsg::Redrive {
                ticket_id: "ur-red".into(),
            }),
        );
        assert!(new_model.status.is_some());
        assert!(new_model.status.as_ref().unwrap().text.contains("Moving"));
        assert!(cmds.iter().any(|c| matches!(c, Cmd::TicketOp(_))));
    }

    #[test]
    fn ticket_op_dispatch_all_sets_status() {
        use crate::msg::TicketOpMsg;
        let model = Model::initial();
        let (new_model, cmds) = update(
            model,
            Msg::TicketOp(TicketOpMsg::DispatchAll {
                ticket_id: "ur-all".into(),
                project_key: "ur".into(),
                image_id: "img".into(),
            }),
        );
        assert!(new_model.status.is_some());
        assert!(
            new_model
                .status
                .as_ref()
                .unwrap()
                .text
                .contains("Dispatching")
        );
        assert!(cmds.iter().any(|c| matches!(c, Cmd::TicketOp(_))));
    }

    // ── Ticket operation result tests ───────────────────────────────

    #[test]
    fn ticket_op_result_dispatched_success_shows_banner() {
        use super::super::components::banner::BannerVariant;
        use crate::msg::TicketOpResultMsg;
        let model = Model::initial();
        let (new_model, _) = update(
            model,
            Msg::TicketOpResult(TicketOpResultMsg::Dispatched {
                result: Ok("Dispatched ur-abc".into()),
            }),
        );
        assert!(new_model.status.is_none());
        assert!(new_model.banner.is_some());
        let banner = new_model.banner.as_ref().unwrap();
        assert_eq!(banner.variant, BannerVariant::Success);
        assert!(banner.message.contains("Dispatched"));
    }

    #[test]
    fn ticket_op_result_dispatched_error_shows_error_banner() {
        use super::super::components::banner::BannerVariant;
        use crate::msg::TicketOpResultMsg;
        let model = Model::initial();
        let (new_model, _) = update(
            model,
            Msg::TicketOpResult(TicketOpResultMsg::Dispatched {
                result: Err("connection refused".into()),
            }),
        );
        assert!(new_model.status.is_none());
        assert!(new_model.banner.is_some());
        let banner = new_model.banner.as_ref().unwrap();
        assert_eq!(banner.variant, BannerVariant::Error);
        assert!(banner.message.contains("connection refused"));
    }

    #[test]
    fn ticket_op_result_closed_success_shows_banner() {
        use super::super::components::banner::BannerVariant;
        use crate::msg::TicketOpResultMsg;
        let model = Model::initial();
        let (new_model, _) = update(
            model,
            Msg::TicketOpResult(TicketOpResultMsg::Closed {
                result: Ok("ur-abc → closed".into()),
            }),
        );
        assert!(new_model.banner.is_some());
        assert_eq!(
            new_model.banner.as_ref().unwrap().variant,
            BannerVariant::Success
        );
    }

    #[test]
    fn ticket_op_result_force_closed_success_is_silent() {
        use crate::msg::TicketOpResultMsg;
        let model = Model::initial();
        let (new_model, _) = update(
            model,
            Msg::TicketOpResult(TicketOpResultMsg::ForceClosed {
                result: Ok("ur-abc → closed (force)".into()),
            }),
        );
        // Force close success is silent (no banner).
        assert!(new_model.banner.is_none());
        assert!(new_model.status.is_none());
    }

    #[test]
    fn ticket_op_result_force_closed_error_shows_banner() {
        use super::super::components::banner::BannerVariant;
        use crate::msg::TicketOpResultMsg;
        let model = Model::initial();
        let (new_model, _) = update(
            model,
            Msg::TicketOpResult(TicketOpResultMsg::ForceClosed {
                result: Err("rpc failed".into()),
            }),
        );
        assert!(new_model.banner.is_some());
        assert_eq!(
            new_model.banner.as_ref().unwrap().variant,
            BannerVariant::Error
        );
    }

    #[test]
    fn ticket_op_result_priority_set_success_is_silent() {
        use crate::msg::TicketOpResultMsg;
        let model = Model::initial();
        let (new_model, _) = update(
            model,
            Msg::TicketOpResult(TicketOpResultMsg::PrioritySet {
                result: Ok("Priority set to P2 for ur-abc".into()),
            }),
        );
        // Priority set success is silent.
        assert!(new_model.banner.is_none());
    }

    #[test]
    fn ticket_op_result_priority_set_error_shows_banner() {
        use super::super::components::banner::BannerVariant;
        use crate::msg::TicketOpResultMsg;
        let model = Model::initial();
        let (new_model, _) = update(
            model,
            Msg::TicketOpResult(TicketOpResultMsg::PrioritySet {
                result: Err("update failed".into()),
            }),
        );
        assert!(new_model.banner.is_some());
        assert_eq!(
            new_model.banner.as_ref().unwrap().variant,
            BannerVariant::Error
        );
    }

    #[test]
    fn ticket_op_result_created_success_shows_banner() {
        use super::super::components::banner::BannerVariant;
        use crate::msg::TicketOpResultMsg;
        let model = Model::initial();
        let (new_model, _) = update(
            model,
            Msg::TicketOpResult(TicketOpResultMsg::Created {
                result: Ok("Created ur-new".into()),
                pending: None,
            }),
        );
        assert!(new_model.banner.is_some());
        assert_eq!(
            new_model.banner.as_ref().unwrap().variant,
            BannerVariant::Success
        );
    }

    #[test]
    fn ticket_op_result_created_error_shows_banner_and_action_menu() {
        use super::super::components::banner::BannerVariant;
        use crate::model::ActiveOverlay;
        use crate::msg::{PendingTicket, TicketOpResultMsg};
        let model = Model::initial();
        let pending = PendingTicket {
            project: "ur".into(),
            title: "Test ticket".into(),
            ticket_type: "code".into(),
            priority: 0,
            body: "Some body".into(),
            parent_id: None,
        };
        let (new_model, _) = update(
            model,
            Msg::TicketOpResult(TicketOpResultMsg::Created {
                result: Err("connection refused".into()),
                pending: Some(pending),
            }),
        );
        // Error banner is shown
        assert!(new_model.banner.is_some());
        assert_eq!(
            new_model.banner.as_ref().unwrap().variant,
            BannerVariant::Error
        );
        // Create action menu is re-opened with the preserved pending ticket
        assert!(matches!(
            new_model.active_overlay,
            Some(ActiveOverlay::CreateActionMenu { .. })
        ));
    }

    #[test]
    fn ticket_op_result_created_error_without_pending_shows_only_banner() {
        use super::super::components::banner::BannerVariant;
        use crate::msg::TicketOpResultMsg;
        let model = Model::initial();
        let (new_model, _) = update(
            model,
            Msg::TicketOpResult(TicketOpResultMsg::Created {
                result: Err("connection refused".into()),
                pending: None,
            }),
        );
        assert!(new_model.banner.is_some());
        assert_eq!(
            new_model.banner.as_ref().unwrap().variant,
            BannerVariant::Error
        );
        assert!(new_model.active_overlay.is_none());
    }

    #[test]
    fn ticket_op_result_design_launched_success_shows_banner() {
        use super::super::components::banner::BannerVariant;
        use crate::msg::TicketOpResultMsg;
        let model = Model::initial();
        let (new_model, _) = update(
            model,
            Msg::TicketOpResult(TicketOpResultMsg::DesignLaunched {
                result: Ok("Launched design worker for ur-des".into()),
            }),
        );
        assert!(new_model.banner.is_some());
        assert_eq!(
            new_model.banner.as_ref().unwrap().variant,
            BannerVariant::Success
        );
    }

    #[test]
    fn ticket_op_result_redriven_success_shows_banner() {
        use super::super::components::banner::BannerVariant;
        use crate::msg::TicketOpResultMsg;
        let model = Model::initial();
        let (new_model, _) = update(
            model,
            Msg::TicketOpResult(TicketOpResultMsg::Redriven {
                result: Ok("Moved ur-red to Verify".into()),
            }),
        );
        assert!(new_model.banner.is_some());
        assert_eq!(
            new_model.banner.as_ref().unwrap().variant,
            BannerVariant::Success
        );
    }

    #[test]
    fn ticket_op_result_clears_status_before_banner() {
        use crate::msg::TicketOpResultMsg;
        // Set a status first, then handle a result.
        let model = Model::initial();
        let (model, _) = update(model, Msg::StatusShow("In progress...".into()));
        assert!(model.status.is_some());

        let (new_model, _) = update(
            model,
            Msg::TicketOpResult(TicketOpResultMsg::Dispatched {
                result: Ok("Done".into()),
            }),
        );
        assert!(new_model.status.is_none());
    }

    // ── Flow operation tests ──────────────────────────────────────────

    #[test]
    fn flow_op_cancel_sets_status_and_cmd() {
        use crate::msg::FlowOpMsg;
        let model = Model::initial();
        let (new_model, cmds) = update(
            model,
            Msg::FlowOp(FlowOpMsg::Cancel {
                ticket_id: "ur-abc".into(),
            }),
        );
        assert!(new_model.status.is_some());
        assert!(
            new_model
                .status
                .as_ref()
                .unwrap()
                .text
                .contains("Cancelling")
        );
        assert!(cmds.iter().any(|c| matches!(c, Cmd::FlowOp(_))));
    }

    #[test]
    fn flow_op_result_cancelled_success_shows_banner() {
        use super::super::components::banner::BannerVariant;
        use crate::msg::FlowOpResultMsg;
        let model = Model::initial();
        let (new_model, _) = update(
            model,
            Msg::FlowOpResult(FlowOpResultMsg::Cancelled {
                result: Ok("Cancelled workflow for ur-abc".into()),
            }),
        );
        assert!(new_model.banner.is_some());
        assert_eq!(
            new_model.banner.as_ref().unwrap().variant,
            BannerVariant::Success
        );
    }

    #[test]
    fn flow_op_result_cancelled_error_shows_error_banner() {
        use super::super::components::banner::BannerVariant;
        use crate::msg::FlowOpResultMsg;
        let model = Model::initial();
        let (new_model, _) = update(
            model,
            Msg::FlowOpResult(FlowOpResultMsg::Cancelled {
                result: Err("connection refused".into()),
            }),
        );
        assert!(new_model.banner.is_some());
        assert_eq!(
            new_model.banner.as_ref().unwrap().variant,
            BannerVariant::Error
        );
    }

    #[test]
    fn flow_op_result_clears_status_before_banner() {
        use crate::msg::FlowOpResultMsg;
        let model = Model::initial();
        let (model, _) = update(model, Msg::StatusShow("Cancelling...".into()));
        assert!(model.status.is_some());

        let (new_model, _) = update(
            model,
            Msg::FlowOpResult(FlowOpResultMsg::Cancelled {
                result: Ok("Done".into()),
            }),
        );
        assert!(new_model.status.is_none());
    }

    // ── Worker operation tests ────────────────────────────────────────

    #[test]
    fn worker_op_kill_sets_status_and_cmd() {
        use crate::msg::WorkerOpMsg;
        let model = Model::initial();
        let (new_model, cmds) = update(
            model,
            Msg::WorkerOp(WorkerOpMsg::Kill {
                worker_id: "wk-123".into(),
            }),
        );
        assert!(new_model.status.is_some());
        assert!(new_model.status.as_ref().unwrap().text.contains("Killing"));
        assert!(cmds.iter().any(|c| matches!(c, Cmd::WorkerOp(_))));
    }

    #[test]
    fn worker_op_result_killed_success_shows_banner() {
        use super::super::components::banner::BannerVariant;
        use crate::msg::WorkerOpResultMsg;
        let model = Model::initial();
        let (new_model, _) = update(
            model,
            Msg::WorkerOpResult(WorkerOpResultMsg::Killed {
                result: Ok("Killed worker wk-123".into()),
            }),
        );
        assert!(new_model.banner.is_some());
        assert_eq!(
            new_model.banner.as_ref().unwrap().variant,
            BannerVariant::Success
        );
    }

    #[test]
    fn worker_op_result_killed_error_shows_error_banner() {
        use super::super::components::banner::BannerVariant;
        use crate::msg::WorkerOpResultMsg;
        let model = Model::initial();
        let (new_model, _) = update(
            model,
            Msg::WorkerOpResult(WorkerOpResultMsg::Killed {
                result: Err("worker not found".into()),
            }),
        );
        assert!(new_model.banner.is_some());
        assert_eq!(
            new_model.banner.as_ref().unwrap().variant,
            BannerVariant::Error
        );
    }

    #[test]
    fn worker_op_result_clears_status_before_banner() {
        use crate::msg::WorkerOpResultMsg;
        let model = Model::initial();
        let (model, _) = update(model, Msg::StatusShow("Killing worker...".into()));
        assert!(model.status.is_some());

        let (new_model, _) = update(
            model,
            Msg::WorkerOpResult(WorkerOpResultMsg::Killed {
                result: Ok("Done".into()),
            }),
        );
        assert!(new_model.status.is_none());
    }
}
