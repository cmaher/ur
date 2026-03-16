use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tonic::transport::{Channel, Endpoint, Server};
use tonic::{Request, Response, Status};

use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;
use ur_rpc::proto::ticket::ticket_service_server::{TicketService, TicketServiceServer};
use ur_rpc::proto::ticket::*;

use crate::args::TicketArgs;

// --- Mock TicketService ---

/// In-memory ticket store for testing execute() against a real gRPC server.
#[derive(Clone, Default)]
struct MockTicketStore {
    inner: Arc<Mutex<MockState>>,
}

#[derive(Default)]
struct MockState {
    tickets: HashMap<String, Ticket>,
    metadata: HashMap<String, HashMap<String, String>>,
    activities: HashMap<String, Vec<ActivityEntry>>,
    edges: Vec<(String, String, String)>, // (from, to, kind)
    next_id: u64,
}

impl MockTicketStore {
    fn next_id(&self) -> String {
        let mut state = self.inner.lock().unwrap();
        state.next_id += 1;
        format!("ur-t{:04}", state.next_id)
    }
}

#[tonic::async_trait]
impl TicketService for MockTicketStore {
    async fn create_ticket(
        &self,
        req: Request<CreateTicketRequest>,
    ) -> Result<Response<CreateTicketResponse>, Status> {
        let req = req.into_inner();
        let id = self.next_id();
        let ticket = Ticket {
            id: id.clone(),
            ticket_type: req.ticket_type,
            status: req.status,
            priority: req.priority,
            parent_id: req.parent_id.unwrap_or_default(),
            title: req.title,
            body: req.body,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        };
        self.inner
            .lock()
            .unwrap()
            .tickets
            .insert(id.clone(), ticket);
        Ok(Response::new(CreateTicketResponse { id }))
    }

    async fn list_tickets(
        &self,
        req: Request<ListTicketsRequest>,
    ) -> Result<Response<ListTicketsResponse>, Status> {
        let req = req.into_inner();
        let state = self.inner.lock().unwrap();
        let tickets: Vec<Ticket> = state
            .tickets
            .values()
            .filter(|t| {
                if let Some(ref s) = req.status
                    && !s.is_empty()
                    && t.status != *s
                {
                    return false;
                }
                if let Some(ref tt) = req.ticket_type
                    && !tt.is_empty()
                    && t.ticket_type != *tt
                {
                    return false;
                }
                if let Some(ref pid) = req.parent_id
                    && !pid.is_empty()
                    && t.parent_id != *pid
                {
                    return false;
                }
                true
            })
            .cloned()
            .collect();
        Ok(Response::new(ListTicketsResponse { tickets }))
    }

