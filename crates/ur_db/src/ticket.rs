use crate::DatabaseManager;
use rand::Rng;

/// A full ticket record with all core fields.
#[derive(Debug, Clone)]
pub struct Ticket {
    pub id: String,
    pub ticket_type: String,
    pub status: String,
    pub priority: i64,
    pub parent_id: String,
    pub title: String,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A ticket together with its metadata and activity log entries.
#[derive(Debug, Clone)]
pub struct TicketDetail {
    pub ticket: Ticket,
    pub metadata: Vec<MetadataEntry>,
    pub activities: Vec<ActivityEntry>,
}

/// A single key-value metadata entry.
#[derive(Debug, Clone)]
pub struct MetadataEntry {
    pub key: String,
    pub value: String,
}

/// A single activity log entry.
#[derive(Debug, Clone)]
pub struct ActivityEntry {
    pub id: String,
    pub timestamp: String,
    pub author: String,
    pub message: String,
}

/// Parameters for creating a new ticket.
pub struct CreateTicketParams {
    pub ticket_type: String,
    pub status: String,
    pub priority: i64,
    pub parent_id: Option<String>,
    pub title: String,
    pub body: String,
}

/// Fields that can be partially updated on a ticket.
pub struct UpdateTicketFields {
    pub status: Option<String>,
    pub priority: Option<i64>,
    pub title: Option<String>,
    pub body: Option<String>,
}

/// Filters for listing tickets.
#[derive(Default)]
pub struct ListTicketFilters {
    pub project: Option<String>,
    pub ticket_type: Option<String>,
    pub status: Option<String>,
    pub parent_id: Option<String>,
    pub meta_key: Option<String>,
    pub meta_value: Option<String>,
}

/// Generate a 4-character random alphanumeric string (lowercase).
fn generate_short_id() -> String {
    let mut rng = rand::thread_rng();
    (0..4)
        .map(|_| {
            let idx = rng.gen_range(0..36);
            if idx < 10 {
                (b'0' + idx) as char
            } else {
                (b'a' + idx - 10) as char
            }
        })
        .collect()
}

/// Get the current UTC timestamp in ISO 8601 format.
fn now_utc() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    days += 719468;
    let era = days / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Escape a string value for safe use inside CozoScript single-quoted string literals.
/// Doubles any backslashes and escapes single quotes.
pub(crate) fn escape_cozo(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

impl DatabaseManager {
    /// Generate a unique top-level ticket ID with collision retry.
    fn generate_top_level_id(&self, project: &str) -> Result<String, String> {
        loop {
            let candidate = format!("{project}.{}", generate_short_id());
            let check = format!(
                "?[id] := *ticket{{id}}, id = '{cid}'",
                cid = escape_cozo(&candidate)
            );
            let result = self.run(&check)?;
            if result.rows.is_empty() {
                return Ok(candidate);
            }
        }
    }

    /// Generate a child ticket ID by finding the next sequential number under a parent.
    fn generate_child_id(&self, parent_id: &str) -> Result<String, String> {
        // Verify parent exists
        let check = format!(
            "?[id] := *ticket{{id}}, id = '{pid}'",
            pid = escape_cozo(parent_id)
        );
        let result = self.run(&check)?;
        if result.rows.is_empty() {
            return Err(format!("Parent ticket not found: {parent_id}"));
        }
        // Find next sequential child number
        let count_script = format!(
            "?[id] := *ticket{{id, parent_id}}, parent_id = '{pid}'",
            pid = escape_cozo(parent_id)
        );
        let children = self.run(&count_script)?;
        let next_n = children.rows.len();
        Ok(format!("{parent_id}.{next_n}"))
    }

    /// Create a new ticket, auto-generating the ID.
    ///
    /// For top-level tickets (no parent), generates `{project}.XXXX` where XXXX is
    /// 4 random alphanumeric characters. For child tickets, generates
    /// `{parent_id}.N` where N is the next sequential child number.
    ///
    /// Returns the generated ticket ID.
    pub fn create_ticket(
        &self,
        project: &str,
        params: &CreateTicketParams,
    ) -> Result<String, String> {
        let id = match &params.parent_id {
            Some(parent_id) => self.generate_child_id(parent_id)?,
            None => self.generate_top_level_id(project)?,
        };

        let now = now_utc();
        let parent_id_val = params.parent_id.as_deref().unwrap_or("");

        let script = format!(
            "?[id, type, status, priority, parent_id, title, body, created_at, updated_at] <- [[
                '{id}',
                '{ticket_type}',
                '{status}',
                {priority},
                '{parent_id}',
                '{title}',
                '{body}',
                '{now}',
                '{now}'
            ]]
            :put ticket {{id => type, status, priority, parent_id, title, body, created_at, updated_at}}",
            id = escape_cozo(&id),
            ticket_type = escape_cozo(&params.ticket_type),
            status = escape_cozo(&params.status),
            priority = params.priority,
            parent_id = escape_cozo(parent_id_val),
            title = escape_cozo(&params.title),
            body = escape_cozo(&params.body),
            now = escape_cozo(&now),
        );

        self.run(&script)?;
        Ok(id)
    }

