use std::collections::HashMap;

use tonic::{Code, Request, Response, Status};
use tracing::info;

use ur_db::{KnowledgeFilter, KnowledgeRepo, KnowledgeUpdate, NewKnowledge};
use ur_rpc::error::{self, INTERNAL, INVALID_ARGUMENT, NOT_FOUND};
use ur_rpc::proto::knowledge::knowledge_service_server::KnowledgeService;
use ur_rpc::proto::knowledge::{
    CreateKnowledgeRequest, CreateKnowledgeResponse, DeleteKnowledgeRequest,
    DeleteKnowledgeResponse, GetKnowledgeRequest, GetKnowledgeResponse, KnowledgeDoc,
    KnowledgeSummary as ProtoKnowledgeSummary, ListKnowledgeRequest, ListKnowledgeResponse,
    ListTagsRequest, ListTagsResponse, UpdateKnowledgeRequest, UpdateKnowledgeResponse,
};

const DOMAIN_KNOWLEDGE: &str = "ur.knowledge";
const MAX_DESCRIPTION_LEN: usize = 120;

#[derive(Debug, thiserror::Error)]
pub enum KnowledgeError {
    #[error("knowledge doc not found: {id}")]
    NotFound { id: String },

    #[error("validation error: {0}")]
    Validation(String),

    #[error("database error: {0}")]
    Db(String),
}

impl From<KnowledgeError> for Status {
    fn from(err: KnowledgeError) -> Self {
        match err {
            KnowledgeError::NotFound { ref id } => {
                let mut meta = HashMap::new();
                meta.insert("knowledge_id".into(), id.clone());
                error::status_with_info(
                    Code::NotFound,
                    err.to_string(),
                    DOMAIN_KNOWLEDGE,
                    NOT_FOUND,
                    meta,
                )
            }
            KnowledgeError::Validation(_) => error::status_with_info(
                Code::InvalidArgument,
                err.to_string(),
                DOMAIN_KNOWLEDGE,
                INVALID_ARGUMENT,
                HashMap::new(),
            ),
            KnowledgeError::Db(_) => error::status_with_info(
                Code::Internal,
                err.to_string(),
                DOMAIN_KNOWLEDGE,
                INTERNAL,
                HashMap::new(),
            ),
        }
    }
}

/// gRPC implementation of the KnowledgeService, delegating to `KnowledgeRepo`.
#[derive(Clone)]
pub struct KnowledgeServiceHandler {
    pub knowledge_repo: KnowledgeRepo,
    pub valid_projects: std::collections::HashSet<String>,
}

fn validate_title_length(title: &str) -> Result<(), KnowledgeError> {
    if title.len() > MAX_DESCRIPTION_LEN {
        return Err(KnowledgeError::Validation(format!(
            "title exceeds maximum length of {MAX_DESCRIPTION_LEN} characters"
        )));
    }
    Ok(())
}

fn validate_project(
    source: &str,
    valid_projects: &std::collections::HashSet<String>,
) -> Result<(), KnowledgeError> {
    if !valid_projects.is_empty() && !source.is_empty() && !valid_projects.contains(source) {
        return Err(KnowledgeError::Validation(format!(
            "unknown project '{}'; configured projects: {}",
            source,
            valid_projects
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
        )));
    }
    Ok(())
}

fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for tag in tags {
        let normalized = tag.trim().to_lowercase();
        if !normalized.is_empty() && seen.insert(normalized.clone()) {
            result.push(normalized);
        }
    }
    result.sort();
    result
}

#[tonic::async_trait]
impl KnowledgeService for KnowledgeServiceHandler {
    async fn create_knowledge(
        &self,
        req: Request<CreateKnowledgeRequest>,
    ) -> Result<Response<CreateKnowledgeResponse>, Status> {
        let req = req.into_inner();
        info!(title = %req.title, source = %req.source, "create_knowledge request");

        validate_title_length(&req.title)?;
        validate_project(&req.source, &self.valid_projects)?;

        let project = if req.source.is_empty() {
            None
        } else {
            Some(req.source)
        };
        let tags = normalize_tags(&req.tags);

        let new_knowledge = NewKnowledge {
            project,
            title: req.title,
            description: String::new(),
            body: req.content,
            tags,
        };

        let doc = self
            .knowledge_repo
            .create(&new_knowledge)
            .await
            .map_err(|e| KnowledgeError::Db(e.to_string()))?;

        Ok(Response::new(CreateKnowledgeResponse { id: doc.id }))
    }

