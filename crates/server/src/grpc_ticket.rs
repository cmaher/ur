use std::collections::HashMap;
use std::pin::Pin;

use tonic::{Code, Request, Response, Status};
use tracing::info;

use ur_db::{
    EdgeKind, LifecycleStatus, NewTicket, TicketFilter, TicketRepo, TicketUpdate, WorkflowRepo,
};
use ur_rpc::error::{
    self, DOMAIN_TICKET, INTERNAL, INVALID_ARGUMENT, NOT_FOUND, TICKET_HAS_ACTIVE_WORKFLOW,
    TICKET_HAS_OPEN_CHILDREN,
};
use ur_rpc::proto::ticket::ticket_service_server::TicketService;
use ur_rpc::proto::ticket::{
    AddActivityRequest, AddActivityResponse, AddBlockRequest, AddBlockResponse, AddLinkRequest,
    AddLinkResponse, CancelWorkflowRequest, CancelWorkflowResponse, CreateTicketRequest,
    CreateTicketResponse, CreateWorkflowRequest, CreateWorkflowResponse, DeleteMetaRequest,
    DeleteMetaResponse, DispatchableTicketsRequest, DispatchableTicketsResponse, GetTicketRequest,
    GetTicketResponse, GetWorkflowRequest, GetWorkflowResponse, ListActivitiesRequest,
    ListActivitiesResponse, ListTicketsRequest, ListTicketsResponse, ListWorkflowsRequest,
    ListWorkflowsResponse, RedriveTicketRequest, RedriveTicketResponse, RemoveBlockRequest,
    RemoveBlockResponse, RemoveLinkRequest, RemoveLinkResponse, SetMetaRequest, SetMetaResponse,
    SubscribeUiEventsRequest, UiEventBatch, UpdateTicketRequest, UpdateTicketResponse,
    WorkflowHistoryEvent, WorkflowInfo,
};

use crate::UiEventPoller;
use crate::WorkerManager;
use crate::worker::WorkerId;

#[derive(Debug, thiserror::Error)]
pub enum TicketError {
    #[error("ticket not found: {id}")]
    NotFound { id: String },

    #[error("ticket {id} has open children; close them first or use --force")]
    HasOpenChildren { id: String, children: Vec<String> },

    #[error("ticket {id} already has an active workflow")]
    ActiveWorkflow { id: String },

    #[error("validation error: {0}")]
    Validation(String),

    #[error("database error: {0}")]
    Db(String),
}

impl From<TicketError> for Status {
    fn from(err: TicketError) -> Self {
        match err {
            TicketError::NotFound { ref id } => {
                let mut meta = HashMap::new();
                meta.insert("ticket_id".into(), id.clone());
                error::status_with_info(
                    Code::NotFound,
                    err.to_string(),
                    DOMAIN_TICKET,
                    NOT_FOUND,
                    meta,
                )
            }
            TicketError::HasOpenChildren {
                ref id,
                ref children,
            } => {
                let mut meta = HashMap::new();
                meta.insert("ticket_id".into(), id.clone());
                meta.insert("children".into(), children.join(","));
                error::status_with_info(
                    Code::FailedPrecondition,
                    err.to_string(),
                    DOMAIN_TICKET,
                    TICKET_HAS_OPEN_CHILDREN,
                    meta,
                )
            }
            TicketError::ActiveWorkflow { ref id } => {
                let mut meta = HashMap::new();
                meta.insert("ticket_id".into(), id.clone());
                error::status_with_info(
                    Code::FailedPrecondition,
                    err.to_string(),
                    DOMAIN_TICKET,
                    TICKET_HAS_ACTIVE_WORKFLOW,
                    meta,
                )
            }
            TicketError::Validation(_) => error::status_with_info(
                Code::InvalidArgument,
                err.to_string(),
                DOMAIN_TICKET,
                INVALID_ARGUMENT,
                HashMap::new(),
            ),
            TicketError::Db(_) => error::status_with_info(
                Code::Internal,
                err.to_string(),
                DOMAIN_TICKET,
                INTERNAL,
                HashMap::new(),
            ),
        }
    }
}

/// gRPC implementation of the TicketService, delegating to `TicketRepo`.
#[derive(Clone)]
pub struct TicketServiceHandler {
    pub ticket_repo: TicketRepo,
    pub workflow_repo: WorkflowRepo,
    pub project_registry: crate::ProjectRegistry,
    /// Optional channel sender for workflow transition requests.
    /// None on the worker server (no workflow engine).
    pub transition_tx: Option<tokio::sync::mpsc::Sender<crate::workflow::TransitionRequest>>,
    /// Optional channel sender for workflow cancellation requests.
    /// None on the worker server (no workflow engine).
    pub cancel_tx: Option<tokio::sync::mpsc::Sender<String>>,
    /// Optional UI event poller for streaming UI events to subscribers.
    /// None on the worker server.
    pub ui_event_poller: Option<UiEventPoller>,
    /// Optional worker manager for killing workers when workflows are cancelled.
    /// None on the worker server.
    pub worker_manager: Option<WorkerManager>,
}

impl TicketServiceHandler {
    /// Enrich a workflow with history events, ticket progress, and PR URL.
    async fn enrich_workflow(&self, wf: ur_db::Workflow) -> Result<WorkflowInfo, Status> {
        let pr_url = self
            .ticket_repo
            .get_meta(&wf.ticket_id, "ticket")
            .await
            .unwrap_or_default()
            .remove("pr_url")
            .unwrap_or_default();

        let events = self
            .workflow_repo
            .get_workflow_events(&wf.id)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        let history: Vec<WorkflowHistoryEvent> = events
            .into_iter()
            .map(|e| WorkflowHistoryEvent {
                event: e.event,
                created_at: e.created_at,
            })
            .collect();

        let (children_open, children_closed) = self
            .workflow_repo
            .get_ticket_children_counts(&wf.ticket_id)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        Ok(workflow_to_proto(
            wf,
            pr_url,
            history,
            children_open,
            children_closed,
        ))
    }

