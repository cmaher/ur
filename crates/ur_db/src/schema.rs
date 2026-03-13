/// CozoScript statements that define all six relations in the ticket database.
///
/// Uses `:create` which is a no-op if the relation already exists with the
/// same schema, making it safe to run on every startup.
pub(crate) const RELATION_STATEMENTS: &[&str] = &[
    // ticket: primary entity, keyed by id
    r#":create ticket {
        id: String
        =>
        type: String,
        status: String,
        priority: Int,
        parent_id: String,
        title: String,
        body: String,
        created_at: String,
        updated_at: String
    }"#,
    // ticket_meta: flexible key-value metadata per ticket
    r#":create ticket_meta {
        ticket_id: String,
        key: String
        =>
        value: String
    }"#,
    // blocks: hard dependency edges forming the dispatch DAG
    r#":create blocks {
        blocker_id: String,
        blocked_id: String
    }"#,
    // relates_to: soft informational links between tickets
    r#":create relates_to {
        left_id: String,
        right_id: String
    }"#,
    // activity: timestamped updates on tickets
    r#":create activity {
        id: String
        =>
        ticket_id: String,
        timestamp: String,
        author: String,
        message: String
    }"#,
    // activity_meta: flexible key-value metadata per activity
    r#":create activity_meta {
        activity_id: String,
        key: String
        =>
        value: String
    }"#,
];