    /// List tickets matching the given filters.
    ///
    /// All filter fields are optional. When multiple filters are set, they are
    /// combined with AND semantics.
    pub fn list_tickets(&self, filters: &ListTicketFilters) -> Result<Vec<Ticket>, String> {
        let mut conditions = Vec::new();

        let base =
            "*ticket{id, type, status, priority, parent_id, title, body, created_at, updated_at}";

        if let Some(ref project) = filters.project {
            conditions.push(format!(
                "starts_with(id, '{pref}.')",
                pref = escape_cozo(project)
            ));
        }
        if let Some(ref t) = filters.ticket_type {
            conditions.push(format!("type = '{val}'", val = escape_cozo(t)));
        }
        if let Some(ref s) = filters.status {
            conditions.push(format!("status = '{val}'", val = escape_cozo(s)));
        }
        if let Some(ref pid) = filters.parent_id {
            conditions.push(format!("parent_id = '{val}'", val = escape_cozo(pid)));
        }

        let meta_join = if let Some(ref key) = filters.meta_key {
            let mut meta_conds = format!(
                "*ticket_meta{{ticket_id, key, value}}, ticket_id = id, key = '{k}'",
                k = escape_cozo(key)
            );
            if let Some(ref v) = filters.meta_value {
                meta_conds.push_str(&format!(", value = '{val}'", val = escape_cozo(v)));
            }
            format!(", {meta_conds}")
        } else {
            String::new()
        };

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!(", {}", conditions.join(", "))
        };

        let script = format!(
            "?[id, type, status, priority, parent_id, title, body, created_at, updated_at] :=
                {base}{meta_join}{where_clause}
            :order id",
        );