    /// Enrich a `PaginatedWorkflow` (which already includes children counts)
    /// with history events and PR URL.
    async fn enrich_paginated_workflow(
        &self,
        pw: ur_db::workflow_repo::PaginatedWorkflow,
    ) -> Result<WorkflowInfo, Status> {
        let pr_url = self
            .ticket_repo
            .get_meta(&pw.workflow.ticket_id, "ticket")
            .await
            .unwrap_or_default()
            .remove("pr_url")
            .unwrap_or_default();

        let events = self
            .workflow_repo
            .get_workflow_events(&pw.workflow.id)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        let history: Vec<WorkflowHistoryEvent> = events
            .into_iter()
            .map(|e| WorkflowHistoryEvent {
                event: e.event,
                created_at: e.created_at,
            })
            .collect();

        Ok(workflow_to_proto(
            pw.workflow,
            pr_url,
            history,
            pw.ticket_children_open,
            pw.ticket_children_closed,
        ))
    }

    /// If the ticket has an active (non-terminal) workflow, cancel it: signal
    /// the coordinator to abort the in-flight handler, kill the associated
    /// worker, delete intent rows, and set the workflow status to `Cancelled`.
    async fn cancel_active_workflow(&self, ticket_id: &str) -> Result<(), Status> {
        let workflow = self
            .workflow_repo
            .get_workflow_by_ticket(ticket_id)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        let workflow = match workflow {
            Some(wf) => wf,
            None => return Ok(()),
        };

        info!(ticket_id = %ticket_id, "cancelling active workflow for ticket close");

        // Signal the coordinator to abort any in-flight handler task.
        if let Some(cancel_tx) = &self.cancel_tx
            && let Err(e) = cancel_tx.send(ticket_id.to_owned()).await
        {
            tracing::warn!(
                ticket_id = %ticket_id,
                error = %e,
                "failed to send cancel signal to coordinator (channel closed)"
            );
        }

        // Kill the worker associated with this workflow.
        self.kill_workflow_worker(ticket_id, &workflow.worker_id)
            .await;

        // Delete intents and mark workflow as cancelled.
        self.workflow_repo
            .delete_intents_for_ticket(ticket_id)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        self.workflow_repo
            .update_workflow_status(ticket_id, LifecycleStatus::Cancelled)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        Ok(())
    }

    /// Kill the worker associated with a workflow, if one is assigned.
    async fn kill_workflow_worker(&self, ticket_id: &str, worker_id: &str) {
        if worker_id.is_empty() {
            return;
        }
        let Some(ref worker_manager) = self.worker_manager else {
            return;
        };
        info!(ticket_id = %ticket_id, worker_id = %worker_id, "killing worker for cancelled workflow");
        if let Err(e) = worker_manager
            .stop_by_worker_id(&WorkerId(worker_id.to_owned()))
            .await
        {
            tracing::warn!(
                ticket_id = %ticket_id,
                worker_id = %worker_id,
                error = %e,
                "failed to stop worker during workflow cancellation"
            );
        }
    }

    /// Convert metadata query results to minimal proto tickets.
    fn meta_tickets_to_proto(
        matches: Vec<ur_db::MetadataMatchTicket>,
    ) -> Vec<ur_rpc::proto::ticket::Ticket> {
        matches
            .into_iter()
            .map(|t| ur_rpc::proto::ticket::Ticket {
                id: t.id,
                ticket_type: t.type_,
                status: t.status,
                priority: 0,
                parent_id: String::new(),
                title: t.title,
                body: String::new(),
                created_at: String::new(),
                updated_at: String::new(),
                project: String::new(),
                branch: String::new(),
                depth: 0,
                children_completed: 0,
                children_total: 0,
                dispatch_status: String::new(),
            })
            .collect()
    }

    /// Enrich a list of proto tickets with dispatch_status from active workflows.
    /// For each ticket, checks if the ticket itself or its parent has an active workflow.
    async fn enrich_dispatch_status(
        &self,
        tickets: &mut [ur_rpc::proto::ticket::Ticket],
    ) -> Result<(), Status> {
        if tickets.is_empty() {
            return Ok(());
        }

        // Collect all ticket IDs and parent IDs for the batch query.
        let mut ids_to_query: Vec<String> = tickets.iter().map(|t| t.id.clone()).collect();
        for t in tickets.iter() {
            if !t.parent_id.is_empty() && !ids_to_query.contains(&t.parent_id) {
                ids_to_query.push(t.parent_id.clone());
            }
        }

        let active_workflows = self
            .workflow_repo
            .get_active_workflows_by_ticket_ids(&ids_to_query)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        for ticket in tickets.iter_mut() {
            // Check if this ticket has an active workflow.
            if let Some(status) = active_workflows.get(&ticket.id) {
                ticket.dispatch_status = status.clone();
            } else if !ticket.parent_id.is_empty()
                && let Some(status) = active_workflows.get(&ticket.parent_id)
            {
                ticket.dispatch_status = status.clone();
            }
        }

        Ok(())
    }

    /// List a ticket tree: root + all descendants with depth, using a recursive CTE.
    async fn list_ticket_tree(
        &self,
        root_id: &str,
        status_filter: Option<&str>,
    ) -> Result<Vec<ur_rpc::proto::ticket::Ticket>, Status> {
        let rows = self
            .ticket_repo
            .list_ticket_tree(root_id, status_filter)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|(t, depth)| ur_rpc::proto::ticket::Ticket {
                id: t.id,
                ticket_type: t.type_,
                status: t.status,
                priority: t.priority as i64,
                parent_id: t.parent_id.unwrap_or_default(),
                title: t.title,
                body: t.body,
                created_at: t.created_at,
                updated_at: t.updated_at,
                project: t.project,
                branch: t.branch.unwrap_or_default(),
                depth,
                children_completed: t.children_completed,
                children_total: t.children_total,
                dispatch_status: String::new(),
            })
            .collect())
    }
}

type SubscribeUiEventsOutputStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<UiEventBatch, Status>> + Send>>;

#[tonic::async_trait]
impl TicketService for TicketServiceHandler {
    type SubscribeUiEventsStream = SubscribeUiEventsOutputStream;

    async fn create_ticket(
        &self,
        req: Request<CreateTicketRequest>,
    ) -> Result<Response<CreateTicketResponse>, Status> {
        let req = req.into_inner();
        info!(project = %req.project, title = %req.title, "create_ticket request");

        let valid_projects = self.project_registry.valid_project_keys();
        if !valid_projects.is_empty() && !valid_projects.contains(&req.project) {
            return Err(TicketError::Validation(format!(
                "unknown project '{}'; configured projects: {}",
                req.project,
                valid_projects
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
            ))
            .into());
        }

        let id = req.id.filter(|s| !s.is_empty());

        let parent_id = req.parent_id.filter(|s| !s.is_empty());

        let status = if req.status.is_empty() {
            None
        } else {
            Some(req.status)
        };
        let created_at = req.created_at.filter(|s| !s.is_empty());

        let lifecycle_status = if req.wip {
            Some(LifecycleStatus::Design)
        } else {
            None
        };

        let branch = req.branch.filter(|s| !s.is_empty());

        let new_ticket = NewTicket {
            id,
            project: req.project,
            type_: req.ticket_type,
            priority: req.priority as i32,
            parent_id,
            title: req.title,
            body: req.body,
            status,
            lifecycle_status,
            branch,
            created_at,
        };

        let ticket = self
            .ticket_repo
            .create_ticket(&new_ticket)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        Ok(Response::new(CreateTicketResponse { id: ticket.id }))
    }

    async fn list_tickets(
        &self,
        req: Request<ListTicketsRequest>,
    ) -> Result<Response<ListTicketsResponse>, Status> {
        let req = req.into_inner();
        info!("list_tickets request");

        let meta_key = req.meta_key.filter(|s| !s.is_empty());
        let meta_value = req.meta_value.filter(|s| !s.is_empty());
        let tree_root_id = req.tree_root_id.filter(|s| !s.is_empty());

        // If metadata filters are provided, use the metadata-based queries
        let tickets = match (&meta_key, &meta_value) {
            (Some(key), Some(value)) => {
                let matches = self
                    .ticket_repo
                    .tickets_by_metadata(key, value)
                    .await
                    .map_err(|e| TicketError::Db(e.to_string()))?;
                Self::meta_tickets_to_proto(matches)
            }
            (Some(key), None) => {
                let matches = self
                    .ticket_repo
                    .tickets_with_metadata_key(key)
                    .await
                    .map_err(|e| TicketError::Db(e.to_string()))?;
                Self::meta_tickets_to_proto(matches)
            }
            _ if tree_root_id.is_some() => {
                let root_id = tree_root_id.unwrap();
                let status_filter = req.status.filter(|s| !s.is_empty());
                self.list_ticket_tree(&root_id, status_filter.as_deref())
                    .await?
            }
            _ => {
                let statuses: Vec<String> = req
                    .status
                    .filter(|s| !s.is_empty())
                    .map(|s| s.split(',').map(|v| v.trim().to_owned()).collect())
                    .unwrap_or_default();
                let filter = TicketFilter {
                    project: req.project.filter(|s| !s.is_empty()),
                    statuses,
                    type_: req.ticket_type.filter(|s| !s.is_empty()),
                    parent_id: req.parent_id.filter(|s| !s.is_empty()),
                    lifecycle_status: None,
                };

                let page_size = req.page_size;
                let offset = req.offset.unwrap_or(0);
                let include_children = req.include_children.unwrap_or(false);

                if offset < 0 {
                    return Err(
                        TicketError::Validation("offset must be non-negative".into()).into(),
                    );
                }
                if let Some(ps) = page_size
                    && ps <= 0
                {
                    return Err(TicketError::Validation("page_size must be positive".into()).into());
                }

                let (db_tickets, total_count) = self
                    .ticket_repo
                    .list_tickets_paginated(&filter, page_size, offset, include_children)
                    .await
                    .map_err(|e| TicketError::Db(e.to_string()))?;

                let tickets: Vec<_> = db_tickets
                    .into_iter()
                    .map(|t| ur_rpc::proto::ticket::Ticket {
                        id: t.id,
                        ticket_type: t.type_,
                        status: t.status,
                        priority: t.priority as i64,
                        parent_id: t.parent_id.unwrap_or_default(),
                        title: t.title,
                        body: t.body,
                        created_at: t.created_at,
                        updated_at: t.updated_at,
                        project: t.project,
                        branch: t.branch.unwrap_or_default(),
                        depth: 0,
                        children_completed: t.children_completed,
                        children_total: t.children_total,
                        dispatch_status: String::new(),
                    })
                    .collect();

                return {
                    let mut tickets = tickets;
                    self.enrich_dispatch_status(&mut tickets).await?;
                    Ok(Response::new(ListTicketsResponse {
                        tickets,
                        total_count,
                    }))
                };
            }
        };

        let mut tickets = tickets;
        self.enrich_dispatch_status(&mut tickets).await?;

        let total_count = tickets.len() as i32;
        Ok(Response::new(ListTicketsResponse {
            tickets,
            total_count,
        }))
    }