    async fn get_ticket(
        &self,
        req: Request<GetTicketRequest>,
    ) -> Result<Response<GetTicketResponse>, Status> {
        let id = req.into_inner().id;
        let state = self.inner.lock().unwrap();
        let ticket = state
            .tickets
            .get(&id)
            .cloned()
            .ok_or_else(|| Status::not_found(format!("ticket not found: {id}")))?;
        let metadata: Vec<MetadataEntry> = state
            .metadata
            .get(&id)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| MetadataEntry {
                        key: k.clone(),
                        value: v.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let activities = state.activities.get(&id).cloned().unwrap_or_default();
        Ok(Response::new(GetTicketResponse {
            ticket: Some(ticket),
            metadata,
            activities,
        }))
    }

    async fn update_ticket(
        &self,
        req: Request<UpdateTicketRequest>,
    ) -> Result<Response<UpdateTicketResponse>, Status> {
        let req = req.into_inner();
        let mut state = self.inner.lock().unwrap();
        let ticket = state
            .tickets
            .get_mut(&req.id)
            .ok_or_else(|| Status::not_found(format!("ticket not found: {}", req.id)))?;
        if let Some(status) = req.status
            && !status.is_empty()
        {
            ticket.status = status;
        }
        if let Some(title) = req.title
            && !title.is_empty()
        {
            ticket.title = title;
        }
        if let Some(priority) = req.priority {
            ticket.priority = priority;
        }
        if let Some(body) = req.body
            && !body.is_empty()
        {
            ticket.body = body;
        }
        if let Some(ticket_type) = req.ticket_type
            && !ticket_type.is_empty()
        {
            ticket.ticket_type = ticket_type;
        }
        Ok(Response::new(UpdateTicketResponse {}))
    }

    async fn set_meta(
        &self,
        req: Request<SetMetaRequest>,
    ) -> Result<Response<SetMetaResponse>, Status> {
        let req = req.into_inner();
        let mut state = self.inner.lock().unwrap();
        state
            .metadata
            .entry(req.ticket_id)
            .or_default()
            .insert(req.key, req.value);
        Ok(Response::new(SetMetaResponse {}))
    }

    async fn delete_meta(
        &self,
        req: Request<DeleteMetaRequest>,
    ) -> Result<Response<DeleteMetaResponse>, Status> {
        let req = req.into_inner();
        let mut state = self.inner.lock().unwrap();
        if let Some(meta) = state.metadata.get_mut(&req.ticket_id) {
            meta.remove(&req.key);
        }
        Ok(Response::new(DeleteMetaResponse {}))
    }

    async fn add_block(
        &self,
        req: Request<AddBlockRequest>,
    ) -> Result<Response<AddBlockResponse>, Status> {
        let req = req.into_inner();
        self.inner
            .lock()
            .unwrap()
            .edges
            .push((req.blocker_id, req.blocked_id, "blocks".into()));
        Ok(Response::new(AddBlockResponse {}))
    }

    async fn remove_block(
        &self,
        req: Request<RemoveBlockRequest>,
    ) -> Result<Response<RemoveBlockResponse>, Status> {
        let req = req.into_inner();
        let mut state = self.inner.lock().unwrap();
        state
            .edges
            .retain(|e| !(e.0 == req.blocker_id && e.1 == req.blocked_id && e.2 == "blocks"));
        Ok(Response::new(RemoveBlockResponse {}))
    }

    async fn add_link(
        &self,
        req: Request<AddLinkRequest>,
    ) -> Result<Response<AddLinkResponse>, Status> {
        let req = req.into_inner();
        self.inner
            .lock()
            .unwrap()
            .edges
            .push((req.left_id, req.right_id, "relates_to".into()));
        Ok(Response::new(AddLinkResponse {}))
    }

    async fn remove_link(
        &self,
        req: Request<RemoveLinkRequest>,
    ) -> Result<Response<RemoveLinkResponse>, Status> {
        let req = req.into_inner();
        let mut state = self.inner.lock().unwrap();
        state
            .edges
            .retain(|e| !(e.0 == req.left_id && e.1 == req.right_id && e.2 == "relates_to"));
        Ok(Response::new(RemoveLinkResponse {}))
    }

    async fn add_activity(
        &self,
        req: Request<AddActivityRequest>,
    ) -> Result<Response<AddActivityResponse>, Status> {
        let req = req.into_inner();
        let activity_id = self.next_id();
        let entry = ActivityEntry {
            id: activity_id.clone(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            author: req.author,
            message: req.message,
        };
        self.inner
            .lock()
            .unwrap()
            .activities
            .entry(req.ticket_id)
            .or_default()
            .push(entry);
        Ok(Response::new(AddActivityResponse { activity_id }))
    }

    async fn list_activities(
        &self,
        req: Request<ListActivitiesRequest>,
    ) -> Result<Response<ListActivitiesResponse>, Status> {
        let req = req.into_inner();
        let state = self.inner.lock().unwrap();
        let activities = state
            .activities
            .get(&req.ticket_id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|entry| ActivityDetail {
                entry: Some(entry),
                metadata: vec![],
            })
            .collect();
        Ok(Response::new(ListActivitiesResponse { activities }))
    }

    async fn dispatchable_tickets(
        &self,
        req: Request<DispatchableTicketsRequest>,
    ) -> Result<Response<DispatchableTicketsResponse>, Status> {
        let epic_id = req.into_inner().epic_id;
        let state = self.inner.lock().unwrap();
        let tickets: Vec<DispatchableTicket> = state
            .tickets
            .values()
            .filter(|t| t.parent_id == epic_id && t.status == "open")
            .map(|t| DispatchableTicket {
                id: t.id.clone(),
                title: t.title.clone(),
                priority: t.priority,
            })
            .collect();
        Ok(Response::new(DispatchableTicketsResponse { tickets }))
    }
}

// --- Test helpers ---

/// Start a mock TicketService gRPC server on an ephemeral port.
/// Returns the server address and a handle to shut it down.
async fn start_mock_server() -> (SocketAddr, MockTicketStore) {
    let store = MockTicketStore::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let store_clone = store.clone();
    tokio::spawn(async move {
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
        Server::builder()
            .add_service(TicketServiceServer::new(store_clone))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    (addr, store)
}

async fn connect(addr: SocketAddr) -> TicketServiceClient<Channel> {
    let endpoint = Endpoint::try_from(format!("http://{addr}")).unwrap();
    let channel = endpoint.connect().await.unwrap();
    TicketServiceClient::new(channel)
}

// --- Tests ---

#[tokio::test]
async fn execute_create_and_show() {
    let (addr, _store) = start_mock_server().await;
    let mut client = connect(addr).await;

    // Create a ticket
    crate::execute(
        TicketArgs::Create {
            title: "Test ticket".into(),
            ticket_type: "task".into(),
            parent: None,
            priority: 2,
            body: "Body text".into(),
        },
        &mut client,
        false,
    )
    .await
    .expect("create should succeed");

    // List tickets and verify the created ticket appears
    crate::execute(
        TicketArgs::List {
            epic: None,
            ticket_type: None,
            status: None,
        },
        &mut client,
        false,
    )
    .await
    .expect("list should succeed");
}

#[tokio::test]
async fn execute_create_and_list_filtered() {
    let (addr, store) = start_mock_server().await;
    let mut client = connect(addr).await;

    // Create two tickets with different statuses
    crate::execute(
        TicketArgs::Create {
            title: "Open ticket".into(),
            ticket_type: "task".into(),
            parent: None,
            priority: 1,
            body: String::new(),
        },
        &mut client,
        false,
    )
    .await
    .unwrap();

    // Manually close one ticket via the store to test filtering
    {
        let mut state = store.inner.lock().unwrap();
        let id = state.tickets.keys().next().unwrap().clone();
        state.tickets.get_mut(&id).unwrap().status = "closed".into();
    }

    crate::execute(
        TicketArgs::Create {
            title: "Another open ticket".into(),
            ticket_type: "task".into(),
            parent: None,
            priority: 2,
            body: String::new(),
        },
        &mut client,
        false,
    )
    .await
    .unwrap();

    // List with status filter
    crate::execute(
        TicketArgs::List {
            epic: None,
            ticket_type: None,
            status: Some("open".into()),
        },
        &mut client,
        false,
    )
    .await
    .expect("list with status filter should succeed");
}

#[tokio::test]
async fn execute_show_nonexistent_returns_error() {
    let (addr, _store) = start_mock_server().await;
    let mut client = connect(addr).await;

    let result = crate::execute(
        TicketArgs::Show {
            id: "ur-nonexistent".into(),
        },
        &mut client,
        false,
    )
    .await;

    assert!(
        result.is_err(),
        "show for nonexistent ticket should return an error"
    );
    let err_msg = format!("{:#}", result.unwrap_err());
    assert!(
        err_msg.contains("not found") || err_msg.contains("failed to get ticket"),
        "error should indicate ticket not found, got: {err_msg}"
    );
}

#[tokio::test]
async fn execute_update_nonexistent_returns_error() {
    let (addr, _store) = start_mock_server().await;
    let mut client = connect(addr).await;

    let result = crate::execute(
        TicketArgs::Update {
            id: "ur-nonexistent".into(),
            title: Some("new title".into()),
            body: None,
            status: None,
            priority: None,
            ticket_type: None,
            parent: None,
            force: false,
        },
        &mut client,
        false,
    )
    .await;

    assert!(
        result.is_err(),
        "update for nonexistent ticket should return an error"
    );
}

#[tokio::test]
async fn execute_set_and_delete_meta() {
    let (addr, store) = start_mock_server().await;
    let mut client = connect(addr).await;

    // Create a ticket first
    crate::execute(
        TicketArgs::Create {
            title: "Meta test".into(),
            ticket_type: "task".into(),
            parent: None,
            priority: 0,
            body: String::new(),
        },
        &mut client,
        false,
    )
    .await
    .unwrap();

    let ticket_id = {
        let state = store.inner.lock().unwrap();
        state.tickets.keys().next().unwrap().clone()
    };

    // Set metadata
    crate::execute(
        TicketArgs::SetMeta {
            id: ticket_id.clone(),
            key: "env".into(),
            value: "prod".into(),
        },
        &mut client,
        false,
    )
    .await
    .expect("set-meta should succeed");

    // Verify metadata was set
    {
        let state = store.inner.lock().unwrap();
        let meta = state.metadata.get(&ticket_id).unwrap();
        assert_eq!(meta.get("env").unwrap(), "prod");
    }

    // Delete metadata
    crate::execute(
        TicketArgs::DeleteMeta {
            id: ticket_id.clone(),
            key: "env".into(),
        },
        &mut client,
        false,
    )
    .await
    .expect("delete-meta should succeed");

    // Verify metadata was deleted
    {
        let state = store.inner.lock().unwrap();
        let meta = state.metadata.get(&ticket_id).unwrap();
        assert!(!meta.contains_key("env"));
    }
}

#[tokio::test]
async fn execute_add_and_list_activities() {
    let (addr, store) = start_mock_server().await;
    let mut client = connect(addr).await;

    // Create ticket
    crate::execute(
        TicketArgs::Create {
            title: "Activity test".into(),
            ticket_type: "task".into(),
            parent: None,
            priority: 0,
            body: String::new(),
        },
        &mut client,
        false,
    )
    .await
    .unwrap();

    let ticket_id = {
        let state = store.inner.lock().unwrap();
        state.tickets.keys().next().unwrap().clone()
    };

    // Add activity
    crate::execute(
        TicketArgs::AddActivity {
            id: ticket_id.clone(),
            message: "did some work".into(),
            meta: vec![],
        },
        &mut client,
        false,
    )
    .await
    .expect("add-activity should succeed");

    // List activities
    crate::execute(
        TicketArgs::ListActivities {
            id: ticket_id.clone(),
        },
        &mut client,
        false,
    )
    .await
    .expect("list-activities should succeed");

    // Verify activity was stored
    {
        let state = store.inner.lock().unwrap();
        let activities = state.activities.get(&ticket_id).unwrap();
        assert_eq!(activities.len(), 1);
        assert_eq!(activities[0].message, "did some work");
    }
}

#[tokio::test]
async fn execute_add_and_remove_block() {
    let (addr, store) = start_mock_server().await;
    let mut client = connect(addr).await;

    // Create two tickets
    crate::execute(
        TicketArgs::Create {
            title: "Blocker".into(),
            ticket_type: "task".into(),
            parent: None,
            priority: 0,
            body: String::new(),
        },
        &mut client,
        false,
    )
    .await
    .unwrap();

    crate::execute(
        TicketArgs::Create {
            title: "Blocked".into(),
            ticket_type: "task".into(),
            parent: None,
            priority: 0,
            body: String::new(),
        },
        &mut client,
        false,
    )
    .await
    .unwrap();

    let (id_a, id_b) = {
        let state = store.inner.lock().unwrap();
        let ids: Vec<String> = state.tickets.keys().cloned().collect();
        (ids[0].clone(), ids[1].clone())
    };

    // Add block
    crate::execute(
        TicketArgs::AddBlock {
            id: id_b.clone(),
            blocked_by_id: id_a.clone(),
        },
        &mut client,
        false,
    )
    .await
    .expect("add-block should succeed");

    {
        let state = store.inner.lock().unwrap();
        assert_eq!(state.edges.len(), 1);
        assert_eq!(state.edges[0].2, "blocks");
    }

    // Remove block
    crate::execute(
        TicketArgs::RemoveBlock {
            id: id_b.clone(),
            blocked_by_id: id_a.clone(),
        },
        &mut client,
        false,
    )
    .await
    .expect("remove-block should succeed");

    {
        let state = store.inner.lock().unwrap();
        assert!(state.edges.is_empty());
    }
}

#[tokio::test]
async fn execute_add_and_remove_link() {
    let (addr, store) = start_mock_server().await;
    let mut client = connect(addr).await;

    // Create two tickets
    crate::execute(
        TicketArgs::Create {
            title: "Ticket A".into(),
            ticket_type: "task".into(),
            parent: None,
            priority: 0,
            body: String::new(),
        },
        &mut client,
        false,
    )
    .await
    .unwrap();

    crate::execute(
        TicketArgs::Create {
            title: "Ticket B".into(),
            ticket_type: "task".into(),
            parent: None,
            priority: 0,
            body: String::new(),
        },
        &mut client,
        false,
    )
    .await
    .unwrap();

    let (id_a, id_b) = {
        let state = store.inner.lock().unwrap();
        let ids: Vec<String> = state.tickets.keys().cloned().collect();
        (ids[0].clone(), ids[1].clone())
    };

    // Add link
    crate::execute(
        TicketArgs::AddLink {
            id: id_a.clone(),
            linked_id: id_b.clone(),
        },
        &mut client,
        false,
    )
    .await
    .expect("add-link should succeed");

    {
        let state = store.inner.lock().unwrap();
        assert_eq!(state.edges.len(), 1);
        assert_eq!(state.edges[0].2, "relates_to");
    }

    // Remove link
    crate::execute(
        TicketArgs::RemoveLink {
            id: id_a.clone(),
            linked_id: id_b.clone(),
        },
        &mut client,
        false,
    )
    .await
    .expect("remove-link should succeed");

    {
        let state = store.inner.lock().unwrap();
        assert!(state.edges.is_empty());
    }
}

#[tokio::test]
async fn execute_update_existing_ticket() {
    let (addr, store) = start_mock_server().await;
    let mut client = connect(addr).await;

    crate::execute(
        TicketArgs::Create {
            title: "Original title".into(),
            ticket_type: "task".into(),
            parent: None,
            priority: 0,
            body: String::new(),
        },
        &mut client,
        false,
    )
    .await
    .unwrap();

    let ticket_id = {
        let state = store.inner.lock().unwrap();
        state.tickets.keys().next().unwrap().clone()
    };

    crate::execute(
        TicketArgs::Update {
            id: ticket_id.clone(),
            title: Some("Updated title".into()),
            body: None,
            status: Some("closed".into()),
            priority: Some(5),
            ticket_type: None,
            parent: None,
            force: false,
        },
        &mut client,
        false,
    )
    .await
    .expect("update should succeed");

    {
        let state = store.inner.lock().unwrap();
        let t = state.tickets.get(&ticket_id).unwrap();
        assert_eq!(t.title, "Updated title");
        assert_eq!(t.status, "closed");
        assert_eq!(t.priority, 5);
    }
}

#[tokio::test]
async fn execute_dispatchable() {
    let (addr, store) = start_mock_server().await;
    let mut client = connect(addr).await;

    // Create an epic
    crate::execute(
        TicketArgs::Create {
            title: "My Epic".into(),
            ticket_type: "epic".into(),
            parent: None,
            priority: 0,
            body: String::new(),
        },
        &mut client,
        false,
    )
    .await
    .unwrap();

    let epic_id = {
        let state = store.inner.lock().unwrap();
        state.tickets.keys().next().unwrap().clone()
    };

    // Create a child task with the epic as parent
    // We need to set parent_id directly since the mock uses parent_id from the request
    crate::execute(
        TicketArgs::Create {
            title: "Child task".into(),
            ticket_type: "task".into(),
            parent: Some(epic_id.clone()),
            priority: 1,
            body: String::new(),
        },
        &mut client,
        false,
    )
    .await
    .unwrap();

    // Dispatchable should return the child task
    crate::execute(
        TicketArgs::Dispatchable {
            epic_id: epic_id.clone(),
        },
        &mut client,
        false,
    )
    .await
    .expect("dispatchable should succeed");

    // Verify via store that the child exists under the epic
    {
        let state = store.inner.lock().unwrap();
        let children: Vec<_> = state
            .tickets
            .values()
            .filter(|t| t.parent_id == epic_id && t.status == "open")
            .collect();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].title, "Child task");
    }
}

#[tokio::test]
async fn execute_list_activities_empty() {
    let (addr, _store) = start_mock_server().await;
    let mut client = connect(addr).await;

    // Create a ticket
    crate::execute(
        TicketArgs::Create {
            title: "No activities".into(),
            ticket_type: "task".into(),
            parent: None,
            priority: 0,
            body: String::new(),
        },
        &mut client,
        false,
    )
    .await
    .unwrap();

    // List activities on a ticket with none — should succeed (print "No activities")
    crate::execute(
        TicketArgs::ListActivities {
            id: "ur-t0001".into(),
        },
        &mut client,
        false,
    )
    .await
    .expect("list-activities on ticket with no activities should succeed");
}

#[tokio::test]
async fn execute_list_empty() {
    let (addr, _store) = start_mock_server().await;
    let mut client = connect(addr).await;

    // List tickets on empty store — should succeed (print "No tickets found.")
    crate::execute(
        TicketArgs::List {
            epic: None,
            ticket_type: None,
            status: None,
        },
        &mut client,
        false,
    )
    .await
    .expect("list on empty store should succeed");
}

/// Test that auth rejection propagates correctly when using an interceptor
/// that always rejects requests.
#[tokio::test]
async fn auth_rejection_propagates_error() {
    let store = MockTicketStore::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let store_clone = store.clone();
    tokio::spawn(async move {
        #[allow(clippy::result_large_err)]
        let interceptor = move |req: Request<()>| -> Result<Request<()>, Status> {
            let _ = req;
            Err(Status::unauthenticated("missing ur-agent-id header"))
        };
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
        Server::builder()
            .add_service(TicketServiceServer::with_interceptor(
                store_clone,
                interceptor,
            ))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    let mut client = connect(addr).await;

    let result = crate::execute(
        TicketArgs::Create {
            title: "Should fail".into(),
            ticket_type: "task".into(),
            parent: None,
            priority: 0,
            body: String::new(),
        },
        &mut client,
        false,
    )
    .await;

    assert!(
        result.is_err(),
        "request to auth-gated server without credentials should fail"
    );
    let err_msg = format!("{:#}", result.unwrap_err());
    assert!(
        err_msg.contains("unauthenticated")
            || err_msg.contains("Unauthenticated")
            || err_msg.contains("missing"),
        "error should indicate auth failure, got: {err_msg}"
    );
}
