use std::collections::HashMap;

use tonic::{Request, Response, Status};
use tracing::info;

use ur_db::DatabaseManager;
use ur_rpc::proto::ticket::ticket_service_server::TicketService;
use ur_rpc::proto::ticket::{
    AddActivityRequest, AddActivityResponse, AddBlockRequest, AddBlockResponse, AddLinkRequest,
    AddLinkResponse, CreateTicketRequest, CreateTicketResponse, DeleteMetaRequest,
    DeleteMetaResponse, DispatchableTicketsRequest, DispatchableTicketsResponse, GetTicketRequest,
    GetTicketResponse, ListActivitiesRequest, ListActivitiesResponse, ListTicketsRequest,
    ListTicketsResponse, RemoveBlockRequest, RemoveBlockResponse, RemoveLinkRequest,
    RemoveLinkResponse, SetMetaRequest, SetMetaResponse, UpdateTicketRequest, UpdateTicketResponse,
};

/// gRPC implementation of the TicketService, delegating to `DatabaseManager`.
#[derive(Clone)]
pub struct TicketServiceHandler {
    pub db: DatabaseManager,
}

/// Convert a `DatabaseManager` `Result<T, String>` error into a gRPC `Status::internal`.
fn db_err(e: String) -> Status {
    Status::internal(e)
}

#[tonic::async_trait]
impl TicketService for TicketServiceHandler {
    async fn create_ticket(
        &self,
        req: Request<CreateTicketRequest>,
    ) -> Result<Response<CreateTicketResponse>, Status> {
        let req = req.into_inner();
        info!(project = %req.project, title = %req.title, "create_ticket request");

        let params = ur_db::CreateTicketParams {
            ticket_type: req.ticket_type,
            status: req.status,
            priority: req.priority,
            parent_id: req.parent_id,
            title: req.title,
            body: req.body,
        };

        let id = self
            .db
            .create_ticket(&req.project, &params)
            .map_err(db_err)?;

        Ok(Response::new(CreateTicketResponse { id }))
    }

    async fn list_tickets(
        &self,
        req: Request<ListTicketsRequest>,
    ) -> Result<Response<ListTicketsResponse>, Status> {
        let req = req.into_inner();
        info!("list_tickets request");

        let filters = ur_db::ListTicketFilters {
            project: req.project,
            ticket_type: req.ticket_type,
            status: req.status,
            parent_id: req.parent_id,
            meta_key: req.meta_key,
            meta_value: req.meta_value,
        };

        let tickets = self.db.list_tickets(&filters).map_err(db_err)?;

        let proto_tickets = tickets
            .into_iter()
            .map(|t| ur_rpc::proto::ticket::Ticket {
                id: t.id,
                ticket_type: t.ticket_type,
                status: t.status,
                priority: t.priority,
                parent_id: t.parent_id,
                title: t.title,
                body: t.body,
                created_at: t.created_at,
                updated_at: t.updated_at,
            })
            .collect();

        Ok(Response::new(ListTicketsResponse {
            tickets: proto_tickets,
        }))
    }

    async fn get_ticket(
        &self,
        req: Request<GetTicketRequest>,
    ) -> Result<Response<GetTicketResponse>, Status> {
        let req = req.into_inner();
        info!(id = %req.id, "get_ticket request");

        let detail = self.db.get_ticket(&req.id).map_err(db_err)?;

        let ticket = ur_rpc::proto::ticket::Ticket {
            id: detail.ticket.id,
            ticket_type: detail.ticket.ticket_type,
            status: detail.ticket.status,
            priority: detail.ticket.priority,
            parent_id: detail.ticket.parent_id,
            title: detail.ticket.title,
            body: detail.ticket.body,
            created_at: detail.ticket.created_at,
            updated_at: detail.ticket.updated_at,
        };

        let metadata = detail
            .metadata
            .into_iter()
            .map(|m| ur_rpc::proto::ticket::MetadataEntry {
                key: m.key,
                value: m.value,
            })
            .collect();

        let activities = detail
            .activities
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

        let fields = ur_db::UpdateTicketFields {
            status: req.status,
            priority: req.priority,
            title: req.title,
            body: req.body,
        };

        self.db.update_ticket(&req.id, &fields).map_err(db_err)?;

        Ok(Response::new(UpdateTicketResponse {}))
    }

