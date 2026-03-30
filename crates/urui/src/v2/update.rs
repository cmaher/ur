use super::cmd::{Cmd, FetchCmd};
use super::model::{
    FlowListData, LoadState, Model, TicketActivitiesData, TicketDetailData, TicketDetailModel,
    TicketListData, WorkerListData,
};
use super::msg::{DataMsg, Msg, NavMsg};

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
        Msg::Tick => (model, vec![]),
        Msg::Data(data_msg) => handle_data(model, *data_msg),
        Msg::Nav(nav_msg) => handle_nav(model, nav_msg),
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
    fn tick_is_noop() {
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
        use crate::v2::navigation::{PageId, TabId};
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
}
