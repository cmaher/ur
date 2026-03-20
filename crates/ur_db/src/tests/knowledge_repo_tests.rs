// Tests for KnowledgeRepo.

use crate::knowledge_repo::KnowledgeRepo;
use crate::model::{KnowledgeFilter, KnowledgeUpdate, NewKnowledge};
use crate::tests::TestDb;

fn repo(db: &TestDb) -> KnowledgeRepo {
    KnowledgeRepo::new(db.db().pool().clone())
}

#[tokio::test]
async fn create_and_get() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let doc = r
        .create(&NewKnowledge {
            project: Some("myproj".into()),
            title: "Getting Started".into(),
            description: "How to set up".into(),
            body: "Full body text here.".into(),
            tags: vec!["setup".into(), "onboarding".into()],
        })
        .await
        .unwrap();

    assert_eq!(doc.title, "Getting Started");
    assert_eq!(doc.project.as_deref(), Some("myproj"));
    assert_eq!(doc.tags, vec!["onboarding", "setup"]);

    let fetched = r.get(&doc.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, doc.id);
    assert_eq!(fetched.body, "Full body text here.");
    assert_eq!(fetched.tags, vec!["onboarding", "setup"]);

    db.cleanup().await;
}

#[tokio::test]
async fn create_shared_knowledge() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let doc = r
        .create(&NewKnowledge {
            project: None,
            title: "Shared Doc".into(),
            description: "Shared across projects".into(),
            body: "body".into(),
            tags: vec![],
        })
        .await
        .unwrap();

    assert!(doc.project.is_none());
    assert!(doc.tags.is_empty());

    db.cleanup().await;
}

