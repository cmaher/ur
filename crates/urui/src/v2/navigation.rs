use std::collections::HashMap;

use super::cmd::Cmd;
use super::model::Model;

/// Identifies a top-level tab in the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TabId {
    Tickets,
    Flows,
    Workers,
}

impl TabId {
    /// Returns all tab variants in display order.
    pub fn all() -> &'static [TabId] {
        &[TabId::Tickets, TabId::Flows, TabId::Workers]
    }

    /// Returns the next tab in display order, wrapping around.
    pub fn next(self) -> TabId {
        match self {
            TabId::Tickets => TabId::Flows,
            TabId::Flows => TabId::Workers,
            TabId::Workers => TabId::Tickets,
        }
    }

    /// Returns the display name for this tab.
    pub fn label(self) -> &'static str {
        match self {
            TabId::Tickets => "Tickets",
            TabId::Flows => "Flows",
            TabId::Workers => "Workers",
        }
    }
}

/// Identifies a page within the navigation stack.
///
/// Each tab maintains a stack of pages. The bottom page is always the
/// tab's root page; detail pages are pushed on top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageId {
    /// The root ticket list page.
    TicketList,
    /// Detail page for a specific ticket.
    TicketDetail { ticket_id: String },
    /// Activities page for a specific ticket.
    TicketActivities {
        ticket_id: String,
        ticket_title: String,
    },
    /// Body page for a specific ticket.
    TicketBody {
        ticket_id: String,
        title: String,
        body: String,
    },
    /// The root flows list page.
    FlowList,
    /// Detail page for a specific flow/workflow.
    FlowDetail { ticket_id: String },
    /// The root workers list page.
    WorkerList,
}

/// Navigation state: tracks the active tab and per-tab page stacks.
///
/// Each tab has its own stack of `PageId`s. The active tab's top page
/// determines what is rendered. Push/pop operations modify the active
/// tab's stack.
#[derive(Debug, Clone)]
pub struct NavigationModel {
    /// The currently active tab.
    pub active_tab: TabId,
    /// Per-tab page stacks. Each stack always has at least one entry (the root).
    pub tab_stacks: HashMap<TabId, Vec<PageId>>,
}

impl NavigationModel {
    /// Create the initial navigation state with each tab having its root page.
    pub fn initial() -> Self {
        let mut tab_stacks = HashMap::new();
        tab_stacks.insert(TabId::Tickets, vec![PageId::TicketList]);
        tab_stacks.insert(TabId::Flows, vec![PageId::FlowList]);
        tab_stacks.insert(TabId::Workers, vec![PageId::WorkerList]);
        Self {
            active_tab: TabId::Tickets,
            tab_stacks,
        }
    }

    /// Returns the current page (top of the active tab's stack).
    pub fn current_page(&self) -> &PageId {
        self.tab_stacks
            .get(&self.active_tab)
            .and_then(|stack| stack.last())
            .expect("tab stack should never be empty")
    }

    /// Returns the depth of the active tab's stack (1 = root only).
    pub fn active_stack_depth(&self) -> usize {
        self.tab_stacks
            .get(&self.active_tab)
            .map(|s| s.len())
            .unwrap_or(1)
    }

    /// Push a page onto the active tab's stack.
    /// Returns the init commands from the page's component lifecycle.
    pub fn push(&mut self, page: PageId, model: &mut Model) -> Vec<Cmd> {
        let stack = self
            .tab_stacks
            .get_mut(&self.active_tab)
            .expect("tab stack should exist");
        stack.push(page);
        init_page(self.current_page(), model)
    }

    /// Pop the top page from the active tab's stack.
    /// Does nothing if the stack has only the root page.
    /// Returns teardown + init commands for the lifecycle transition.
    pub fn pop(&mut self, model: &mut Model) -> Vec<Cmd> {
        let stack = self
            .tab_stacks
            .get_mut(&self.active_tab)
            .expect("tab stack should exist");

        if stack.len() <= 1 {
            return vec![];
        }

        let popped = stack.pop().expect("stack has more than one entry");
        let mut cmds = vec![];

        // Teardown the popped page
        let handler_count = teardown_page(&popped, model);
        for _ in 0..handler_count {
            model.input_stack.pop();
        }

        // Init the newly revealed page
        cmds.extend(init_page(self.current_page(), model));
        cmds
    }

