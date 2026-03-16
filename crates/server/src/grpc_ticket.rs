use std::collections::HashMap;

use tonic::{Request, Response, Status};
use tracing::info;
use uuid::Uuid;

use ur_db::{EdgeKind, NewTicket, TicketFilter, TicketRepo, TicketUpdate};
use ur_rpc::proto::ticket::ticket_service_server::TicketService;
use ur_rpc::proto::ticket::{
    AddActivityRequest, AddActivityResponse, AddBlockRequest, AddBlockResponse, AddLinkRequest,
    AddLinkResponse, CreateTicketRequest, CreateTicketResponse, DeleteMetaRequest,
    DeleteMetaResponse, DispatchableTicketsRequest, DispatchableTicketsResponse, GetTicketRequest,
    GetTicketResponse, ListActivitiesRequest, ListActivitiesResponse, ListTicketsRequest,
    ListTicketsResponse, RemoveBlockRequest, RemoveBlockResponse, RemoveLinkRequest,
    RemoveLinkResponse, SetMetaRequest, SetMetaResponse, UpdateTicketRequest, UpdateTicketResponse,
};

/// gRPC implementation of the TicketService, delegating to `TicketRepo`.
#[derive(Clone)]
pub struct TicketServiceHandler {
    pub ticket_repo: TicketRepo,
}

/// Convert a database error into a gRPC `Status::internal`.
fn db_err(e: impl std::fmt::Display) -> Status {
    Status::internal(e.to_string())
}

#[tonic::async_trait]
impl TicketService for TicketServiceHandler {
    async fn create_ticket(
        &self,
        req: Request<CreateTicketRequest>,
    ) -> Result<Response<CreateTicketResponse>, Status> {
        let req = req.into_inner();
        info!(project = %req.project, title = %req.title, "create_ticket request");

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

        let new_ticket = NewTicket {
            id: id.clone(),
            type_: req.ticket_type,
            priority: req.priority as i32,
            parent_id,
            title: req.title,
            body: req.body,
            status,
            created_at,
        };

        self.ticket_repo
            .create_ticket(&new_ticket)
            .await
            .map_err(db_err)?;

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
                    .map_err(db_err)?;
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
                    })
                    .collect()
            }
            (Some(key), None) => {
                let matches = self
                    .ticket_repo
                    .tickets_with_metadata_key(key)
                    .await
                    .map_err(db_err)?;
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
                    })
                    .collect()
            }
            _ => {
                let filter = TicketFilter {
                    status: req.status.filter(|s| !s.is_empty()),
                    type_: req.ticket_type.filter(|s| !s.is_empty()),
                    parent_id: req.parent_id.filter(|s| !s.is_empty()),
                };

                let db_tickets = self
                    .ticket_repo
                    .list_tickets(&filter)
                    .await
                    .map_err(db_err)?;

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
            .map_err(db_err)?
            .ok_or_else(|| Status::not_found(format!("ticket not found: {}", req.id)))?;

        let meta = self
            .ticket_repo
            .get_meta(&req.id, "ticket")
            .await
            .map_err(db_err)?;

        let activities_list = self
            .ticket_repo
            .get_activities(&req.id)
            .await
            .map_err(db_err)?;

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

        let update = TicketUpdate {
            status: req.status.filter(|s| !s.is_empty()),
            type_: req.ticket_type.filter(|s| !s.is_empty()),
            priority: req.priority.map(|p| p as i32),
            title: req.title.filter(|s| !s.is_empty()),
            body: req.body.filter(|s| !s.is_empty()),
            parent_id: None,
        };

        if update.status.as_deref() == Some("closed") && !req.force {
            let all_closed = self
                .ticket_repo
                .epic_all_children_closed(&req.id)
                .await
                .map_err(db_err)?;
            if !all_closed {
                return Err(Status::failed_precondition(format!(
                    "ticket {} has open children; close them first or use --force",
                    req.id
                )));
            }
        }

        self.ticket_repo
            .update_ticket(&req.id, &update)
            .await
            .map_err(db_err)?;

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
            .map_err(db_err)?;

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
            .map_err(db_err)?;

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
            .map_err(db_err)?;

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
            .map_err(db_err)?;

        Ok(Response::new(RemoveBlockResponse {}))
    }

    async fn add_link(
        &self,
        req: Request<AddLinkRequest>,
    ) -> Result<Response<AddLinkResponse>, Status> {
        let req = req.into_inner();
        info!(left = %req.left_id, right = %req.right_id, "add_link request");

        self.ticket_repo
            .add_edge(&req.left_id, &req.right_id, EdgeKind::RelatesTo)
            .await
            .map_err(db_err)?;

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
            .map_err(db_err)?;

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
            .map_err(db_err)?;

        for (key, value) in &meta {
            self.ticket_repo
                .set_meta(&activity.id, "activity", key, value)
                .await
                .map_err(db_err)?;
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
            .map_err(db_err)?;

        let mut activities = Vec::new();
        for a in activities_list {
            let meta = self
                .ticket_repo
                .get_meta(&a.id, "activity")
                .await
                .map_err(db_err)?;

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

        let tickets = self
            .ticket_repo
            .dispatchable_tickets(&req.epic_id)
            .await
            .map_err(db_err)?;

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
}
