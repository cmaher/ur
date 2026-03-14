use crate::database::DatabaseManager;

#[tokio::test]
async fn open_creates_database_and_runs_migrations() {
    let db_path = format!("/tmp/ur_db_test_{}.db", uuid::Uuid::new_v4());
    let manager = DatabaseManager::open(&db_path)
        .await
        .expect("should open database");

    // Verify tables exist by querying sqlite_master
    let tables: Vec<(String,)> = sqlx::query_as(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE '_sqlx%' ORDER BY name",
    )
    .fetch_all(manager.pool())
    .await
    .expect("should query tables");

    let table_names: Vec<&str> = tables.iter().map(|t| t.0.as_str()).collect();
    assert!(table_names.contains(&"ticket"), "missing ticket table");
    assert!(table_names.contains(&"edge"), "missing edge table");
    assert!(table_names.contains(&"meta"), "missing meta table");
    assert!(table_names.contains(&"activity"), "missing activity table");

    // Verify foreign keys are enabled
    let fk: (i32,) = sqlx::query_as("PRAGMA foreign_keys")
        .fetch_one(manager.pool())
        .await
        .expect("should query foreign_keys pragma");
    assert_eq!(fk.0, 1, "foreign_keys should be ON");

    // Cleanup
    drop(manager);
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(format!("{db_path}-shm"));
    let _ = std::fs::remove_file(format!("{db_path}-wal"));
}