    async fn set_meta(
        &self,
        req: Request<SetMetaRequest>,
    ) -> Result<Response<SetMetaResponse>, Status> {
        let req = req.into_inner();
        info!(ticket_id = %req.ticket_id, key = %req.key, "set_meta request");

        self.db
            .set_meta(&req.ticket_id, &req.key, &req.value)
            .map_err(db_err)?;

        Ok(Response::new(SetMetaResponse {}))
    }

    async fn delete_meta(
        &self,
        req: Request<DeleteMetaRequest>,
    ) -> Result<Response<DeleteMetaResponse>, Status> {
        let req = req.into_inner();
        info!(ticket_id = %req.ticket_id, key = %req.key, "delete_meta request");

        self.db
            .delete_meta(&req.ticket_id, &req.key)
            .map_err(db_err)?;

        Ok(Response::new(DeleteMetaResponse {}))
    }

    async fn add_block(
        &self,
        req: Request<AddBlockRequest>,
    ) -> Result<Response<AddBlockResponse>, Status> {
        let req = req.into_inner();
        info!(blocker = %req.blocker_id, blocked = %req.blocked_id, "add_block request");

        self.db
            .add_block(&req.blocker_id, &req.blocked_id)
            .map_err(db_err)?;

        Ok(Response::new(AddBlockResponse {}))
    }

    async fn remove_block(
        &self,
        req: Request<RemoveBlockRequest>,
    ) -> Result<Response<RemoveBlockResponse>, Status> {
        let req = req.into_inner();
        info!(blocker = %req.blocker_id, blocked = %req.blocked_id, "remove_block request");

        self.db
            .remove_block(&req.blocker_id, &req.blocked_id)
            .map_err(db_err)?;

        Ok(Response::new(RemoveBlockResponse {}))
    }

    async fn add_link(
        &self,
        req: Request<AddLinkRequest>,
    ) -> Result<Response<AddLinkResponse>, Status> {
        let req = req.into_inner();
        info!(left = %req.left_id, right = %req.right_id, "add_link request");

        self.db
            .add_link(&req.left_id, &req.right_id)
            .map_err(db_err)?;

        Ok(Response::new(AddLinkResponse {}))
    }

    async fn remove_link(
        &self,
        req: Request<RemoveLinkRequest>,
    ) -> Result<Response<RemoveLinkResponse>, Status> {
        let req = req.into_inner();
        info!(left = %req.left_id, right = %req.right_id, "remove_link request");

        self.db
            .remove_link(&req.left_id, &req.right_id)
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

        let activity_id = self
            .db
            .add_activity(&req.ticket_id, &req.author, &req.message, &meta)
            .map_err(db_err)?;

        Ok(Response::new(AddActivityResponse { activity_id }))
    }

    async fn list_activities(
        &self,
        req: Request<ListActivitiesRequest>,
    ) -> Result<Response<ListActivitiesResponse>, Status> {
        let req = req.into_inner();
        info!(ticket_id = %req.ticket_id, "list_activities request");

        let details = self.db.list_activities(&req.ticket_id).map_err(db_err)?;

        let activities = details
            .into_iter()
            .map(|d| ur_rpc::proto::ticket::ActivityDetail {
                entry: Some(ur_rpc::proto::ticket::ActivityEntry {
                    id: d.entry.id,
                    timestamp: d.entry.timestamp,
                    author: d.entry.author,
                    message: d.entry.message,
                }),
                metadata: d
                    .metadata
                    .into_iter()
                    .map(|m| ur_rpc::proto::ticket::ActivityMetadataEntry {
                        key: m.key,
                        value: m.value,
                    })
                    .collect(),
            })
            .collect();

        Ok(Response::new(ListActivitiesResponse { activities }))
    }

    async fn dispatchable_tickets(
        &self,
        req: Request<DispatchableTicketsRequest>,
    ) -> Result<Response<DispatchableTicketsResponse>, Status> {
        let req = req.into_inner();
        info!(epic_id = %req.epic_id, "dispatchable_tickets request");

        let tickets = self.db.dispatchable_tickets(&req.epic_id).map_err(db_err)?;

        let proto_tickets = tickets
            .into_iter()
            .map(|t| ur_rpc::proto::ticket::DispatchableTicket {
                id: t.id,
                title: t.title,
                priority: t.priority,
            })
            .collect();

        Ok(Response::new(DispatchableTicketsResponse {
            tickets: proto_tickets,
        }))
    }
}