    async fn get_ticket(
        &self,
        req: Request<GetTicketRequest>,
    ) -> Result<Response<GetTicketResponse>, Status> {
        let req = req.into_inner();
        info!(id = %req.id, "get_ticket request");

        let t = self
            .ticket_repo
            .get_ticket_by_id(&req.id)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?
            .ok_or_else(|| TicketError::NotFound { id: req.id.clone() })?;

        let meta = self
            .ticket_repo
            .get_meta(&req.id, "ticket")
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        let activities_list = if let Some(author) = &req.activity_author_filter {
            self.ticket_repo
                .get_activities_by_author(&req.id, author)
                .await
                .map_err(|e| TicketError::Db(e.to_string()))?
        } else {
            self.ticket_repo
                .get_activities(&req.id)
                .await
                .map_err(|e| TicketError::Db(e.to_string()))?
        };

        let ticket = ur_rpc::proto::ticket::Ticket {
            id: t.id,
            ticket_type: t.type_,
            status: t.status,
            priority: t.priority as i64,
            parent_id: t.parent_id.unwrap_or_default(),
            title: t.title,
            body: t.body,
            created_at: t.created_at,
            updated_at: t.updated_at,
            project: t.project,
            branch: t.branch.unwrap_or_default(),
            depth: 0,
            children_completed: t.children_completed,
            children_total: t.children_total,
            dispatch_status: String::new(),
        };

        let metadata: Vec<_> = meta
            .into_iter()
            .map(|(key, value)| ur_rpc::proto::ticket::MetadataEntry { key, value })
            .collect();

        let activities = activities_list
            .into_iter()
            .map(|a| ur_rpc::proto::ticket::ActivityEntry {
                id: a.id,
                timestamp: a.timestamp,
                author: a.author,
                message: a.message,
            })
            .collect();

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
        info!(id = %req.id, "update_ticket request");

        let parent_id = match req.parent_id {
            None => None,
            Some(ref s) if s == "NONE" => Some(None),
            Some(s) => Some(Some(s)),
        };

        let branch = match req.branch {
            None => None,
            Some(ref s) if s == "NONE" => Some(None),
            Some(s) if s.is_empty() => None,
            Some(s) => Some(Some(s)),
        };

        let project = req.project.filter(|s| !s.is_empty());
        let valid_projects = self.project_registry.valid_project_keys();
        if let Some(ref p) = project
            && !valid_projects.is_empty()
            && !valid_projects.contains(p)
        {
            return Err(TicketError::Validation(format!(
                "unknown project '{}'; configured projects: {}",
                p,
                valid_projects
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
            ))
            .into());
        }

        let update = TicketUpdate {
            status: req.status.filter(|s| !s.is_empty()),
            lifecycle_status: None,
            lifecycle_managed: None,
            type_: req.ticket_type.filter(|s| !s.is_empty()),
            priority: req.priority.map(|p| p as i32),
            title: req.title.filter(|s| !s.is_empty()),
            body: req.body.filter(|s| !s.is_empty()),
            branch,
            parent_id,
            project,
        };

        if update.status.as_deref() == Some("closed") {
            if req.force {
                self.ticket_repo
                    .close_open_children(&req.id)
                    .await
                    .map_err(|e| TicketError::Db(e.to_string()))?;
            } else {
                let all_closed = self
                    .ticket_repo
                    .epic_all_children_closed(&req.id)
                    .await
                    .map_err(|e| TicketError::Db(e.to_string()))?;
                if !all_closed {
                    return Err(TicketError::HasOpenChildren {
                        id: req.id.clone(),
                        children: Vec::new(),
                    }
                    .into());
                }
            }
        }

        let updated = self
            .ticket_repo
            .update_ticket(&req.id, &update)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        let new_id = if updated.id != req.id {
            updated.id
        } else {
            String::new()
        };

        Ok(Response::new(UpdateTicketResponse { new_id }))
    }

    async fn set_meta(
        &self,
        req: Request<SetMetaRequest>,
    ) -> Result<Response<SetMetaResponse>, Status> {
        let req = req.into_inner();
        info!(ticket_id = %req.ticket_id, key = %req.key, "set_meta request");

        // Route workflow-owned keys to the workflow table instead of ticket metadata.
        match req.key.as_str() {
            ur_rpc::ticket_meta::NOVERIFY => {
                let noverify = req.value == "true" || req.value == "1";
                self.workflow_repo
                    .set_workflow_noverify(&req.ticket_id, noverify)
                    .await
                    .map_err(|e| TicketError::Db(e.to_string()))?;
            }
            ur_rpc::ticket_meta::FEEDBACK_MODE => {
                self.workflow_repo
                    .set_workflow_feedback_mode(&req.ticket_id, &req.value)
                    .await
                    .map_err(|e| TicketError::Db(e.to_string()))?;
            }
            _ => {
                self.ticket_repo
                    .set_meta(&req.ticket_id, "ticket", &req.key, &req.value)
                    .await
                    .map_err(|e| TicketError::Db(e.to_string()))?;
            }
        }

        Ok(Response::new(SetMetaResponse {}))
    }

    async fn delete_meta(
        &self,
        req: Request<DeleteMetaRequest>,
    ) -> Result<Response<DeleteMetaResponse>, Status> {
        let req = req.into_inner();
        info!(ticket_id = %req.ticket_id, key = %req.key, "delete_meta request");

        self.ticket_repo
            .delete_meta(&req.ticket_id, "ticket", &req.key)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        Ok(Response::new(DeleteMetaResponse {}))
    }

    async fn add_block(
        &self,
        req: Request<AddBlockRequest>,
    ) -> Result<Response<AddBlockResponse>, Status> {
        let req = req.into_inner();
        info!(blocker = %req.blocker_id, blocked = %req.blocked_id, "add_block request");

        self.ticket_repo
            .add_edge(&req.blocker_id, &req.blocked_id, EdgeKind::Blocks)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        Ok(Response::new(AddBlockResponse {}))
    }

    async fn remove_block(
        &self,
        req: Request<RemoveBlockRequest>,
    ) -> Result<Response<RemoveBlockResponse>, Status> {
        let req = req.into_inner();
        info!(blocker = %req.blocker_id, blocked = %req.blocked_id, "remove_block request");

        self.ticket_repo
            .remove_edge(&req.blocker_id, &req.blocked_id, EdgeKind::Blocks)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        Ok(Response::new(RemoveBlockResponse {}))
    }

