use accelerator::CacheError;

use crate::support::{CacheEvictBatchHarness, MacroTestError, User};

#[tokio::test]
async fn cache_evict_batch_after_success_deletes_cache() {
    let harness = CacheEvictBatchHarness::new();
    harness.cache.seed(
        1,
        Some(User {
            id: 1,
            name: "x".to_string(),
        }),
    );
    harness.cache.seed(
        2,
        Some(User {
            id: 2,
            name: "y".to_string(),
        }),
    );

    harness.evict_after_ok(vec![1, 2]).await.unwrap();

    assert_eq!(harness.runs_after_ok(), 1);
    assert_eq!(harness.cache.mdel_calls(), 1);
    assert_eq!(harness.cache.read(1), None);
    assert_eq!(harness.cache.read(2), None);
}

#[tokio::test]
async fn cache_evict_batch_after_fail_keeps_cache() {
    let harness = CacheEvictBatchHarness::new();
    harness.cache.seed(
        3,
        Some(User {
            id: 3,
            name: "keep".to_string(),
        }),
    );

    let err = harness.evict_after_fail(vec![3]).await.unwrap_err();

    assert!(matches!(err, MacroTestError::Biz("after-fail")));
    assert_eq!(harness.runs_after_fail(), 1);
    assert_eq!(harness.cache.mdel_calls(), 0);
    assert_eq!(
        harness.cache.read(3),
        Some(User {
            id: 3,
            name: "keep".to_string()
        })
    );
}

#[tokio::test]
async fn cache_evict_batch_before_fail_still_deletes() {
    let harness = CacheEvictBatchHarness::new();
    harness.cache.seed(
        4,
        Some(User {
            id: 4,
            name: "drop".to_string(),
        }),
    );

    let err = harness.evict_before_fail(vec![4]).await.unwrap_err();

    assert!(matches!(err, MacroTestError::Biz("before-fail")));
    assert_eq!(harness.runs_before_fail(), 1);
    assert_eq!(harness.cache.mdel_calls(), 1);
    assert_eq!(harness.cache.read(4), None);
}

#[tokio::test]
async fn cache_evict_batch_ignore_mdel_error_keeps_business_success() {
    let harness = CacheEvictBatchHarness::new();
    harness.cache.set_fail_mdel(true);

    harness.evict_after_ok(vec![8, 9]).await.unwrap();

    assert_eq!(harness.runs_after_ok(), 1);
    assert_eq!(harness.cache.mdel_calls(), 1);
}

#[tokio::test]
async fn cache_evict_batch_propagate_mdel_error_returns_cache_error() {
    let harness = CacheEvictBatchHarness::new();
    harness.cache.set_fail_mdel(true);

    let err = harness.evict_propagate(vec![10, 11]).await.unwrap_err();

    assert!(matches!(
        err,
        MacroTestError::Cache(CacheError::Backend(ref msg)) if msg.contains("mock mdel failed")
    ));
    assert_eq!(harness.runs_propagate(), 1);
    assert_eq!(harness.cache.mdel_calls(), 1);
}

#[tokio::test]
async fn cache_evict_batch_deduplicates_keys_before_mdel() {
    let harness = CacheEvictBatchHarness::new();

    harness.evict_after_ok(vec![5, 5, 5, 6]).await.unwrap();

    let calls = harness.cache.last_mdel_keys();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], vec![5, 6]);
}

#[tokio::test]
async fn cache_evict_batch_empty_keys_skips_mdel_but_runs_business() {
    let harness = CacheEvictBatchHarness::new();

    harness.evict_after_ok(Vec::new()).await.unwrap();

    assert_eq!(harness.runs_after_ok(), 1);
    assert_eq!(harness.cache.mdel_calls(), 0);
}

#[tokio::test]
async fn cache_evict_batch_accepts_slice_keys() {
    let harness = CacheEvictBatchHarness::new();

    harness
        .evict_after_ok_with_slice(&[12, 12, 13])
        .await
        .unwrap();

    let calls = harness.cache.last_mdel_keys();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], vec![12, 13]);
}
