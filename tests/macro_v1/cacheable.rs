use accelerator::CacheError;

use crate::support::{CacheableHarness, MacroTestError, User, UserService};

#[tokio::test]
async fn cacheable_reuses_cached_value_on_second_call() {
    let service = UserService::new();

    let first = service.query_user(7).await.unwrap();
    let second = service.query_user(7).await.unwrap();

    assert_eq!(
        first,
        Some(User {
            id: 7,
            name: "user-7".to_string()
        })
    );
    assert_eq!(second, first);
    assert_eq!(service.query_count(), 1);
}

#[tokio::test]
async fn cacheable_ignore_on_get_error_keeps_business_flow() {
    let harness = CacheableHarness::new();
    harness.cache.set_fail_get(true);

    let first = harness.get_ignore(21).await.unwrap();
    assert_eq!(
        first,
        Some(User {
            id: 21,
            name: "ignore-21".to_string(),
        })
    );
    assert_eq!(harness.hits_ignore(), 1);
    assert_eq!(harness.cache.get_calls(), 1);
    assert_eq!(harness.cache.set_calls(), 1);

    harness.cache.set_fail_get(false);
    let second = harness.get_ignore(21).await.unwrap();
    assert_eq!(second, first);
    assert_eq!(harness.hits_ignore(), 1);
}

#[tokio::test]
async fn cacheable_propagate_on_get_error_short_circuits_business() {
    let harness = CacheableHarness::new();
    harness.cache.set_fail_get(true);

    let err = harness.get_propagate(22).await.unwrap_err();
    assert!(matches!(
        err,
        MacroTestError::Cache(CacheError::Backend(ref msg)) if msg.contains("mock get failed")
    ));
    assert_eq!(harness.hits_propagate(), 0);
    assert_eq!(harness.cache.get_calls(), 1);
}

#[tokio::test]
async fn cacheable_propagate_on_set_error_returns_cache_error() {
    let harness = CacheableHarness::new();
    harness.cache.set_fail_set(true);

    let err = harness.get_propagate(23).await.unwrap_err();
    assert!(matches!(
        err,
        MacroTestError::Cache(CacheError::Backend(ref msg)) if msg.contains("mock set failed")
    ));
    assert_eq!(harness.hits_propagate(), 1);
    assert_eq!(harness.cache.get_calls(), 1);
    assert_eq!(harness.cache.set_calls(), 1);
}

#[tokio::test]
async fn cacheable_cache_none_true_writes_null_value() {
    let harness = CacheableHarness::new();

    let result = harness.get_none_with_cache(24).await.unwrap();
    assert_eq!(result, None);
    assert_eq!(harness.hits_none_true(), 1);
    assert_eq!(harness.cache.set_calls(), 1);
}

#[tokio::test]
async fn cacheable_cache_none_false_skips_null_write() {
    let harness = CacheableHarness::new();

    let result = harness.get_none_without_cache(25).await.unwrap();
    assert_eq!(result, None);
    assert_eq!(harness.hits_none_false(), 1);
    assert_eq!(harness.cache.set_calls(), 0);
}
