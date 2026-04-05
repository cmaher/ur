use crate::database::DatabaseManager;

#[tokio::test]
async fn open_creates_database_and_runs_migrations() {
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://ur:ur@localhost:5432/ur_test".to_string());
    let manager = DatabaseManager::open(&db_url)
        .await
        .expect("should open database");

    // Verify tables exist by querying information_schema
    let tables: Vec<(String,)> = sqlx::query_as(
        "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' AND table_name NOT LIKE '_sqlx%' ORDER BY table_name",
    )
    .fetch_all(manager.pool())
    .await
    .expect("should query tables");

    let table_names: Vec<&str> = tables.iter().map(|t| t.0.as_str()).collect();
    assert!(table_names.contains(&"ticket"), "missing ticket table");
    assert!(table_names.contains(&"edge"), "missing edge table");
    assert!(table_names.contains(&"meta"), "missing meta table");
    assert!(table_names.contains(&"activity"), "missing activity table");
}
