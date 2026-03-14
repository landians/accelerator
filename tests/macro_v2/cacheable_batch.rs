use accelerator::CacheError;

use crate::support::{CacheableBatchHarness, MacroTestError, User};

#[tokio::test]
async fn cacheable_batch_merges_cache_hit_and_load_miss() {
    let harness = CacheableBatchHarness::new();
    harness.cache.seed(
        1,
        Some(User {
            id: 1,
            name: "cached-1".to_string(),
        }),
    );

    let values = harness.load_users(vec![1, 2, 4]).await.unwrap();

    assert_eq!(
        values.get(&1).cloned().unwrap(),
        Some(User {
            id: 1,
            name: "cached-1".to_string()
        })
    );
    assert_eq!(
        values.get(&2).cloned().unwrap(),
        Some(User {
            id: 2,
            name: "u2".to_string()
        })
    );
    assert_eq!(values.get(&4).cloned().unwrap(), None);
    assert_eq!(harness.load_inputs(), vec![vec![2, 4]]);
    assert_eq!(harness.cache.mget_calls(), 1);
    assert_eq!(harness.cache.mset_calls(), 1);
    assert_eq!(harness.cache.last_mset_sizes(), vec![2]);
}

#[tokio::test]
async fn cacheable_batch_deduplicates_duplicate_keys() {
    let harness = CacheableBatchHarness::new();

    let values = harness.load_users(vec![2, 2, 2, 3, 3]).await.unwrap();

    assert_eq!(values.len(), 2);
    assert_eq!(
        values.get(&2).cloned().unwrap(),
        Some(User {
            id: 2,
            name: "u2".to_string()
        })
    );
    assert_eq!(
        values.get(&3).cloned().unwrap(),
        Some(User {
            id: 3,
            name: "u3".to_string()
        })
    );
    assert_eq!(harness.load_inputs(), vec![vec![2, 3]]);
}

#[tokio::test]
async fn cacheable_batch_empty_input_short_circuits() {
    let harness = CacheableBatchHarness::new();

    let values = harness.load_users(Vec::new()).await.unwrap();

    assert!(values.is_empty());
    assert_eq!(harness.load_inputs().len(), 0);
    assert_eq!(harness.cache.mget_calls(), 0);
    assert_eq!(harness.cache.mset_calls(), 0);
}

#[tokio::test]
async fn cacheable_batch_ignore_mget_error_keeps_business_flow() {
    let harness = CacheableBatchHarness::new();
    harness.cache.set_fail_mget(true);

    let values = harness.load_users(vec![1, 2]).await.unwrap();

    assert_eq!(
        values.get(&1).cloned().unwrap(),
        Some(User {
            id: 1,
            name: "u1".to_string()
        })
    );
    assert_eq!(
        values.get(&2).cloned().unwrap(),
        Some(User {
            id: 2,
            name: "u2".to_string()
        })
    );
    assert_eq!(harness.load_inputs(), vec![vec![1, 2]]);
}

#[tokio::test]
async fn cacheable_batch_propagate_mget_error_returns_cache_error() {
    let harness = CacheableBatchHarness::new();
    harness.cache.set_fail_mget(true);

    let err = harness.load_users_propagate(vec![1, 2]).await.unwrap_err();

    assert!(matches!(
        err,
        MacroTestError::Cache(CacheError::Backend(ref msg)) if msg.contains("mock mget failed")
    ));
    assert_eq!(harness.load_inputs().len(), 0);
}

#[tokio::test]
async fn cacheable_batch_propagate_mset_error_returns_cache_error() {
    let harness = CacheableBatchHarness::new();
    harness.cache.set_fail_mset(true);

    let err = harness.load_users_propagate(vec![2, 3]).await.unwrap_err();

    assert!(matches!(
        err,
        MacroTestError::Cache(CacheError::Backend(ref msg)) if msg.contains("mock mset failed")
    ));
    assert_eq!(harness.load_inputs(), vec![vec![2, 3]]);
    assert_eq!(harness.cache.mset_calls(), 1);
}

#[tokio::test]
async fn cacheable_batch_business_error_is_returned_and_skips_mset() {
    let harness = CacheableBatchHarness::new();

    let err = harness.load_users_fail(vec![1, 2]).await.unwrap_err();

    assert!(matches!(err, MacroTestError::Biz("batch-load-failed")));
    assert_eq!(harness.load_inputs(), vec![vec![1, 2]]);
    assert_eq!(harness.cache.mset_calls(), 0);
}