    async fn add_link(
        &self,
        req: Request<AddLinkRequest>,
    ) -> Result<Response<AddLinkResponse>, Status> {
        let req = req.into_inner();
        let edge_kind_str = req.edge_kind.as_deref().unwrap_or("relates_to");
        let edge_kind = match edge_kind_str {
            "relates_to" => EdgeKind::RelatesTo,
            other => {
                return Err(TicketError::Validation(format!("unknown edge kind: {other}")).into());
            }
        };
        info!(left = %req.left_id, right = %req.right_id, edge_kind = edge_kind_str, "add_link request");

        self.ticket_repo
            .add_edge(&req.left_id, &req.right_id, edge_kind)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        Ok(Response::new(AddLinkResponse {}))
    }

    async fn remove_link(
        &self,
        req: Request<RemoveLinkRequest>,
    ) -> Result<Response<RemoveLinkResponse>, Status> {
        let req = req.into_inner();
        info!(left = %req.left_id, right = %req.right_id, "remove_link request");

        self.ticket_repo
            .remove_edge(&req.left_id, &req.right_id, EdgeKind::RelatesTo)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        Ok(Response::new(RemoveLinkResponse {}))
    }

    async fn add_activity(
        &self,
        req: Request<AddActivityRequest>,
    ) -> Result<Response<AddActivityResponse>, Status> {
        let req = req.into_inner();
        info!(ticket_id = %req.ticket_id, author = %req.author, "add_activity request");

        let meta: HashMap<String, String> = req.metadata;

        let activity = self
            .ticket_repo
            .add_activity(&req.ticket_id, &req.author, &req.message)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        for (key, value) in &meta {
            self.ticket_repo
                .set_meta(&activity.id, "activity", key, value)
                .await
                .map_err(|e| TicketError::Db(e.to_string()))?;
        }

        Ok(Response::new(AddActivityResponse {
            activity_id: activity.id,
        }))
    }

    async fn list_activities(
        &self,
        req: Request<ListActivitiesRequest>,
    ) -> Result<Response<ListActivitiesResponse>, Status> {
        let req = req.into_inner();
        info!(ticket_id = %req.ticket_id, "list_activities request");

        let activities_list = self
            .ticket_repo
            .get_activities(&req.ticket_id)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        let mut activities = Vec::new();
        for a in activities_list {
            let meta = self
                .ticket_repo
                .get_meta(&a.id, "activity")
                .await
                .map_err(|e| TicketError::Db(e.to_string()))?;

            activities.push(ur_rpc::proto::ticket::ActivityDetail {
                entry: Some(ur_rpc::proto::ticket::ActivityEntry {
                    id: a.id,
                    timestamp: a.timestamp,
                    author: a.author,
                    message: a.message,
                }),
                metadata: meta
                    .into_iter()
                    .map(|(key, value)| ur_rpc::proto::ticket::ActivityMetadataEntry { key, value })
                    .collect(),
            });
        }

        Ok(Response::new(ListActivitiesResponse { activities }))
    }

    async fn dispatchable_tickets(
        &self,
        req: Request<DispatchableTicketsRequest>,
    ) -> Result<Response<DispatchableTicketsResponse>, Status> {
        let req = req.into_inner();
        info!(epic_id = %req.epic_id, "dispatchable_tickets request");

        let project = req.project.filter(|s| !s.is_empty());
        let tickets = self
            .ticket_repo
            .dispatchable_tickets(&req.epic_id, project.as_deref())
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        let proto_tickets = tickets
            .into_iter()
            .map(|t| ur_rpc::proto::ticket::DispatchableTicket {
                id: t.id,
                title: t.title,
                priority: t.priority as i64,
            })
            .collect();

        Ok(Response::new(DispatchableTicketsResponse {
            tickets: proto_tickets,
        }))
    }

    async fn create_workflow(
        &self,
        req: Request<CreateWorkflowRequest>,
    ) -> Result<Response<CreateWorkflowResponse>, Status> {
        let req = req.into_inner();
        info!(ticket_id = %req.ticket_id, status = %req.status, "create_workflow request");

        let transition_tx = self.transition_tx.as_ref().ok_or_else(|| {
            Status::unavailable(
                "workflow creation not available on this server (no workflow engine)",
            )
        })?;

        // Validate the ticket exists and is not closed.
        let ticket = self
            .ticket_repo
            .get_ticket(&req.ticket_id)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?
            .ok_or_else(|| TicketError::NotFound {
                id: req.ticket_id.clone(),
            })?;

        if ticket.status == "closed" {
            return Err(TicketError::Validation(format!(
                "ticket {} has status 'closed', cannot create workflow",
                req.ticket_id,
            ))
            .into());
        }

        let status: LifecycleStatus = req
            .status
            .parse()
            .map_err(|_| Status::invalid_argument(format!("invalid status: {}", req.status)))?;

        // Reject if there is already an active (non-terminal) workflow for this ticket.
        if self
            .workflow_repo
            .get_workflow_by_ticket(&req.ticket_id)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?
            .is_some()
        {
            return Err(TicketError::ActiveWorkflow {
                id: req.ticket_id.clone(),
            }
            .into());
        }

        // Create the workflow row.
        let workflow = self
            .workflow_repo
            .create_workflow(&req.ticket_id, status)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        // Send the transition request to the coordinator.
        transition_tx
            .send(crate::workflow::TransitionRequest {
                ticket_id: req.ticket_id.clone(),
                target_status: status,
            })
            .await
            .map_err(|e| Status::internal(format!("failed to send transition request: {e}")))?;

        Ok(Response::new(CreateWorkflowResponse {
            workflow_id: workflow.id,
        }))
    }

    async fn cancel_workflow(
        &self,
        req: Request<CancelWorkflowRequest>,
    ) -> Result<Response<CancelWorkflowResponse>, Status> {
        let req = req.into_inner();
        info!(ticket_id = %req.ticket_id, "cancel_workflow request");

        self.cancel_active_workflow(&req.ticket_id).await?;

        Ok(Response::new(CancelWorkflowResponse {}))
    }