#[tokio::test]
async fn get_nonexistent_returns_none() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let result = r.get("nonexistent-id").await.unwrap();
    assert!(result.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn update_title_and_body() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let doc = r
        .create(&NewKnowledge {
            title: "Original".into(),
            body: "Original body".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    let updated = r
        .update(
            &doc.id,
            &KnowledgeUpdate {
                title: Some("Updated Title".into()),
                body: Some("Updated body".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(updated.title, "Updated Title");
    assert_eq!(updated.body, "Updated body");
    assert!(updated.updated_at > doc.updated_at);

    db.cleanup().await;
}

#[tokio::test]
async fn update_replaces_tags() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let doc = r
        .create(&NewKnowledge {
            title: "Doc".into(),
            tags: vec!["alpha".into(), "beta".into()],
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(doc.tags, vec!["alpha", "beta"]);

    let updated = r
        .update(
            &doc.id,
            &KnowledgeUpdate {
                tags: Some(vec!["gamma".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(updated.tags, vec!["gamma"]);

    // Verify via fresh get
    let fetched = r.get(&doc.id).await.unwrap().unwrap();
    assert_eq!(fetched.tags, vec!["gamma"]);

    db.cleanup().await;
}

#[tokio::test]
async fn update_without_tags_preserves_existing() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let doc = r
        .create(&NewKnowledge {
            title: "Doc".into(),
            tags: vec!["keep".into()],
            ..Default::default()
        })
        .await
        .unwrap();

    let updated = r
        .update(
            &doc.id,
            &KnowledgeUpdate {
                title: Some("New Title".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(updated.tags, vec!["keep"]);

    db.cleanup().await;
}

#[tokio::test]
async fn update_nonexistent_returns_error() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let result = r
        .update(
            "nonexistent",
            &KnowledgeUpdate {
                title: Some("x".into()),
                ..Default::default()
            },
        )
        .await;

    assert!(result.is_err());

    db.cleanup().await;
}

#[tokio::test]
async fn delete_existing() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let doc = r
        .create(&NewKnowledge {
            title: "To Delete".into(),
            tags: vec!["doomed".into()],
            ..Default::default()
        })
        .await
        .unwrap();

    let deleted = r.delete(&doc.id).await.unwrap();
    assert!(deleted);

    let fetched = r.get(&doc.id).await.unwrap();
    assert!(fetched.is_none());

    db.cleanup().await;
}

#[tokio::test]
async fn delete_nonexistent_returns_false() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let deleted = r.delete("nonexistent").await.unwrap();
    assert!(!deleted);

    db.cleanup().await;
}

#[tokio::test]
async fn list_by_project() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.create(&NewKnowledge {
        project: Some("proj-a".into()),
        title: "A Doc".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    r.create(&NewKnowledge {
        project: Some("proj-b".into()),
        title: "B Doc".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let results = r
        .list(&KnowledgeFilter {
            project: Some("proj-a".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "A Doc");

    db.cleanup().await;
}

#[tokio::test]
async fn list_shared() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.create(&NewKnowledge {
        project: None,
        title: "Shared".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    r.create(&NewKnowledge {
        project: Some("proj".into()),
        title: "Scoped".into(),
        ..Default::default()
    })
    .await
    .unwrap();

    let results = r
        .list(&KnowledgeFilter {
            shared: true,
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Shared");

    db.cleanup().await;
}

#[tokio::test]
async fn list_by_tag() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.create(&NewKnowledge {
        title: "Tagged A".into(),
        tags: vec!["rust".into(), "async".into()],
        ..Default::default()
    })
    .await
    .unwrap();

    r.create(&NewKnowledge {
        title: "Tagged B".into(),
        tags: vec!["rust".into()],
        ..Default::default()
    })
    .await
    .unwrap();

    r.create(&NewKnowledge {
        title: "Tagged C".into(),
        tags: vec!["python".into()],
        ..Default::default()
    })
    .await
    .unwrap();

    let results = r
        .list(&KnowledgeFilter {
            tag: Some("rust".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    // Ordered by title ASC
    assert_eq!(results[0].title, "Tagged A");
    assert_eq!(results[1].title, "Tagged B");

    db.cleanup().await;
}

#[tokio::test]
async fn list_returns_summary_without_body() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.create(&NewKnowledge {
        title: "Doc".into(),
        description: "Short desc".into(),
        body: "This should not appear in list".into(),
        tags: vec!["info".into()],
        ..Default::default()
    })
    .await
    .unwrap();

    let results = r.list(&KnowledgeFilter::default()).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Doc");
    assert_eq!(results[0].description, "Short desc");
    assert_eq!(results[0].tags, vec!["info"]);
    // KnowledgeSummary has no body field — compile-time guarantee

    db.cleanup().await;
}

#[tokio::test]
async fn tags_normalized_lowercase_trimmed_deduplicated() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let doc = r
        .create(&NewKnowledge {
            title: "Tag Test".into(),
            tags: vec![" Rust ".into(), "RUST".into(), "async".into(), "  ".into()],
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(doc.tags, vec!["async", "rust"]);

    db.cleanup().await;
}

#[tokio::test]
async fn description_max_length_enforced() {
    let db = TestDb::new().await;
    let r = repo(&db);

    // Exactly 120 chars should succeed
    let desc_120 = "a".repeat(120);
    let doc = r
        .create(&NewKnowledge {
            title: "OK".into(),
            description: desc_120.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(doc.description, desc_120);

    // 121 chars should fail
    let desc_121 = "a".repeat(121);
    let result = r
        .create(&NewKnowledge {
            title: "Too Long".into(),
            description: desc_121,
            ..Default::default()
        })
        .await;
    assert!(result.is_err());

    db.cleanup().await;
}

#[tokio::test]
async fn description_max_length_enforced_on_update() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let doc = r
        .create(&NewKnowledge {
            title: "Doc".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    let desc_121 = "b".repeat(121);
    let result = r
        .update(
            &doc.id,
            &KnowledgeUpdate {
                description: Some(desc_121),
                ..Default::default()
            },
        )
        .await;
    assert!(result.is_err());

    db.cleanup().await;
}

#[tokio::test]
async fn list_tags_returns_all_unique_tags() {
    let db = TestDb::new().await;
    let r = repo(&db);

    r.create(&NewKnowledge {
        title: "A".into(),
        tags: vec!["rust".into(), "async".into()],
        ..Default::default()
    })
    .await
    .unwrap();

    r.create(&NewKnowledge {
        title: "B".into(),
        tags: vec!["rust".into(), "grpc".into()],
        ..Default::default()
    })
    .await
    .unwrap();

    let tags = r.list_tags().await.unwrap();
    assert_eq!(tags, vec!["async", "grpc", "rust"]);

    db.cleanup().await;
}

#[tokio::test]
async fn list_tags_empty_when_no_docs() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let tags = r.list_tags().await.unwrap();
    assert!(tags.is_empty());

    db.cleanup().await;
}

#[tokio::test]
async fn delete_cascades_tags() {
    let db = TestDb::new().await;
    let r = repo(&db);

    let doc = r
        .create(&NewKnowledge {
            title: "Cascade".into(),
            tags: vec!["orphan".into()],
            ..Default::default()
        })
        .await
        .unwrap();

    r.delete(&doc.id).await.unwrap();

    // The tag should no longer appear
    let tags = r.list_tags().await.unwrap();
    assert!(!tags.contains(&"orphan".to_owned()));

    db.cleanup().await;
}
