use std::collections::{HashMap, HashSet};

use tonic::{Code, Request, Response, Status};
use tracing::info;
use uuid::Uuid;

use ur_db::{EdgeKind, LifecycleStatus, NewTicket, TicketFilter, TicketRepo, TicketUpdate};
use ur_rpc::error::{
    self, DOMAIN_TICKET, INTERNAL, INVALID_ARGUMENT, NOT_FOUND, TICKET_HAS_OPEN_CHILDREN,
};
use ur_rpc::proto::ticket::ticket_service_server::TicketService;
use ur_rpc::proto::ticket::{
    AddActivityRequest, AddActivityResponse, AddBlockRequest, AddBlockResponse, AddLinkRequest,
    AddLinkResponse, CancelWorkflowRequest, CancelWorkflowResponse, CreateTicketRequest,
    CreateTicketResponse, CreateWorkflowRequest, CreateWorkflowResponse, DeleteMetaRequest,
    DeleteMetaResponse, DispatchableTicketsRequest, DispatchableTicketsResponse, GetTicketRequest,
    GetTicketResponse, ListActivitiesRequest, ListActivitiesResponse, ListTicketsRequest,
    ListTicketsResponse, RedriveTicketRequest, RedriveTicketResponse, RemoveBlockRequest,
    RemoveBlockResponse, RemoveLinkRequest, RemoveLinkResponse, SetMetaRequest, SetMetaResponse,
    UpdateTicketRequest, UpdateTicketResponse,
};

#[derive(Debug, thiserror::Error)]
pub enum TicketError {
    #[error("ticket not found: {id}")]
    NotFound { id: String },

    #[error("ticket {id} has open children; close them first or use --force")]
    HasOpenChildren { id: String, children: Vec<String> },

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
    pub valid_projects: HashSet<String>,
    /// Optional channel sender for workflow transition requests.
    /// None on the worker server (no workflow engine).
    pub transition_tx: Option<tokio::sync::mpsc::Sender<crate::workflow::TransitionRequest>>,
    /// Optional channel sender for workflow cancellation requests.
    /// None on the worker server (no workflow engine).
    pub cancel_tx: Option<tokio::sync::mpsc::Sender<String>>,
}

impl TicketServiceHandler {
    /// If the ticket has an active workflow, cancel it: signal the coordinator
    /// to abort the in-flight handler, then delete the workflow and intent rows.
    async fn cancel_active_workflow(&self, ticket_id: &str) -> Result<(), Status> {
        let workflow = self
            .ticket_repo
            .get_workflow_by_ticket(ticket_id)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        if workflow.is_none() {
            return Ok(());
        }

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

        // Delete intents and workflow from the database.
        self.ticket_repo
            .delete_intents_for_ticket(ticket_id)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        self.ticket_repo
            .delete_workflow(ticket_id)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        Ok(())
    }
}

#[tonic::async_trait]
impl TicketService for TicketServiceHandler {
    async fn create_ticket(
        &self,
        req: Request<CreateTicketRequest>,
    ) -> Result<Response<CreateTicketResponse>, Status> {
        let req = req.into_inner();
        info!(project = %req.project, title = %req.title, "create_ticket request");

        if !self.valid_projects.is_empty() && !self.valid_projects.contains(&req.project) {
            return Err(TicketError::Validation(format!(
                "unknown project '{}'; configured projects: {}",
                req.project,
                self.valid_projects
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
            ))
            .into());
        }

        let id = req
            .id
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("ur-{}", &Uuid::new_v4().to_string()[..5]));

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

        let new_ticket = NewTicket {
            id: id.clone(),
            project: req.project,
            type_: req.ticket_type,
            priority: req.priority as i32,
            parent_id,
            title: req.title,
            body: req.body,
            status,
            lifecycle_status,
            branch: None,
            created_at,
        };

