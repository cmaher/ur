use serde::Serialize;
use ur_rpc::proto::ticket::{
    ActivityEntry, DispatchableTicket, MetadataEntry, Ticket,
};

#[derive(Serialize)]
pub struct TicketJson {
    pub id: String,
    pub title: String,
    #[serde(rename = "type")]
    pub ticket_type: String,
    pub status: String,
    pub priority: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize)]
pub struct TicketDetailJson {
    #[serde(flatten)]
    pub ticket: TicketJson,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Vec<MetaJson>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub activities: Vec<ActivityJson>,
}

#[derive(Serialize)]
pub struct DispatchableJson {
    pub id: String,
    pub title: String,
    pub priority: i64,
}

#[derive(Serialize)]
pub struct MetaJson {
    pub key: String,
    pub value: String,
}

#[derive(Serialize)]
pub struct ActivityJson {
    pub id: String,
    pub timestamp: String,
    pub author: String,
    pub message: String,
}

impl From<&Ticket> for TicketJson {
    fn from(t: &Ticket) -> Self {
        Self {
            id: t.id.clone(),
            title: t.title.clone(),
            ticket_type: t.ticket_type.clone(),
            status: t.status.clone(),
            priority: t.priority,
            parent_id: if t.parent_id.is_empty() {
                None
            } else {
                Some(t.parent_id.clone())
            },
            body: t.body.clone(),
            created_at: t.created_at.clone(),
            updated_at: t.updated_at.clone(),
        }
    }
}

impl From<&DispatchableTicket> for DispatchableJson {
    fn from(t: &DispatchableTicket) -> Self {
        Self {
            id: t.id.clone(),
            title: t.title.clone(),
            priority: t.priority,
        }
    }
}

pub fn ticket_detail_json(
    ticket: &Ticket,
    metadata: &[MetadataEntry],
    activities: &[ActivityEntry],
) -> TicketDetailJson {
    TicketDetailJson {
        ticket: TicketJson::from(ticket),
        metadata: metadata
            .iter()
            .map(|m| MetaJson {
                key: m.key.clone(),
                value: m.value.clone(),
            })
            .collect(),
        activities: activities
            .iter()
            .map(|a| ActivityJson {
                id: a.id.clone(),
                timestamp: a.timestamp.clone(),
                author: a.author.clone(),
                message: a.message.clone(),
            })
            .collect(),
    }
}
