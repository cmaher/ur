use ur_rpc::proto::core::WorkerSummary;
use ur_rpc::proto::ticket::{ActivityEntry, GetTicketResponse, Ticket, WorkflowInfo};

use super::input::{GlobalHandler, InputStack};
use super::navigation::NavigationModel;

/// Tracks the loading state of asynchronous data.
///
/// Each page sub-model uses `LoadState<T>` to represent whether its data
/// has not been fetched yet, is currently loading, has been successfully
/// loaded, or failed to load.
#[derive(Debug, Clone)]
pub enum LoadState<T> {
    /// Data has not been requested yet.
    NotLoaded,
    /// A fetch is in progress.
    Loading,
    /// Data was successfully loaded.
    Loaded(T),
    /// The fetch failed with an error message.
    Error(String),
}

impl<T> LoadState<T> {
    /// Returns `true` if the state is `Loaded`.
    pub fn is_loaded(&self) -> bool {
        matches!(self, LoadState::Loaded(_))
    }

    /// Returns `true` if the state is `Loading`.
    pub fn is_loading(&self) -> bool {
        matches!(self, LoadState::Loading)
    }

    /// Returns a reference to the loaded data, if available.
    pub fn data(&self) -> Option<&T> {
        match self {
            LoadState::Loaded(data) => Some(data),
            _ => None,
        }
    }
}

/// Data loaded for the ticket list page.
#[derive(Debug, Clone)]
pub struct TicketListData {
    pub tickets: Vec<Ticket>,
    pub total_count: i32,
}

/// Data loaded for the ticket detail page.
#[derive(Debug, Clone)]
pub struct TicketDetailData {
    pub detail: GetTicketResponse,
    pub children: Vec<Ticket>,
    pub total_children: i32,
}

/// Data loaded for the flows page.
#[derive(Debug, Clone)]
pub struct FlowListData {
    pub workflows: Vec<WorkflowInfo>,
    pub total_count: i32,
}

/// Data loaded for the workers page.
#[derive(Debug, Clone)]
pub struct WorkerListData {
    pub workers: Vec<WorkerSummary>,
}

/// Data loaded for ticket activities.
#[derive(Debug, Clone)]
pub struct TicketActivitiesData {
    pub activities: Vec<ActivityEntry>,
}

/// Sub-model for the ticket list page.
#[derive(Debug, Clone)]
pub struct TicketListModel {
    pub data: LoadState<TicketListData>,
}

/// Sub-model for the ticket detail page.
#[derive(Debug, Clone)]
pub struct TicketDetailModel {
    pub ticket_id: String,
    pub data: LoadState<TicketDetailData>,
    pub activities: LoadState<TicketActivitiesData>,
}

/// Sub-model for the flows page.
#[derive(Debug, Clone)]
pub struct FlowListModel {
    pub data: LoadState<FlowListData>,
}

/// Sub-model for the workers page.
#[derive(Debug, Clone)]
pub struct WorkerListModel {
    pub data: LoadState<WorkerListData>,
}

/// The top-level application model for the v2 TEA architecture.
///
/// This struct holds all application state. It is owned by the main loop and
/// passed (by value) to the pure `update` function, which returns a new `Model`.
#[derive(Debug, Clone)]
pub struct Model {
    /// When true, the main loop should exit.
    pub should_quit: bool,
    /// Placeholder for future navigation state (active tab, page stack, etc.).
    pub navigation_model: NavigationModel,
    /// The input focus stack. Handlers are walked top-to-bottom on each key
    /// event; the first to capture wins. Also collects footer commands.
    pub input_stack: InputStack,
    /// Sub-model for the ticket list page.
    pub ticket_list: TicketListModel,
    /// Sub-model for the ticket detail page (set when viewing a ticket).
    pub ticket_detail: Option<TicketDetailModel>,
    /// Sub-model for the flows page.
    pub flow_list: FlowListModel,
    /// Sub-model for the workers page.
    pub worker_list: WorkerListModel,
}

impl Model {
    /// Create the initial application model.
    pub fn initial() -> Self {
        let mut input_stack = InputStack::default();
        input_stack.push(Box::new(GlobalHandler));
        Self {
            should_quit: false,
            navigation_model: NavigationModel::initial(),
            input_stack,
            ticket_list: TicketListModel {
                data: LoadState::NotLoaded,
            },
            ticket_detail: None,
            flow_list: FlowListModel {
                data: LoadState::NotLoaded,
            },
            worker_list: WorkerListModel {
                data: LoadState::NotLoaded,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_model_does_not_quit() {
        let model = Model::initial();
        assert!(!model.should_quit);
    }

    #[test]
    fn load_state_not_loaded_is_not_loaded() {
        let state: LoadState<String> = LoadState::NotLoaded;
        assert!(!state.is_loaded());
        assert!(!state.is_loading());
        assert!(state.data().is_none());
    }

    #[test]
    fn load_state_loading_is_loading() {
        let state: LoadState<String> = LoadState::Loading;
        assert!(state.is_loading());
        assert!(!state.is_loaded());
        assert!(state.data().is_none());
    }

    #[test]
    fn load_state_loaded_has_data() {
        let state = LoadState::Loaded("hello".to_string());
        assert!(state.is_loaded());
        assert!(!state.is_loading());
        assert_eq!(state.data(), Some(&"hello".to_string()));
    }

    #[test]
    fn load_state_error_has_no_data() {
        let state: LoadState<String> = LoadState::Error("oops".to_string());
        assert!(!state.is_loaded());
        assert!(!state.is_loading());
        assert!(state.data().is_none());
    }

    #[test]
    fn initial_model_sub_models_not_loaded() {
        let model = Model::initial();
        assert!(matches!(model.ticket_list.data, LoadState::NotLoaded));
        assert!(matches!(model.flow_list.data, LoadState::NotLoaded));
        assert!(matches!(model.worker_list.data, LoadState::NotLoaded));
        assert!(model.ticket_detail.is_none());
    }
}