        let result = self.run(&script)?;
        Ok(result
            .rows
            .iter()
            .map(|r| Ticket {
                id: r[0].get_str().unwrap().to_string(),
                ticket_type: r[1].get_str().unwrap().to_string(),
                status: r[2].get_str().unwrap().to_string(),
                priority: r[3].get_int().unwrap(),
                parent_id: r[4].get_str().unwrap().to_string(),
                title: r[5].get_str().unwrap().to_string(),
                body: r[6].get_str().unwrap().to_string(),
                created_at: r[7].get_str().unwrap().to_string(),
                updated_at: r[8].get_str().unwrap().to_string(),
            })
            .collect())
    }

    /// Get a single ticket by ID, including its metadata and activity log.
    ///
    /// Returns an error if the ticket is not found.
    pub fn get_ticket(&self, id: &str) -> Result<TicketDetail, String> {
        let eid = escape_cozo(id);

        let ticket_script = format!(
            "?[id, type, status, priority, parent_id, title, body, created_at, updated_at] :=
                *ticket{{id, type, status, priority, parent_id, title, body, created_at, updated_at}},
                id = '{eid}'",
        );
        let ticket_result = self.run(&ticket_script)?;
        if ticket_result.rows.is_empty() {
            return Err(format!("Ticket not found: {id}"));
        }
        let r = &ticket_result.rows[0];
        let ticket = Ticket {
            id: r[0].get_str().unwrap().to_string(),
            ticket_type: r[1].get_str().unwrap().to_string(),
            status: r[2].get_str().unwrap().to_string(),
            priority: r[3].get_int().unwrap(),
            parent_id: r[4].get_str().unwrap().to_string(),
            title: r[5].get_str().unwrap().to_string(),
            body: r[6].get_str().unwrap().to_string(),
            created_at: r[7].get_str().unwrap().to_string(),
            updated_at: r[8].get_str().unwrap().to_string(),
        };

        let meta_script = format!(
            "?[key, value] :=
                *ticket_meta{{ticket_id, key, value}},
                ticket_id = '{eid}'
            :order key",
        );
        let meta_result = self.run(&meta_script)?;
        let metadata = meta_result
            .rows
            .iter()
            .map(|r| MetadataEntry {
                key: r[0].get_str().unwrap().to_string(),
                value: r[1].get_str().unwrap().to_string(),
            })
            .collect();

        let activity_script = format!(
            "?[id, timestamp, author, message] :=
                *activity{{id, ticket_id, timestamp, author, message}},
                ticket_id = '{eid}'
            :order timestamp",
        );
        let activity_result = self.run(&activity_script)?;
        let activities = activity_result
            .rows
            .iter()
            .map(|r| ActivityEntry {
                id: r[0].get_str().unwrap().to_string(),
                timestamp: r[1].get_str().unwrap().to_string(),
                author: r[2].get_str().unwrap().to_string(),
                message: r[3].get_str().unwrap().to_string(),
            })
            .collect();

        Ok(TicketDetail {
            ticket,
            metadata,
            activities,
        })
    }

    /// Partially update a ticket's core fields.
    ///
    /// Only the fields set in `UpdateTicketFields` are changed; others remain as-is.
    /// Returns an error if the ticket is not found.
    pub fn update_ticket(&self, id: &str, fields: &UpdateTicketFields) -> Result<(), String> {
        let eid = escape_cozo(id);

        let existing_script = format!(
            "?[id, type, status, priority, parent_id, title, body, created_at, updated_at] :=
                *ticket{{id, type, status, priority, parent_id, title, body, created_at, updated_at}},
                id = '{eid}'",
        );
        let result = self.run(&existing_script)?;
        if result.rows.is_empty() {
            return Err(format!("Ticket not found: {id}"));
        }
        let r = &result.rows[0];

        let ticket_type = r[1].get_str().unwrap();
        let status = fields
            .status
            .as_deref()
            .unwrap_or_else(|| r[2].get_str().unwrap());
        let priority = fields.priority.unwrap_or_else(|| r[3].get_int().unwrap());
        let parent_id = r[4].get_str().unwrap();
        let title = fields
            .title
            .as_deref()
            .unwrap_or_else(|| r[5].get_str().unwrap());
        let body = fields
            .body
            .as_deref()
            .unwrap_or_else(|| r[6].get_str().unwrap());
        let created_at = r[7].get_str().unwrap();
        let now = now_utc();

        let update_script = format!(
            "?[id, type, status, priority, parent_id, title, body, created_at, updated_at] <- [[
                '{eid}',
                '{ticket_type}',
                '{status}',
                {priority},
                '{parent_id}',
                '{title}',
                '{body}',
                '{created_at}',
                '{now}'
            ]]
            :put ticket {{id => type, status, priority, parent_id, title, body, created_at, updated_at}}",
            ticket_type = escape_cozo(ticket_type),
            status = escape_cozo(status),
            parent_id = escape_cozo(parent_id),
            title = escape_cozo(title),
            body = escape_cozo(body),
            created_at = escape_cozo(created_at),
            now = escape_cozo(&now),
        );

        self.run(&update_script)?;
        Ok(())
    }

    /// Set a metadata key-value pair on a ticket.
    ///
    /// If the key already exists, its value is updated. Returns an error if the
    /// ticket does not exist.
    pub fn set_meta(&self, ticket_id: &str, key: &str, value: &str) -> Result<(), String> {
        let etid = escape_cozo(ticket_id);

        let check = format!("?[id] := *ticket{{id}}, id = '{etid}'");
        let result = self.run(&check)?;
        if result.rows.is_empty() {
            return Err(format!("Ticket not found: {ticket_id}"));
        }

        let script = format!(
            "?[ticket_id, key, value] <- [['{etid}', '{k}', '{v}']]
            :put ticket_meta {{ticket_id, key => value}}",
            k = escape_cozo(key),
            v = escape_cozo(value),
        );
        self.run(&script)?;
        Ok(())
    }

    /// Delete a metadata key from a ticket.
    ///
    /// Returns an error if the ticket does not exist. Silently succeeds if the
    /// key was not present.
    pub fn delete_meta(&self, ticket_id: &str, key: &str) -> Result<(), String> {
        let etid = escape_cozo(ticket_id);

        let check = format!("?[id] := *ticket{{id}}, id = '{etid}'");
        let result = self.run(&check)?;
        if result.rows.is_empty() {
            return Err(format!("Ticket not found: {ticket_id}"));
        }

        let script = format!(
            "?[ticket_id, key] <- [['{etid}', '{k}']]
            :rm ticket_meta {{ticket_id, key}}",
            k = escape_cozo(key),
        );
        self.run(&script)?;
        Ok(())
    }
}