    async fn redrive_ticket(
        &self,
        req: Request<RedriveTicketRequest>,
    ) -> Result<Response<RedriveTicketResponse>, Status> {
        let req = req.into_inner();
        info!(id = %req.id, to_status = %req.to_status, "redrive_ticket request");

        let transition_tx = self.transition_tx.as_ref().ok_or_else(|| {
            Status::unavailable("redrive not available on this server (no workflow coordinator)")
        })?;

        let to_status: LifecycleStatus = req
            .to_status
            .parse()
            .map_err(|_| Status::invalid_argument(format!("invalid status: {}", req.to_status)))?;

        // 1. Clear workflow stall (stalled flag + stall_reason on workflow table) and reset cycles.
        let _ = self.workflow_repo.clear_workflow_stall(&req.id).await;
        let _ = self.workflow_repo.reset_implement_cycles(&req.id).await;

        // 2. Delete any stale workflow events for this ticket.
        self.workflow_repo
            .delete_workflow_events_for_ticket(&req.id)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        // 5. Submit transition to the coordinator for processing.
        transition_tx
            .send(crate::workflow::TransitionRequest {
                ticket_id: req.id.clone(),
                target_status: to_status,
            })
            .await
            .map_err(|e| Status::internal(format!("failed to submit redrive transition: {e}")))?;

        Ok(Response::new(RedriveTicketResponse {
            lifecycle_status: to_status.to_string(),
        }))
    }

    async fn get_workflow(
        &self,
        req: Request<GetWorkflowRequest>,
    ) -> Result<Response<GetWorkflowResponse>, Status> {
        let ticket_id = &req.get_ref().ticket_id;
        let workflow = self
            .workflow_repo
            .get_latest_workflow_by_ticket(ticket_id)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        match workflow {
            Some(wf) => {
                let proto = self.enrich_workflow(wf).await?;
                Ok(Response::new(GetWorkflowResponse {
                    workflow: Some(proto),
                }))
            }
            None => {
                let mut meta = HashMap::new();
                meta.insert("ticket_id".into(), ticket_id.clone());
                Err(error::status_with_info(
                    Code::NotFound,
                    format!("no workflow for ticket {ticket_id}"),
                    DOMAIN_TICKET,
                    NOT_FOUND,
                    meta,
                ))
            }
        }
    }

    async fn list_workflows(
        &self,
        req: Request<ListWorkflowsRequest>,
    ) -> Result<Response<ListWorkflowsResponse>, Status> {
        let req = req.into_inner();

        let status_filter = match req.status.as_deref() {
            Some(s) => Some(
                s.parse::<LifecycleStatus>()
                    .map_err(|e| Status::new(Code::InvalidArgument, e))?,
            ),
            None => None,
        };

        let page_size = req.page_size;
        let offset = req.offset.unwrap_or(0);
        let project = req.project.as_deref().filter(|s| !s.is_empty());

        if offset < 0 {
            return Err(TicketError::Validation("offset must be non-negative".into()).into());
        }
        if let Some(ps) = page_size
            && ps <= 0
        {
            return Err(TicketError::Validation("page_size must be positive".into()).into());
        }

        let (paginated, total_count) = self
            .workflow_repo
            .list_workflows_paginated(page_size, offset, status_filter, project)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        let mut protos = Vec::with_capacity(paginated.len());
        for pw in paginated {
            let proto = self.enrich_paginated_workflow(pw).await?;
            protos.push(proto);
        }
        Ok(Response::new(ListWorkflowsResponse {
            workflows: protos,
            total_count,
        }))
    }

    async fn subscribe_ui_events(
        &self,
        _req: Request<SubscribeUiEventsRequest>,
    ) -> Result<Response<Self::SubscribeUiEventsStream>, Status> {
        let poller = self.ui_event_poller.as_ref().ok_or_else(|| {
            Status::unavailable("UI event streaming not available on this server")
        })?;

        let rx = poller.add_listener().await;
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        let mapped = tokio_stream::StreamExt::map(stream, Ok);

        Ok(Response::new(Box::pin(mapped)))
    }
}