    /// Switch to a different tab, or pop to root if already on the same tab.
    ///
    /// - Same tab: pops all pages except the root (teardown each popped page).
    /// - Different tab: switches active_tab (no teardown/init — stacks persist).
    pub fn switch_tab(&mut self, target: TabId, model: &mut Model) -> Vec<Cmd> {
        if self.active_tab == target {
            self.pop_to_root(model)
        } else {
            self.active_tab = target;
            // Trigger a fetch if the target tab's root data hasn't been loaded yet.
            if is_tab_data_not_loaded(target, model) {
                init_page(self.current_page(), model)
            } else {
                vec![]
            }
        }
    }

    /// Pop all pages from the active tab's stack except the root,
    /// tearing down each in reverse order.
    fn pop_to_root(&mut self, model: &mut Model) -> Vec<Cmd> {
        let mut cmds = vec![];
        let stack = self
            .tab_stacks
            .get_mut(&self.active_tab)
            .expect("tab stack should exist");

        while stack.len() > 1 {
            let popped = stack.pop().expect("stack has more than one entry");
            let handler_count = teardown_page(&popped, model);
            for _ in 0..handler_count {
                model.input_stack.pop();
            }
        }

        // Re-init the root page
        cmds.extend(init_page(self.current_page(), model));
        cmds
    }

    /// Initialize the current page by triggering its data fetch.
    ///
    /// Called at startup to ensure the initial tab's data is loaded immediately
    /// instead of showing a permanent "Loading..." state.
    pub fn init_current_page(&self, model: &mut Model) -> Vec<Cmd> {
        init_page(self.current_page(), model)
    }

    /// Navigate directly to a specific page, pushing it if not already
    /// the current page.
    pub fn goto(&mut self, page: PageId, model: &mut Model) -> Vec<Cmd> {
        if self.current_page() == &page {
            return vec![];
        }
        self.push(page, model)
    }
}

/// Check whether a tab's root page data is in the `NotLoaded` state.
fn is_tab_data_not_loaded(tab: TabId, model: &Model) -> bool {
    match tab {
        TabId::Tickets => matches!(model.ticket_list.data, super::model::LoadState::NotLoaded),
        TabId::Flows => matches!(model.flow_list.data, super::model::LoadState::NotLoaded),
        TabId::Workers => matches!(model.worker_list.data, super::model::LoadState::NotLoaded),
    }
}

/// Initialize a page and return its init commands.
///
/// Page-specific initialization logic (pushing input handlers, starting
/// data fetches) is handled here. Each page type maps to specific setup.
fn init_page(page: &PageId, model: &mut Model) -> Vec<Cmd> {
    use super::pages::ticket_activities::{TicketActivitiesHandler, start_activities_fetch};
    use super::pages::ticket_body::{TicketBodyHandler, init_body_model};
    use super::update::{
        start_flow_list_fetch, start_ticket_detail_fetch, start_ticket_list_fetch,
        start_worker_list_fetch,
    };

    match page {
        PageId::TicketList => {
            let cmd = start_ticket_list_fetch(model);
            vec![cmd]
        }
        PageId::TicketDetail { ticket_id } => {
            let cmd = start_ticket_detail_fetch(model, ticket_id.clone());
            vec![cmd]
        }
        PageId::TicketActivities {
            ticket_id,
            ticket_title,
        } => {
            model.input_stack.push(Box::new(TicketActivitiesHandler));
            let cmd = start_activities_fetch(model, ticket_id.clone(), ticket_title.clone());
            vec![cmd]
        }
        PageId::TicketBody {
            ticket_id,
            title,
            body,
        } => {
            model.input_stack.push(Box::new(TicketBodyHandler));
            init_body_model(model, ticket_id.clone(), title.clone(), body.clone());
            vec![]
        }
        PageId::FlowList => {
            let cmd = start_flow_list_fetch(model);
            vec![cmd]
        }
        PageId::FlowDetail { ticket_id } => {
            super::pages::flow_detail::init_flow_detail(model, ticket_id.clone());
            vec![]
        }
        PageId::WorkerList => {
            let cmd = start_worker_list_fetch(model);
            vec![cmd]
        }
    }
}