        self.ticket_repo
            .create_ticket(&new_ticket)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        Ok(Response::new(CreateTicketResponse { id }))
    }

    async fn list_tickets(
        &self,
        req: Request<ListTicketsRequest>,
    ) -> Result<Response<ListTicketsResponse>, Status> {
        let req = req.into_inner();
        info!("list_tickets request");

        let meta_key = req.meta_key.filter(|s| !s.is_empty());
        let meta_value = req.meta_value.filter(|s| !s.is_empty());

        // If metadata filters are provided, use the metadata-based queries
        let tickets = match (&meta_key, &meta_value) {
            (Some(key), Some(value)) => {
                let matches = self
                    .ticket_repo
                    .tickets_by_metadata(key, value)
                    .await
                    .map_err(|e| TicketError::Db(e.to_string()))?;
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
                    })
                    .collect()
            }
            (Some(key), None) => {
                let matches = self
                    .ticket_repo
                    .tickets_with_metadata_key(key)
                    .await
                    .map_err(|e| TicketError::Db(e.to_string()))?;
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
                    })
                    .collect()
            }
            _ => {
                let filter = TicketFilter {
                    project: req.project.filter(|s| !s.is_empty()),
                    status: req.status.filter(|s| !s.is_empty()),
                    type_: req.ticket_type.filter(|s| !s.is_empty()),
                    parent_id: req.parent_id.filter(|s| !s.is_empty()),
                    lifecycle_status: None,
                };

                let db_tickets = self
                    .ticket_repo
                    .list_tickets(&filter)
                    .await
                    .map_err(|e| TicketError::Db(e.to_string()))?;

                db_tickets
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
                    })
                    .collect()
            }
        };

        Ok(Response::new(ListTicketsResponse { tickets }))
    }

    async fn get_ticket(
        &self,
        req: Request<GetTicketRequest>,
    ) -> Result<Response<GetTicketResponse>, Status> {
        let req = req.into_inner();
        info!(id = %req.id, "get_ticket request");

        let t = self
            .ticket_repo
            .get_ticket(&req.id)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?
            .ok_or_else(|| TicketError::NotFound { id: req.id.clone() })?;

        let meta = self
            .ticket_repo
            .get_meta(&req.id, "ticket")
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        let activities_list = self
            .ticket_repo
            .get_activities(&req.id)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

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
        if let Some(ref p) = project
            && !self.valid_projects.is_empty()
            && !self.valid_projects.contains(p)
        {
            return Err(TicketError::Validation(format!(
                "unknown project '{}'; configured projects: {}",
                p,
                self.valid_projects
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

            self.cancel_active_workflow(&req.id).await?;
        }

        self.ticket_repo
            .update_ticket(&req.id, &update)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        Ok(Response::new(UpdateTicketResponse {}))
    }

    async fn set_meta(
        &self,
        req: Request<SetMetaRequest>,
    ) -> Result<Response<SetMetaResponse>, Status> {
        let req = req.into_inner();
        info!(ticket_id = %req.ticket_id, key = %req.key, "set_meta request");

        self.ticket_repo
            .set_meta(&req.ticket_id, "ticket", &req.key, &req.value)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

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
            "follow_up" => EdgeKind::FollowUp,
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

        // Cancel any existing workflow before creating a new one.
        self.cancel_active_workflow(&req.ticket_id).await?;

        // Create the workflow row.
        let workflow = self
            .ticket_repo
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

        // 1. Clear workflow stall (stalled flag + stall_reason on workflow table).
        let _ = self.ticket_repo.clear_workflow_stall(&req.id).await;

        // 2. Clear legacy stall_reason metadata.
        let _ = self
            .ticket_repo
            .delete_meta(&req.id, "ticket", "stall_reason")
            .await;

        // 3. Set lifecycle to the target status.
        let update = TicketUpdate {
            lifecycle_status: Some(to_status),
            status: None,
            lifecycle_managed: None,
            type_: None,
            priority: None,
            title: None,
            body: None,
            branch: None,
            parent_id: None,
            project: None,
        };
        self.ticket_repo
            .update_ticket(&req.id, &update)
            .await
            .map_err(|e| TicketError::Db(e.to_string()))?;

        // 4. Delete any stale workflow events for this ticket (from the trigger).
        self.ticket_repo
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
}