    async fn get_knowledge(
        &self,
        req: Request<GetKnowledgeRequest>,
    ) -> Result<Response<GetKnowledgeResponse>, Status> {
        let req = req.into_inner();
        info!(id = %req.id, "get_knowledge request");

        let doc = self
            .knowledge_repo
            .get(&req.id)
            .await
            .map_err(|e| KnowledgeError::Db(e.to_string()))?
            .ok_or_else(|| KnowledgeError::NotFound { id: req.id.clone() })?;

        Ok(Response::new(GetKnowledgeResponse {
            doc: Some(KnowledgeDoc {
                id: doc.id,
                title: doc.title,
                content: doc.body,
                source: doc.project.unwrap_or_default(),
                tags: doc.tags,
                created_at: doc.created_at,
                updated_at: doc.updated_at,
            }),
        }))
    }

    async fn update_knowledge(
        &self,
        req: Request<UpdateKnowledgeRequest>,
    ) -> Result<Response<UpdateKnowledgeResponse>, Status> {
        let req = req.into_inner();
        info!(id = %req.id, "update_knowledge request");

        if let Some(ref title) = req.title {
            validate_title_length(title)?;
        }

        let tags = if req.update_tags {
            Some(normalize_tags(&req.tags))
        } else {
            None
        };

        let update = KnowledgeUpdate {
            title: req.title,
            description: None,
            body: req.content,
            tags,
        };

        self.knowledge_repo
            .update(&req.id, &update)
            .await
            .map_err(|e| {
                if e.to_string().contains("RowNotFound") {
                    KnowledgeError::NotFound { id: req.id.clone() }
                } else {
                    KnowledgeError::Db(e.to_string())
                }
            })?;

        Ok(Response::new(UpdateKnowledgeResponse {}))
    }

    async fn delete_knowledge(
        &self,
        req: Request<DeleteKnowledgeRequest>,
    ) -> Result<Response<DeleteKnowledgeResponse>, Status> {
        let req = req.into_inner();
        info!(id = %req.id, "delete_knowledge request");

        let deleted = self
            .knowledge_repo
            .delete(&req.id)
            .await
            .map_err(|e| KnowledgeError::Db(e.to_string()))?;

        if !deleted {
            return Err(KnowledgeError::NotFound { id: req.id }.into());
        }

        Ok(Response::new(DeleteKnowledgeResponse {}))
    }

    async fn list_knowledge(
        &self,
        req: Request<ListKnowledgeRequest>,
    ) -> Result<Response<ListKnowledgeResponse>, Status> {
        let req = req.into_inner();
        info!("list_knowledge request");

        if let Some(ref source) = req.source {
            validate_project(source, &self.valid_projects)?;
        }

        let filter = KnowledgeFilter {
            project: req.source.filter(|s| !s.is_empty()),
            shared: false,
            tag: req.tag.filter(|s| !s.is_empty()),
        };

        let summaries = self
            .knowledge_repo
            .list(&filter)
            .await
            .map_err(|e| KnowledgeError::Db(e.to_string()))?;

        let docs = summaries
            .into_iter()
            .map(|s| ProtoKnowledgeSummary {
                id: s.id,
                title: s.title,
                source: String::new(),
                tags: s.tags,
                created_at: String::new(),
                updated_at: String::new(),
            })
            .collect();

        Ok(Response::new(ListKnowledgeResponse { docs }))
    }

    async fn list_tags(
        &self,
        _req: Request<ListTagsRequest>,
    ) -> Result<Response<ListTagsResponse>, Status> {
        info!("list_tags request");

        let tags = self
            .knowledge_repo
            .list_tags()
            .await
            .map_err(|e| KnowledgeError::Db(e.to_string()))?;

        Ok(Response::new(ListTagsResponse { tags }))
    }
}
