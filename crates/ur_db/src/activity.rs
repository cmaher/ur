use std::collections::HashMap;

use crate::DatabaseManager;
use crate::ticket::{ActivityEntry, escape_cozo};
use rand::Rng;

/// A single key-value metadata entry for an activity.
#[derive(Debug, Clone)]
pub struct ActivityMetadataEntry {
    pub key: String,
    pub value: String,
}

/// An activity log entry together with its metadata.
#[derive(Debug, Clone)]
pub struct ActivityDetail {
    pub entry: ActivityEntry,
    pub metadata: Vec<ActivityMetadataEntry>,
}

/// Generate a unique activity ID (8 random alphanumeric characters).
fn generate_activity_id() -> String {
    let mut rng = rand::thread_rng();
    (0..8)
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

/// Get the current UTC timestamp in ISO 8601 format with microsecond precision.
///
/// Microsecond precision ensures activities created in rapid succession
/// receive distinct, correctly-ordered timestamps.
fn now_utc_millis() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let micros = duration.subsec_micros();
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{micros:06}Z")
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
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

impl DatabaseManager {
    /// Add an activity log entry to a ticket.
    ///
    /// Generates a unique activity ID and records the current UTC timestamp.
    /// Optionally attaches key-value metadata to the activity via the
    /// `activity_meta` relation.
    ///
    /// Returns the generated activity ID. Returns an error if the ticket
    /// does not exist.
    pub fn add_activity(
        &self,
        ticket_id: &str,
        author: &str,
        message: &str,
        meta: &HashMap<String, String>,
    ) -> Result<String, String> {
        let etid = escape_cozo(ticket_id);

        // Verify ticket exists
        let check = format!("?[id] := *ticket{{id}}, id = '{etid}'");
        let result = self.run(&check)?;
        if result.rows.is_empty() {
            return Err(format!("Ticket not found: {ticket_id}"));
        }

        // Generate unique activity ID with collision retry
        let activity_id = loop {
            let candidate = generate_activity_id();
            let id_check = format!(
                "?[id] := *activity{{id}}, id = '{cid}'",
                cid = escape_cozo(&candidate)
            );
            let id_result = self.run(&id_check)?;
            if id_result.rows.is_empty() {
                break candidate;
            }
        };

        let timestamp = now_utc_millis();
        let eaid = escape_cozo(&activity_id);

        let insert_script = format!(
            "?[id, ticket_id, timestamp, author, message] <- [[
                '{eaid}',
                '{etid}',
                '{ts}',
                '{author}',
                '{message}'
            ]]
            :put activity {{id => ticket_id, timestamp, author, message}}",
            ts = escape_cozo(&timestamp),
            author = escape_cozo(author),
            message = escape_cozo(message),
        );
        self.run(&insert_script)?;

        // Insert metadata entries
        for (key, value) in meta {
            let meta_script = format!(
                "?[activity_id, key, value] <- [['{eaid}', '{k}', '{v}']]
                :put activity_meta {{activity_id, key => value}}",
                k = escape_cozo(key),
                v = escape_cozo(value),
            );
            self.run(&meta_script)?;
        }

        Ok(activity_id)
    }

    /// List all activity log entries for a ticket, ordered by timestamp.
    ///
    /// Each entry includes its metadata (if any). Returns an error if the
    /// ticket does not exist.
    pub fn list_activities(&self, ticket_id: &str) -> Result<Vec<ActivityDetail>, String> {
        let etid = escape_cozo(ticket_id);

        // Verify ticket exists
        let check = format!("?[id] := *ticket{{id}}, id = '{etid}'");
        let result = self.run(&check)?;
        if result.rows.is_empty() {
            return Err(format!("Ticket not found: {ticket_id}"));
        }

        // Fetch activities ordered by timestamp
        let activity_script = format!(
            "?[id, timestamp, author, message] :=
                *activity{{id, ticket_id, timestamp, author, message}},
                ticket_id = '{etid}'
            :order timestamp",
        );
        let activity_result = self.run(&activity_script)?;

        let mut details = Vec::new();
        for row in &activity_result.rows {
            let aid = row[0].get_str().unwrap().to_string();

            // Fetch metadata for this activity
            let meta_script = format!(
                "?[key, value] :=
                    *activity_meta{{activity_id, key, value}},
                    activity_id = '{eaid}'
                :order key",
                eaid = escape_cozo(&aid),
            );
            let meta_result = self.run(&meta_script)?;
            let metadata = meta_result
                .rows
                .iter()
                .map(|r| ActivityMetadataEntry {
                    key: r[0].get_str().unwrap().to_string(),
                    value: r[1].get_str().unwrap().to_string(),
                })
                .collect();

            details.push(ActivityDetail {
                entry: ActivityEntry {
                    id: aid,
                    timestamp: row[1].get_str().unwrap().to_string(),
                    author: row[2].get_str().unwrap().to_string(),
                    message: row[3].get_str().unwrap().to_string(),
                },
                metadata,
            });
        }

        Ok(details)
    }
}
