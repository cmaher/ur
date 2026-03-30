use super::cmd::{Cmd, FetchCmd};
use super::model::{
    FlowListData, LoadState, Model, TicketActivitiesData, TicketDetailData, TicketDetailModel,
    TicketListData, WorkerListData,
};
use super::msg::{DataMsg, Msg, NavMsg, UiEventItem};
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
    }
}

/// Handle a key press event by dispatching through the input stack.
/// The input stack walks handlers top-to-bottom; the first capture wins.
/// If a handler captures the key and produces a message, that message is
/// fed back through update() recursively.
fn handle_key(model: Model, key: crossterm::event::KeyEvent) -> (Model, Vec<Cmd>) {
    match model.input_stack.dispatch(key) {
        Some(msg) => update(model, msg),
        None => (model, vec![]),
    }
}

/// Handle a navigation message by delegating to the NavigationModel.
///
/// Takes ownership of the NavigationModel to avoid double-borrow of `model`,
/// since navigation methods need `&mut Model` for input stack manipulation.
fn handle_nav(mut model: Model, nav_msg: NavMsg) -> (Model, Vec<Cmd>) {
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
    };
    model.navigation_model = nav;
    (model, cmds)
}

/// Handle a tick: check if the throttle cooldown has elapsed and flush dirty tabs.
fn handle_tick(mut model: Model) -> (Model, Vec<Cmd>) {
    if model.ui_event_throttle.should_flush() {
        let cmds = flush_throttle(&mut model);
        (model, cmds)
    } else {
        (model, vec![])
    }
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
            let mut cmds = vec![Cmd::Fetch(FetchCmd::Tickets {
                page_size: None,
                offset: None,
                include_children: None,
                statuses: vec![],
            })];
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
    }
}

/// Set a non-active tab's data back to `NotLoaded` so it re-fetches when viewed.
fn invalidate_tab(tab: TabId, model: &mut Model) {
    match tab {
        TabId::Tickets => model.ticket_list.data = LoadState::NotLoaded,
        TabId::Flows => model.flow_list.data = LoadState::NotLoaded,
        TabId::Workers => model.worker_list.data = LoadState::NotLoaded,
    }
}

/// Handle a data message by updating the appropriate sub-model's `LoadState`.
fn handle_data(mut model: Model, data_msg: DataMsg) -> (Model, Vec<Cmd>) {
    match data_msg {
        DataMsg::TicketsLoaded(result) => {
            model.ticket_list.data = match result {
                Ok((tickets, total_count)) => LoadState::Loaded(TicketListData {
                    tickets,
                    total_count,
                }),
                Err(e) => LoadState::Error(e),
            };
        }
        DataMsg::DetailLoaded(result) => {
            if let Some(ref mut detail_model) = model.ticket_detail {
                detail_model.data = match *result {
                    Ok((detail, children, total_children)) => LoadState::Loaded(TicketDetailData {
                        detail,
                        children,
                        total_children,
                    }),
                    Err(e) => LoadState::Error(e),
                };
            }
        }
        DataMsg::FlowsLoaded(result) => {
            model.flow_list.data = match result {
                Ok((workflows, total_count)) => LoadState::Loaded(FlowListData {
                    workflows,
                    total_count,
                }),
                Err(e) => LoadState::Error(e),
            };
        }
        DataMsg::WorkersLoaded(result) => {
            model.worker_list.data = match result {
                Ok(workers) => LoadState::Loaded(WorkerListData { workers }),
                Err(e) => LoadState::Error(e),
            };
        }
        DataMsg::ActivitiesLoaded { ticket_id, result } => {
            if let Some(ref mut detail_model) = model.ticket_detail {
                if detail_model.ticket_id == ticket_id {
                    detail_model.activities = match result {
                        Ok(activities) => LoadState::Loaded(TicketActivitiesData { activities }),
                        Err(e) => LoadState::Error(e),
                    };
                }
            }
        }
    }
    (model, vec![])
}

/// Build a batch of fetch commands to refresh all page data.
/// Called on `NavPop` or explicit refresh to re-fetch potentially stale data.
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
    Cmd::Fetch(FetchCmd::Tickets {
        page_size: None,
        offset: None,
        include_children: None,
        statuses: vec![],
    })
}

/// Set a sub-model to `Loading` and return the corresponding fetch command
/// for the ticket detail page.
pub fn start_ticket_detail_fetch(model: &mut Model, ticket_id: String) -> Cmd {
    model.ticket_detail = Some(TicketDetailModel {
        ticket_id: ticket_id.clone(),
        data: LoadState::Loading,
        activities: LoadState::Loading,
    });
    Cmd::batch(vec![
        Cmd::Fetch(FetchCmd::TicketDetail {
            ticket_id: ticket_id.clone(),
            child_page_size: None,
            child_offset: None,
            child_status_filter: None,
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
        use crate::v2::navigation::TabId;
        let model = Model::initial();
        let (new_model, _cmds) = update(model, Msg::Nav(NavMsg::TabSwitch(TabId::Flows)));
        assert_eq!(new_model.navigation_model.active_tab, TabId::Flows);
    }

    #[test]
    fn nav_tab_next_cycles_tab() {
        use crate::v2::navigation::TabId;
        let model = Model::initial();
        assert_eq!(model.navigation_model.active_tab, TabId::Tickets);
        let (new_model, _cmds) = update(model, Msg::Nav(NavMsg::TabNext));
        assert_eq!(new_model.navigation_model.active_tab, TabId::Flows);
    }

    #[test]
    fn nav_push_adds_page() {
        use crate::v2::navigation::PageId;
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
        use crate::v2::navigation::PageId;
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
        use crate::v2::navigation::PageId;
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
        use crate::v2::navigation::PageId;
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
        });
        let cmd = fetch_cmd_for_tab(TabId::Tickets, &model);
        // Should be a Batch containing ticket list + detail + activities.
        assert!(matches!(cmd, Cmd::Batch(_)));
    }
}
