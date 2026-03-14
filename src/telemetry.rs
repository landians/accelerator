use std::collections::HashMap;

use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::{SpanExporter, WithExportConfig, WithHttpConfig, WithTonicConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tonic::metadata::{MetadataMap, MetadataValue};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Transport protocol for OTLP trace export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtlpTransport {
    /// OTLP over gRPC.
    Grpc,
    /// OTLP over HTTP/protobuf.
    Http,
}

/// Builder for reusable OTLP tracing setup.
///
/// This component is intended to be embedded into host applications so the
/// cache runtime spans (for example `cache.get`, `cache.mget`) can be exported
/// to ClickStack or any OTLP-compatible backend.
#[derive(Debug, Clone)]
pub struct OtlpTelemetryBuilder {
    service_name: String,
    service_namespace: Option<String>,
    service_version: Option<String>,
    endpoint: String,
    transport: OtlpTransport,
    env_filter: String,
    tracer_name: String,
    authorization: Option<String>,
}

impl OtlpTelemetryBuilder {
    /// Creates a builder with sane defaults for local ClickStack development.
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            service_namespace: None,
            service_version: None,
            endpoint: "http://127.0.0.1:4317".to_string(),
            transport: OtlpTransport::Grpc,
            env_filter: "info".to_string(),
            tracer_name: "accelerator".to_string(),
            authorization: None,
        }
    }

    /// Sets OTLP collector endpoint (for example `http://127.0.0.1:4317`).
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    /// Sets OTLP transport protocol.
    pub fn transport(mut self, transport: OtlpTransport) -> Self {
        self.transport = transport;
        self
    }

    /// Sets service namespace resource attribute.
    pub fn service_namespace(mut self, service_namespace: impl Into<String>) -> Self {
        self.service_namespace = Some(service_namespace.into());
        self
    }

    /// Sets service version resource attribute.
    pub fn service_version(mut self, service_version: impl Into<String>) -> Self {
        self.service_version = Some(service_version.into());
        self
    }

    /// Sets `tracing_subscriber` env filter expression.
    pub fn env_filter(mut self, env_filter: impl Into<String>) -> Self {
        self.env_filter = env_filter.into();
        self
    }

    /// Sets tracer name used by OpenTelemetry tracer provider.
    pub fn tracer_name(mut self, tracer_name: impl Into<String>) -> Self {
        self.tracer_name = tracer_name.into();
        self
    }

    /// Sets explicit OTLP authorization header value.
    ///
    /// Examples:
    /// - `authorization("your-ingestion-key")`
    /// - `authorization("Bearer your-token")`
    pub fn authorization(mut self, authorization: impl Into<String>) -> Self {
        self.authorization = Some(authorization.into());
        self
    }

    /// Installs tracing + OTLP exporter and returns a shutdown guard.
    ///
    /// Host app should keep the returned guard alive for the full process
    /// lifecycle, and call `shutdown` on graceful exit.
    pub fn install(self) -> Result<TelemetryGuard, TelemetryInitError> {
        let mut attrs = vec![KeyValue::new("service.name", self.service_name)];
        if let Some(ns) = self.service_namespace {
            attrs.push(KeyValue::new("service.namespace", ns));
        }
        if let Some(version) = self.service_version {
            attrs.push(KeyValue::new("service.version", version));
        }

        let resource = Resource::builder().with_attributes(attrs).build();
        let exporter = build_exporter(self.transport, &self.endpoint, self.authorization)?;

        let provider = SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_resource(resource)
            .build();
        let tracer = provider.tracer(self.tracer_name);

        global::set_tracer_provider(provider.clone());
        tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::new(self.env_filter))
            .with(tracing_subscriber::fmt::layer())
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .try_init()
            .map_err(|err| TelemetryInitError::SubscriberInit(err.to_string()))?;

        Ok(TelemetryGuard {
            provider: Some(provider),
        })
    }
}

/// Guard object that owns tracer provider lifecycle.
pub struct TelemetryGuard {
    provider: Option<SdkTracerProvider>,
}

impl TelemetryGuard {
    /// Flushes and shuts down OTLP export pipeline.
    pub fn shutdown(mut self) {
        if let Some(provider) = self.provider.take() {
            let _ = provider.shutdown();
        }
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            let _ = provider.shutdown();
        }
    }
}

fn build_exporter(
    transport: OtlpTransport,
    endpoint: &str,
    authorization: Option<String>,
) -> Result<SpanExporter, TelemetryInitError> {
    match transport {
        OtlpTransport::Grpc => {
            let mut builder = SpanExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint.to_string());
            if let Some(auth) = authorization {
                let mut metadata = MetadataMap::new();
                let value = MetadataValue::try_from(auth.as_str())
                    .map_err(|err| TelemetryInitError::InvalidAuthorization(err.to_string()))?;
                metadata.insert("authorization", value);
                builder = builder.with_metadata(metadata);
            }
            builder
                .build()
                .map_err(|err| TelemetryInitError::ExporterBuild(err.to_string()))
        }
        OtlpTransport::Http => {
            let mut builder = SpanExporter::builder()
                .with_http()
                .with_endpoint(normalize_http_trace_endpoint(endpoint));
            if let Some(auth) = authorization {
                let mut headers = HashMap::new();
                headers.insert("authorization".to_string(), auth);
                builder = builder.with_headers(headers);
            }
            builder
                .build()
                .map_err(|err| TelemetryInitError::ExporterBuild(err.to_string()))
        }
    }
}

fn normalize_http_trace_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim_end_matches('/');
    if trimmed.ends_with("/v1/traces") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1/traces")
    }
}

/// Error type for OTLP telemetry bootstrap.
#[derive(Debug, thiserror::Error)]
pub enum TelemetryInitError {
    /// Failed to create OTLP exporter.
    #[error("failed to build otlp exporter: {0}")]
    ExporterBuild(String),
    /// Failed to install tracing subscriber.
    #[error("failed to initialize tracing subscriber: {0}")]
    SubscriberInit(String),
    /// Authorization header value is invalid for OTLP metadata.
    #[error("invalid authorization header value: {0}")]
    InvalidAuthorization(String),
}
