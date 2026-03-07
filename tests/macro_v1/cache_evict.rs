use accelerator::CacheError;

use crate::support::{CacheEvictHarness, MacroTestError, User, UserService};

#[tokio::test]
async fn cache_evict_deletes_cache_after_success() {
    let service = UserService::new();

    service
        .seed_cache(
            11,
            Some(User {
                id: 11,
                name: "to-delete".to_string(),
            }),
        )
        .await;

    service.delete_user(11).await.unwrap();

    let value = service.cache_only_get(11).await;
    assert_eq!(value, None);
    assert_eq!(service.delete_count(), 1);
}

#[tokio::test]
async fn cache_evict_before_runs_even_when_business_fails() {
    let service = UserService::new();

    service
        .seed_cache(
            13,
            Some(User {
                id: 13,
                name: "pre-delete".to_string(),
            }),
        )
        .await;

    let err = service.delete_user_before_and_fail(13).await.unwrap_err();
    assert!(matches!(err, CacheError::Loader(_)));

    let value = service.cache_only_get(13).await;
    assert_eq!(value, None);
}

#[tokio::test]
async fn cache_evict_after_failure_does_not_delete_when_before_false() {
    let harness = CacheEvictHarness::new();
    harness.cache.seed(
        41,
        Some(User {
            id: 41,
            name: "after-fail".to_string(),
        }),
    );

    let err = harness.evict_after_fail(41).await.unwrap_err();
    assert!(matches!(err, MacroTestError::Biz("after-fail")));
    assert_eq!(harness.runs_after_fail(), 1);
    assert_eq!(harness.cache.del_calls(), 0);
    assert_eq!(
        harness.cache.read(41),
        Some(User {
            id: 41,
            name: "after-fail".to_string(),
        })
    );
}

#[tokio::test]
async fn cache_evict_before_true_deletes_even_when_business_fails() {
    let harness = CacheEvictHarness::new();
    harness.cache.seed(
        42,
        Some(User {
            id: 42,
            name: "before-fail".to_string(),
        }),
    );

    let err = harness.evict_before_fail(42).await.unwrap_err();
    assert!(matches!(err, MacroTestError::Biz("before-fail")));
    assert_eq!(harness.runs_before_fail(), 1);
    assert_eq!(harness.cache.del_calls(), 1);
    assert_eq!(harness.cache.read(42), None);
}

#[tokio::test]
async fn cache_evict_ignore_on_del_error_keeps_business_success() {
    let harness = CacheEvictHarness::new();
    harness.cache.set_fail_del(true);

    harness.evict_after_ok(43).await.unwrap();

    assert_eq!(harness.runs_after_ok(), 1);
    assert_eq!(harness.cache.del_calls(), 1);
}

#[tokio::test]
async fn cache_evict_propagate_on_del_error_returns_cache_error() {
    let harness = CacheEvictHarness::new();
    harness.cache.set_fail_del(true);

    let err = harness.evict_propagate(44).await.unwrap_err();

    assert!(matches!(
        err,
        MacroTestError::Cache(CacheError::Backend(ref msg)) if msg.contains("mock del failed")
    ));
    assert_eq!(harness.runs_propagate(), 1);
    assert_eq!(harness.cache.del_calls(), 1);
}