fn workflow_to_proto(
    wf: ur_db::Workflow,
    pr_url: String,
    history: Vec<WorkflowHistoryEvent>,
    ticket_children_open: i64,
    ticket_children_closed: i64,
) -> WorkflowInfo {
    WorkflowInfo {
        id: wf.id,
        ticket_id: wf.ticket_id,
        status: wf.status.to_string(),
        stalled: wf.stalled,
        stall_reason: wf.stall_reason,
        implement_cycles: i64::from(wf.implement_cycles),
        worker_id: wf.worker_id,
        feedback_mode: wf.feedback_mode,
        created_at: wf.created_at,
        pr_url,
        history,
        ticket_children_open,
        ticket_children_closed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::Request;
    use ur_db::{GraphManager, NewTicket};
    use ur_db_test::TestDb;

    fn test_workflow() -> ur_db::Workflow {
        ur_db::Workflow {
            id: "wf-id".into(),
            ticket_id: "t-1".into(),
            status: LifecycleStatus::Implementing,
            stalled: false,
            stall_reason: String::new(),
            implement_cycles: 2,
            worker_id: "w-1".into(),
            noverify: false,
            feedback_mode: String::new(),
            ci_status: String::new(),
            mergeable: String::new(),
            review_status: String::new(),
            node_id: String::new(),
            created_at: "2025-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn workflow_to_proto_includes_history_and_progress() {
        let wf = test_workflow();
        let history = vec![
            WorkflowHistoryEvent {
                event: "implementing".into(),
                created_at: "2025-01-01T00:00:01Z".into(),
            },
            WorkflowHistoryEvent {
                event: "pushing".into(),
                created_at: "2025-01-01T00:00:02Z".into(),
            },
        ];

        let proto = workflow_to_proto(wf, "https://pr.url".into(), history, 3, 5);

        assert_eq!(proto.history.len(), 2);
        assert_eq!(proto.history[0].event, "implementing");
        assert_eq!(proto.history[1].event, "pushing");
        assert_eq!(proto.ticket_children_open, 3);
        assert_eq!(proto.ticket_children_closed, 5);
        assert_eq!(proto.pr_url, "https://pr.url");
        assert_eq!(proto.status, "implementing");
    }

    #[test]
    fn workflow_to_proto_handles_empty_history() {
        let wf = test_workflow();

        let proto = workflow_to_proto(wf, String::new(), vec![], 0, 0);

        assert!(proto.history.is_empty());
        assert_eq!(proto.ticket_children_open, 0);
        assert_eq!(proto.ticket_children_closed, 0);
    }

    async fn setup_handler() -> (TestDb, TicketServiceHandler) {
        let test_db = TestDb::new().await;
        let pool = test_db.db().pool().clone();
        let graph_manager = GraphManager::new(pool.clone());
        let ticket_repo = TicketRepo::new(pool.clone(), graph_manager);
        let workflow_repo = WorkflowRepo::new(pool, "test-node".to_string());
        let project_registry = crate::ProjectRegistry::new(
            std::collections::HashMap::new(),
            crate::hostexec::HostExecConfigManager::empty(),
        );
        let handler = TicketServiceHandler {
            ticket_repo,
            workflow_repo,
            project_registry,
            transition_tx: None,
            cancel_tx: None,
            ui_event_poller: None,
            worker_manager: None,
        };
        (test_db, handler)
    }

    #[tokio::test]
    async fn get_ticket_found() {
        let (_test_db, handler) = setup_handler().await;

        handler
            .ticket_repo
            .create_ticket(&NewTicket {
                id: Some("t-found".into()),
                type_: "code".into(),
                priority: 1,
                parent_id: None,
                title: "Found ticket".into(),
                body: "A body".into(),
                project: "test".into(),
                ..Default::default()
            })
            .await
            .unwrap();

        let resp = TicketService::get_ticket(
            &handler,
            Request::new(GetTicketRequest {
                id: "t-found".into(),
                activity_author_filter: None,
            }),
        )
        .await
        .unwrap();

        let ticket = resp.into_inner().ticket.unwrap();
        assert_eq!(ticket.id, "t-found");
        assert_eq!(ticket.title, "Found ticket");
        assert_eq!(ticket.body, "A body");
    }

    #[tokio::test]
    async fn get_ticket_not_found() {
        let (_test_db, handler) = setup_handler().await;

        let result = TicketService::get_ticket(
            &handler,
            Request::new(GetTicketRequest {
                id: "nonexistent".into(),
                activity_author_filter: None,
            }),
        )
        .await;

        let err = result.unwrap_err();
        assert_eq!(err.code(), Code::NotFound);
    }

    #[tokio::test]
    async fn get_workflow_includes_history_and_progress() {
        let (_test_db, handler) = setup_handler().await;

        handler
            .ticket_repo
            .create_ticket(&NewTicket {
                id: Some("t-wfhist".into()),
                type_: "code".into(),
                priority: 1,
                title: "Workflow history".into(),
                project: "test".into(),
                ..Default::default()
            })
            .await
            .unwrap();

        // Create a child ticket (open).
        handler
            .ticket_repo
            .create_ticket(&NewTicket {
                id: Some("t-wfhist-c1".into()),
                type_: "code".into(),
                priority: 1,
                title: "Child 1".into(),
                project: "test".into(),
                parent_id: Some("t-wfhist".into()),
                ..Default::default()
            })
            .await
            .unwrap();

        let wf = handler
            .workflow_repo
            .create_workflow("t-wfhist", LifecycleStatus::Open)
            .await
            .unwrap();

        handler
            .workflow_repo
            .insert_workflow_event(&wf.id, ur_rpc::workflow_event::WorkflowEvent::Implementing)
            .await
            .unwrap();

        let resp = TicketService::get_workflow(
            &handler,
            Request::new(GetWorkflowRequest {
                ticket_id: "t-wfhist".into(),
            }),
        )
        .await
        .unwrap();

        let info = resp.into_inner().workflow.unwrap();
        assert_eq!(info.history.len(), 1);
        assert_eq!(info.history[0].event, "implementing");
        assert_eq!(info.ticket_children_open, 1);
        assert_eq!(info.ticket_children_closed, 0);
    }

    #[tokio::test]
    async fn list_workflows_includes_history_and_progress() {
        let (_test_db, handler) = setup_handler().await;

        handler
            .ticket_repo
            .create_ticket(&NewTicket {
                id: Some("t-wflist".into()),
                type_: "code".into(),
                priority: 1,
                title: "Workflow list".into(),
                project: "test".into(),
                ..Default::default()
            })
            .await
            .unwrap();

        let wf = handler
            .workflow_repo
            .create_workflow("t-wflist", LifecycleStatus::Implementing)
            .await
            .unwrap();

        handler
            .workflow_repo
            .insert_workflow_event(&wf.id, ur_rpc::workflow_event::WorkflowEvent::Implementing)
            .await
            .unwrap();

        let resp = TicketService::list_workflows(
            &handler,
            Request::new(ListWorkflowsRequest {
                status: None,
                page_size: None,
                offset: None,
                project: None,
            }),
        )
        .await
        .unwrap();

        let workflows = &resp.into_inner().workflows;
        assert_eq!(workflows.len(), 1);
        assert_eq!(workflows[0].history.len(), 1);
        assert_eq!(workflows[0].history[0].event, "implementing");
    }

    // ── cancel_workflow / cancel_active_workflow tests ──────────────

    #[tokio::test]
    async fn cancel_workflow_no_workflow_is_noop() {
        let (_test_db, handler) = setup_handler().await;

        // No workflow exists for this ticket — should succeed silently.
        let resp = TicketService::cancel_workflow(
            &handler,
            Request::new(CancelWorkflowRequest {
                ticket_id: "nonexistent".into(),
            }),
        )
        .await;

        assert!(resp.is_ok());
    }

    #[tokio::test]
    async fn cancel_active_workflow_marks_cancelled_and_deletes_intents() {
        let (_test_db, handler) = setup_handler().await;

        handler
            .ticket_repo
            .create_ticket(&NewTicket {
                id: Some("t-cancel".into()),
                type_: "code".into(),
                priority: 1,
                title: "Cancel me".into(),
                project: "test".into(),
                ..Default::default()
            })
            .await
            .unwrap();

        handler
            .workflow_repo
            .create_workflow("t-cancel", LifecycleStatus::Implementing)
            .await
            .unwrap();

        // Create an intent that should be deleted on cancel.
        handler
            .workflow_repo
            .create_intent("t-cancel", LifecycleStatus::Pushing)
            .await
            .unwrap();

        let resp = TicketService::cancel_workflow(
            &handler,
            Request::new(CancelWorkflowRequest {
                ticket_id: "t-cancel".into(),
            }),
        )
        .await;

        assert!(resp.is_ok());

        // Workflow status should now be cancelled.
        // Use get_latest (not get_workflow_by_ticket which filters terminal states).
        let wf = handler
            .workflow_repo
            .get_latest_workflow_by_ticket("t-cancel")
            .await
            .unwrap()
            .expect("workflow should still exist");
        assert_eq!(wf.status, LifecycleStatus::Cancelled);

        // Intents should have been deleted.
        let intent = handler.workflow_repo.poll_intent().await.unwrap();
        assert!(
            intent.is_none(),
            "intents should be deleted after cancellation"
        );
    }

    #[tokio::test]
    async fn cancel_active_workflow_sends_cancel_signal() {
        let (_tmp, mut handler) = setup_handler().await;

        handler
            .ticket_repo
            .create_ticket(&NewTicket {
                id: Some("t-sig".into()),
                type_: "code".into(),
                priority: 1,
                title: "Signal cancel".into(),
                project: "test".into(),
                ..Default::default()
            })
            .await
            .unwrap();

        handler
            .workflow_repo
            .create_workflow("t-sig", LifecycleStatus::Implementing)
            .await
            .unwrap();

        // Wire up a cancel channel and capture what gets sent.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(1);
        handler.cancel_tx = Some(tx);

        let resp = TicketService::cancel_workflow(
            &handler,
            Request::new(CancelWorkflowRequest {
                ticket_id: "t-sig".into(),
            }),
        )
        .await;

        assert!(resp.is_ok());

        // The ticket_id should have been sent on the cancel channel.
        let received = rx.try_recv().expect("should have received cancel signal");
        assert_eq!(received, "t-sig");
    }

    #[tokio::test]
    async fn cancel_active_workflow_with_worker_id_but_no_manager() {
        let (_test_db, handler) = setup_handler().await;

        handler
            .ticket_repo
            .create_ticket(&NewTicket {
                id: Some("t-noworkmgr".into()),
                type_: "code".into(),
                priority: 1,
                title: "No worker manager".into(),
                project: "test".into(),
                ..Default::default()
            })
            .await
            .unwrap();

        handler
            .workflow_repo
            .create_workflow("t-noworkmgr", LifecycleStatus::Implementing)
            .await
            .unwrap();

        // Assign a worker_id to the workflow.
        handler
            .workflow_repo
            .set_workflow_worker_id("t-noworkmgr", "worker-123")
            .await
            .unwrap();

        // handler.worker_manager is None — kill_workflow_worker should
        // gracefully skip. The cancel should still succeed.
        let resp = TicketService::cancel_workflow(
            &handler,
            Request::new(CancelWorkflowRequest {
                ticket_id: "t-noworkmgr".into(),
            }),
        )
        .await;

        assert!(resp.is_ok());

        let wf = handler
            .workflow_repo
            .get_latest_workflow_by_ticket("t-noworkmgr")
            .await
            .unwrap()
            .expect("workflow should exist");
        assert_eq!(wf.status, LifecycleStatus::Cancelled);
    }

    #[tokio::test]
    async fn kill_workflow_worker_skips_empty_worker_id() {
        let (_test_db, handler) = setup_handler().await;
        // Should return immediately without error.
        handler.kill_workflow_worker("t-any", "").await;
    }

    #[tokio::test]
    async fn kill_workflow_worker_skips_when_no_worker_manager() {
        let (_test_db, handler) = setup_handler().await;
        assert!(handler.worker_manager.is_none());
        // Should return immediately without error even with a worker_id.
        handler.kill_workflow_worker("t-any", "worker-456").await;
    }

    fn create_request_with_branch(title: &str, branch: Option<String>) -> CreateTicketRequest {
        CreateTicketRequest {
            project: "test".into(),
            ticket_type: "code".into(),
            status: String::new(),
            priority: 1,
            parent_id: None,
            title: title.into(),
            body: String::new(),
            id: None,
            created_at: None,
            wip: false,
            branch,
        }
    }

    #[tokio::test]
    async fn create_ticket_persists_branch_when_set() {
        let (_test_db, handler) = setup_handler().await;

        let resp = TicketService::create_ticket(
            &handler,
            Request::new(create_request_with_branch(
                "with branch",
                Some("feature/foo".into()),
            )),
        )
        .await
        .unwrap();
        let id = resp.into_inner().id;

        let got = TicketService::get_ticket(
            &handler,
            Request::new(GetTicketRequest {
                id: id.clone(),
                activity_author_filter: None,
            }),
        )
        .await
        .unwrap();
        let ticket = got.into_inner().ticket.unwrap();
        assert_eq!(ticket.branch, "feature/foo");
    }

    #[tokio::test]
    async fn create_ticket_branch_defaults_to_empty_when_unset() {
        let (_test_db, handler) = setup_handler().await;

        let resp = TicketService::create_ticket(
            &handler,
            Request::new(create_request_with_branch("no branch", None)),
        )
        .await
        .unwrap();
        let id = resp.into_inner().id;

        let got = TicketService::get_ticket(
            &handler,
            Request::new(GetTicketRequest {
                id,
                activity_author_filter: None,
            }),
        )
        .await
        .unwrap();
        let ticket = got.into_inner().ticket.unwrap();
        assert_eq!(ticket.branch, "");
    }
}
