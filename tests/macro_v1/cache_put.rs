use accelerator::CacheError;

use crate::support::{CachePutHarness, MacroTestError, User, UserService};

#[tokio::test]
async fn cache_put_writes_value_after_business_success() {
    let service = UserService::new();
    let user = User {
        id: 9,
        name: "nina".to_string(),
    };

    service.save_user(user.clone()).await.unwrap();

    let value = service.cache_only_get(9).await;

    assert_eq!(value, Some(user));
    assert_eq!(service.save_count(), 1);
}

#[tokio::test]
async fn cache_put_ignore_on_set_error_keeps_business_success() {
    let harness = CachePutHarness::new();
    let user = User {
        id: 31,
        name: "ignore-put".to_string(),
    };
    harness.cache.set_fail_set(true);

    harness.put_ignore(user).await.unwrap();

    assert_eq!(harness.runs_ignore(), 1);
    assert_eq!(harness.cache.set_calls(), 1);
}

#[tokio::test]
async fn cache_put_propagate_on_set_error_returns_cache_error() {
    let harness = CachePutHarness::new();
    let user = User {
        id: 32,
        name: "propagate-put".to_string(),
    };
    harness.cache.set_fail_set(true);

    let err = harness.put_propagate(user).await.unwrap_err();

    assert!(matches!(
        err,
        MacroTestError::Cache(CacheError::Backend(ref msg)) if msg.contains("mock set failed")
    ));
    assert_eq!(harness.runs_propagate(), 1);
    assert_eq!(harness.cache.set_calls(), 1);
}