/// Teardown a page and return how many input handlers to pop.
///
/// Currently all pages push zero handlers during init (handlers will be
/// added when page-specific components are implemented). The return value
/// ensures the input stack stays consistent.
fn teardown_page(page: &PageId, model: &mut Model) -> usize {
    match page {
        PageId::TicketActivities { .. } => {
            model.ticket_activities = None;
            1 // TicketActivitiesHandler
        }
        PageId::TicketBody { .. } => {
            model.ticket_body = None;
            1 // TicketBodyHandler
        }
        PageId::FlowDetail { .. } => {
            model.flow_detail = None;
            0 // FlowDetailHandler dispatched from root, not pushed
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_navigation_starts_on_tickets() {
        let nav = NavigationModel::initial();
        assert_eq!(nav.active_tab, TabId::Tickets);
        assert_eq!(nav.current_page(), &PageId::TicketList);
    }

    #[test]
    fn all_tabs_have_root_pages() {
        let nav = NavigationModel::initial();
        for tab in TabId::all() {
            let stack = nav.tab_stacks.get(tab).unwrap();
            assert_eq!(stack.len(), 1, "tab {:?} should have root page", tab);
        }
    }

    #[test]
    fn push_adds_page_to_active_stack() {
        let mut nav = NavigationModel::initial();
        let mut model = Model::initial();
        let _cmds = nav.push(
            PageId::TicketDetail {
                ticket_id: "ur-abc".into(),
            },
            &mut model,
        );
        assert_eq!(nav.active_stack_depth(), 2);
        assert_eq!(
            nav.current_page(),
            &PageId::TicketDetail {
                ticket_id: "ur-abc".into()
            }
        );
    }

    #[test]
    fn pop_returns_to_previous_page() {
        let mut nav = NavigationModel::initial();
        let mut model = Model::initial();
        nav.push(
            PageId::TicketDetail {
                ticket_id: "ur-abc".into(),
            },
            &mut model,
        );
        assert_eq!(nav.active_stack_depth(), 2);

        let _cmds = nav.pop(&mut model);
        assert_eq!(nav.active_stack_depth(), 1);
        assert_eq!(nav.current_page(), &PageId::TicketList);
    }

    #[test]
    fn pop_at_root_is_noop() {
        let mut nav = NavigationModel::initial();
        let mut model = Model::initial();
        let cmds = nav.pop(&mut model);
        assert!(cmds.is_empty());
        assert_eq!(nav.active_stack_depth(), 1);
    }

    #[test]
    fn switch_to_different_tab() {
        let mut nav = NavigationModel::initial();
        let mut model = Model::initial();
        let _cmds = nav.switch_tab(TabId::Flows, &mut model);
        assert_eq!(nav.active_tab, TabId::Flows);
        assert_eq!(nav.current_page(), &PageId::FlowList);
    }

    #[test]
    fn switch_to_same_tab_pops_to_root() {
        let mut nav = NavigationModel::initial();
        let mut model = Model::initial();
        // Push a detail page
        nav.push(
            PageId::TicketDetail {
                ticket_id: "ur-abc".into(),
            },
            &mut model,
        );
        assert_eq!(nav.active_stack_depth(), 2);

        // Switch to same tab should pop to root
        let _cmds = nav.switch_tab(TabId::Tickets, &mut model);
        assert_eq!(nav.active_stack_depth(), 1);
        assert_eq!(nav.current_page(), &PageId::TicketList);
    }

    #[test]
    fn tab_next_cycles() {
        assert_eq!(TabId::Tickets.next(), TabId::Flows);
        assert_eq!(TabId::Flows.next(), TabId::Workers);
        assert_eq!(TabId::Workers.next(), TabId::Tickets);
    }

    #[test]
    fn tab_labels() {
        assert_eq!(TabId::Tickets.label(), "Tickets");
        assert_eq!(TabId::Flows.label(), "Flows");
        assert_eq!(TabId::Workers.label(), "Workers");
    }

    #[test]
    fn goto_pushes_new_page() {
        let mut nav = NavigationModel::initial();
        let mut model = Model::initial();
        let _cmds = nav.goto(
            PageId::TicketDetail {
                ticket_id: "ur-xyz".into(),
            },
            &mut model,
        );
        assert_eq!(nav.active_stack_depth(), 2);
    }

    #[test]
    fn goto_same_page_is_noop() {
        let mut nav = NavigationModel::initial();
        let mut model = Model::initial();
        let cmds = nav.goto(PageId::TicketList, &mut model);
        assert!(cmds.is_empty());
        assert_eq!(nav.active_stack_depth(), 1);
    }

    #[test]
    fn push_returns_init_commands() {
        let mut nav = NavigationModel::initial();
        let mut model = Model::initial();
        let cmds = nav.push(
            PageId::TicketDetail {
                ticket_id: "ur-test".into(),
            },
            &mut model,
        );
        // Should have returned fetch commands from init
        assert!(!cmds.is_empty());
    }

    #[test]
    fn pop_returns_init_commands_for_revealed_page() {
        let mut nav = NavigationModel::initial();
        let mut model = Model::initial();
        nav.push(
            PageId::TicketDetail {
                ticket_id: "ur-test".into(),
            },
            &mut model,
        );
        let cmds = nav.pop(&mut model);
        // Should have init commands for the revealed root page
        assert!(!cmds.is_empty());
    }

    #[test]
    fn different_tabs_maintain_independent_stacks() {
        let mut nav = NavigationModel::initial();
        let mut model = Model::initial();

        // Push detail on tickets tab
        nav.push(
            PageId::TicketDetail {
                ticket_id: "ur-abc".into(),
            },
            &mut model,
        );
        assert_eq!(nav.active_stack_depth(), 2);

        // Switch to flows tab
        nav.switch_tab(TabId::Flows, &mut model);
        assert_eq!(nav.active_stack_depth(), 1);

        // Switch back to tickets — stack should still have 2
        nav.switch_tab(TabId::Tickets, &mut model);
        // Same-tab switch pops to root, but we're switching TO tickets
        // from flows, so it's a different-tab switch — stack preserved
        // Wait: we switched FROM flows TO tickets, that's a different tab
        // so the stack should be preserved with depth 2
        assert_eq!(nav.active_stack_depth(), 2);
    }

    #[test]
    fn pop_to_root_pops_multiple_pages() {
        let mut nav = NavigationModel::initial();
        let mut model = Model::initial();

        nav.push(
            PageId::TicketDetail {
                ticket_id: "ur-1".into(),
            },
            &mut model,
        );
        nav.push(
            PageId::TicketDetail {
                ticket_id: "ur-2".into(),
            },
            &mut model,
        );
        assert_eq!(nav.active_stack_depth(), 3);

        // Same-tab switch pops to root
        let _cmds = nav.switch_tab(TabId::Tickets, &mut model);
        assert_eq!(nav.active_stack_depth(), 1);
        assert_eq!(nav.current_page(), &PageId::TicketList);
    }

    #[test]
    fn handler_stack_consistent_after_push_pop() {
        let mut nav = NavigationModel::initial();
        let mut model = Model::initial();
        let initial_handler_count = model.input_stack.len();

        // Push a page (currently pages push 0 handlers)
        nav.push(
            PageId::TicketDetail {
                ticket_id: "ur-abc".into(),
            },
            &mut model,
        );
        assert_eq!(model.input_stack.len(), initial_handler_count);

        // Pop the page
        nav.pop(&mut model);
        assert_eq!(model.input_stack.len(), initial_handler_count);
    }

    #[test]
    fn init_current_page_returns_fetch_commands() {
        let nav = NavigationModel::initial();
        let mut model = Model::initial();
        // Starting tab is Tickets with NotLoaded data — init should produce cmds.
        let cmds = nav.init_current_page(&mut model);
        assert!(!cmds.is_empty(), "startup init should trigger a fetch");
    }

    #[test]
    fn switch_tab_triggers_fetch_when_not_loaded() {
        let mut nav = NavigationModel::initial();
        let mut model = Model::initial();
        // Flows data starts as NotLoaded, so switching should trigger fetch.
        let cmds = nav.switch_tab(TabId::Flows, &mut model);
        assert!(
            !cmds.is_empty(),
            "switching to a NotLoaded tab should trigger a fetch"
        );
    }

    #[test]
    fn switch_tab_no_fetch_when_already_loaded() {
        use crate::v2::model::{FlowListData, LoadState};
        let mut nav = NavigationModel::initial();
        let mut model = Model::initial();
        // Simulate that Flows data was already loaded.
        model.flow_list.data = LoadState::Loaded(FlowListData {
            workflows: vec![],
            total_count: 0,
        });
        let cmds = nav.switch_tab(TabId::Flows, &mut model);
        assert!(
            cmds.is_empty(),
            "switching to an already-loaded tab should not trigger a fetch"
        );
    }

    #[test]
    fn switch_tab_refetches_invalidated_data() {
        use crate::v2::model::{FlowListData, LoadState};
        let mut nav = NavigationModel::initial();
        let mut model = Model::initial();
        // Simulate loaded then invalidated (back to NotLoaded).
        model.flow_list.data = LoadState::Loaded(FlowListData {
            workflows: vec![],
            total_count: 0,
        });
        nav.switch_tab(TabId::Flows, &mut model);
        // Invalidate the data.
        model.flow_list.data = LoadState::NotLoaded;
        // Switch away and back.
        nav.switch_tab(TabId::Tickets, &mut model);
        let cmds = nav.switch_tab(TabId::Flows, &mut model);
        assert!(
            !cmds.is_empty(),
            "switching to an invalidated tab should trigger a re-fetch"
        );
    }
}
