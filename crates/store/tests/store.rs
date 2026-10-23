use sing_portal_store::{
    NewSubscription, NodeReport, Source, Store, SubscriptionPatch,
};
use tempfile::TempDir;

async fn new_store() -> (TempDir, Store) {
    let dir = TempDir::new().unwrap();
    let store = Store::connect(&dir.path().join("portal.db")).await.unwrap();
    (dir, store)
}

fn new_subscription(id: &str, slug: &str, tag: &str) -> NewSubscription {
    NewSubscription {
        id: id.to_string(),
        name: format!("sub {id}"),
        slug: slug.to_string(),
        source: Source::Manual,
        url: None,
        user_agent: String::new(),
        format: "sing-box-json".to_string(),
        content: "{}".to_string(),
        outbounds: vec![serde_json::json!({"type": "vless", "tag": tag})],
        enabled: true,
        auto_refresh: true,
        refresh_interval_secs: 3600,
        next_refresh_at: Some(100),
        validated: true,
        failures: 0,
        now: 10,
    }
}

#[tokio::test]
async fn migrations_are_idempotent() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("portal.db");
    let first = Store::connect(&path).await.unwrap();
    first.healthz().await.unwrap();
    drop(first);

    let second = Store::connect(&path).await.unwrap();
    second.healthz().await.unwrap();
}

#[tokio::test]
async fn subscriptions_roundtrip() {
    let (_dir, store) = new_store().await;
    let created = store
        .create_subscription(&new_subscription("id1", "sub-one", "a"))
        .await
        .unwrap();

    assert_eq!(created.id, "id1");
    assert_eq!(created.slug, "sub-one");
    assert_eq!(created.outbound_tags(), vec!["a".to_string()]);
    assert!(created.last_error.is_empty());

    let listed = store.list_subscriptions().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(store.slugs().await.unwrap(), vec!["sub-one".to_string()]);

    let patched = store
        .update_subscription(
            "id1",
            &SubscriptionPatch {
                name: Some("renamed".into()),
                enabled: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(patched.name, "renamed");
    assert!(!patched.enabled);
    assert_eq!(patched.slug, "sub-one");

    let refreshed = store
        .set_refresh_success(
            "id1",
            "uri",
            "vmess://x",
            &[serde_json::json!({"type": "vmess", "tag": "b"})],
            500,
            Some(900),
            true,
        )
        .await
        .unwrap();
    assert_eq!(refreshed.format, "uri");
    assert_eq!(refreshed.outbound_tags(), vec!["b".to_string()]);
    assert_eq!(refreshed.last_fetched_at, Some(500));
    assert_eq!(refreshed.next_refresh_at, Some(900));

    store.set_refresh_error("id1", "boom", Some(1000)).await.unwrap();
    let after_error = store.get_subscription("id1").await.unwrap().unwrap();
    assert_eq!(after_error.last_error, "boom");
    assert_eq!(after_error.outbound_tags(), vec!["b".to_string()]);

    store.delete_subscription("id1").await.unwrap();
    assert!(store.get_subscription("id1").await.unwrap().is_none());
}

#[tokio::test]
async fn slug_is_unique() {
    let (_dir, store) = new_store().await;
    store
        .create_subscription(&new_subscription("a", "dup", "x"))
        .await
        .unwrap();
    let error = store
        .create_subscription(&new_subscription("b", "dup", "y"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("database"));
}

#[tokio::test]
async fn due_subscriptions_respects_schedule() {
    let (_dir, store) = new_store().await;
    store
        .create_subscription(&new_subscription("a", "a", "x"))
        .await
        .unwrap();

    assert_eq!(store.due_subscriptions(50).await.unwrap().len(), 0);
    assert_eq!(store.due_subscriptions(100).await.unwrap().len(), 1);
    assert_eq!(store.due_subscriptions(200).await.unwrap().len(), 1);
}

#[tokio::test]
async fn selections_and_reports() {
    let (_dir, store) = new_store().await;
    assert_eq!(store.selection("home").await.unwrap(), None);

    store.set_selection("home", "s_auto").await.unwrap();
    store.set_selection("home", "node-1").await.unwrap();
    assert_eq!(
        store.selection("home").await.unwrap(),
        Some("node-1".to_string())
    );

    store.seed_selection("home", "s_direct").await.unwrap();
    assert_eq!(
        store.selection("home").await.unwrap(),
        Some("node-1".to_string())
    );
    store.seed_selection("cola", "s_auto").await.unwrap();
    assert_eq!(store.selections().await.unwrap().len(), 2);

    store
        .record_report(&NodeReport {
            node_name: "home".into(),
            revision: "sha256:1".into(),
            selection: "node-1".into(),
            health: "ok".into(),
            error: String::new(),
            reported_at: 42,
        })
        .await
        .unwrap();
    store
        .record_report(&NodeReport {
            node_name: "home".into(),
            revision: "sha256:2".into(),
            selection: "node-1".into(),
            health: "degraded".into(),
            error: "check failed".into(),
            reported_at: 43,
        })
        .await
        .unwrap();

    let report = store.report("home").await.unwrap().unwrap();
    assert_eq!(report.revision, "sha256:2");
    assert_eq!(report.health, "degraded");
    assert_eq!(store.reports().await.unwrap().len(), 1);
}

#[tokio::test]
async fn audit_log_is_recorded() {
    let (_dir, store) = new_store().await;
    store
        .record_audit("ui", "subscription.create", "sub-one", "format=uri")
        .await
        .unwrap();
    store
        .record_audit("agent:home", "node.report", "home", "revision=sha256:1")
        .await
        .unwrap();

    let entries = store.audit_entries(10).await.unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].actor, "agent:home");
    assert_eq!(entries[1].action, "subscription.create");
}

#[tokio::test]
async fn snapshot_roundtrip() {
    let (_dir, store) = new_store().await;
    store
        .create_subscription(&new_subscription("a", "alpha", "x"))
        .await
        .unwrap();
    store.set_selection("home", "alpha · x").await.unwrap();

    let snapshot = store.snapshot().await.unwrap();
    assert_eq!(snapshot.version, 1);
    assert_eq!(snapshot.subscriptions.len(), 1);
    assert_eq!(snapshot.selections.len(), 1);

    let (_dir2, other) = new_store().await;
    other.import_snapshot(&snapshot).await.unwrap();
    let restored = other.get_subscription_by_slug("alpha").await.unwrap();
    assert!(restored.is_some());
    assert_eq!(
        other.selection("home").await.unwrap(),
        Some("alpha · x".to_string())
    );

    other
        .import_snapshot(&snapshot)
        .await
        .expect("import is idempotent");
    assert_eq!(other.list_subscriptions().await.unwrap().len(), 1);
}
