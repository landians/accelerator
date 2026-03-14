use crate::cache::CacheMetricsSnapshot;

/// One metric sample with a stable metric name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricPoint {
    /// Metric name exported by this crate.
    pub name: &'static str,
    /// Current counter value.
    pub value: u64,
}

/// OpenTelemetry-friendly metric point model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtelMetricPoint {
    /// Metric name exported by this crate.
    pub name: &'static str,
    /// Current counter value.
    pub value: u64,
    /// Attribute key/value pairs attached to the point.
    pub attributes: Vec<(&'static str, String)>,
}

/// Returns stable metric points for exporter adapters.
pub fn metric_points(snapshot: &CacheMetricsSnapshot) -> Vec<MetricPoint> {
    vec![
        MetricPoint {
            name: "accelerator_cache_local_hit_total",
            value: snapshot.local_hit,
        },
        MetricPoint {
            name: "accelerator_cache_local_miss_total",
            value: snapshot.local_miss,
        },
        MetricPoint {
            name: "accelerator_cache_remote_hit_total",
            value: snapshot.remote_hit,
        },
        MetricPoint {
            name: "accelerator_cache_remote_miss_total",
            value: snapshot.remote_miss,
        },
        MetricPoint {
            name: "accelerator_cache_load_total",
            value: snapshot.load_total,
        },
        MetricPoint {
            name: "accelerator_cache_load_success_total",
            value: snapshot.load_success,
        },
        MetricPoint {
            name: "accelerator_cache_load_timeout_total",
            value: snapshot.load_timeout,
        },
        MetricPoint {
            name: "accelerator_cache_load_error_total",
            value: snapshot.load_error,
        },
        MetricPoint {
            name: "accelerator_cache_stale_fallback_total",
            value: snapshot.stale_fallback,
        },
        MetricPoint {
            name: "accelerator_cache_refresh_attempts_total",
            value: snapshot.refresh_attempts,
        },
        MetricPoint {
            name: "accelerator_cache_refresh_success_total",
            value: snapshot.refresh_success,
        },
        MetricPoint {
            name: "accelerator_cache_refresh_failures_total",
            value: snapshot.refresh_failures,
        },
        MetricPoint {
            name: "accelerator_cache_invalidation_publish_total",
            value: snapshot.invalidation_publish,
        },
        MetricPoint {
            name: "accelerator_cache_invalidation_publish_failures_total",
            value: snapshot.invalidation_publish_failures,
        },
        MetricPoint {
            name: "accelerator_cache_invalidation_receive_total",
            value: snapshot.invalidation_receive,
        },
        MetricPoint {
            name: "accelerator_cache_invalidation_receive_failures_total",
            value: snapshot.invalidation_receive_failures,
        },
    ]
}

/// Converts metrics snapshot into OpenTelemetry-friendly points.
pub fn to_otel_points(area: &str, snapshot: &CacheMetricsSnapshot) -> Vec<OtelMetricPoint> {
    metric_points(snapshot)
        .into_iter()
        .map(|point| OtelMetricPoint {
            name: point.name,
            value: point.value,
            attributes: vec![("area", area.to_string())],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::cache::CacheMetricsSnapshot;

    use super::{metric_points, to_otel_points};

    #[test]
    fn metric_points_returns_stable_metric_set() {
        let snapshot = CacheMetricsSnapshot {
            local_hit: 1,
            local_miss: 2,
            remote_hit: 3,
            remote_miss: 4,
            load_total: 5,
            load_success: 6,
            load_timeout: 7,
            load_error: 8,
            stale_fallback: 9,
            refresh_attempts: 10,
            refresh_success: 11,
            refresh_failures: 12,
            invalidation_publish: 13,
            invalidation_publish_failures: 14,
            invalidation_receive: 15,
            invalidation_receive_failures: 16,
        };

        let points = metric_points(&snapshot);

        assert_eq!(points.len(), 16);
        assert_eq!(points[0].name, "accelerator_cache_local_hit_total");
        assert_eq!(points[0].value, 1);
        assert_eq!(
            points[15].name,
            "accelerator_cache_invalidation_receive_failures_total"
        );
        assert_eq!(points[15].value, 16);
    }

    #[test]
    fn to_otel_points_keeps_metric_name_value_and_area_attribute() {
        let snapshot = CacheMetricsSnapshot {
            load_error: 3,
            ..CacheMetricsSnapshot::default()
        };

        let points = to_otel_points("area-a", &snapshot);

        let load_error = points
            .iter()
            .find(|point| point.name == "accelerator_cache_load_error_total")
            .unwrap();

        assert_eq!(load_error.value, 3);
        assert_eq!(load_error.attributes, vec![("area", "area-a".to_string())]);
    }
}
