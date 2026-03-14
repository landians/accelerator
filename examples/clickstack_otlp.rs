#![allow(clippy::print_stdout)]

#[cfg(feature = "otlp")]
mod app {
    use std::time::Duration;

    use accelerator::builder::LevelCacheBuilder;
    use accelerator::cache::ReadOptions;
    use accelerator::config::CacheMode;
    use accelerator::local;
    use accelerator::telemetry::{OtlpTelemetryBuilder, OtlpTransport};

    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    struct UserProfile {
        id: u64,
        name: String,
    }

    pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
        let otlp_endpoint = std::env::var("ACCELERATOR_CLICKSTACK_OTLP_ENDPOINT")
            .unwrap_or_else(|_| "http://127.0.0.1:4317".to_string());
        let otlp_transport = std::env::var("ACCELERATOR_CLICKSTACK_OTLP_TRANSPORT")
            .ok()
            .map(|v| v.to_lowercase())
            .map(|v| {
                if v == "http" {
                    OtlpTransport::Http
                } else {
                    OtlpTransport::Grpc
                }
            })
            .unwrap_or(OtlpTransport::Grpc);
        let authorization = std::env::var("ACCELERATOR_CLICKSTACK_AUTHORIZATION").ok();

        let mut telemetry = OtlpTelemetryBuilder::new("accelerator-example")
            .service_namespace("accelerator")
            .service_version(env!("CARGO_PKG_VERSION"))
            .endpoint(otlp_endpoint)
            .transport(otlp_transport)
            .env_filter("info,accelerator=info");
        if let Some(authorization) = authorization {
            telemetry = telemetry.authorization(authorization);
        }
        let guard = telemetry.install()?;

        let l1 = local::moka::<UserProfile>().max_capacity(100_000).build()?;

        let cache = LevelCacheBuilder::<u64, UserProfile>::new()
            .area("clickstack_demo")
            .mode(CacheMode::Local)
            .local(l1)
            .local_ttl(Duration::from_secs(60))
            .null_ttl(Duration::from_secs(30))
            .loader_fn(|uid: u64| async move {
                Ok(Some(UserProfile {
                    id: uid,
                    name: format!("user-{uid}"),
                }))
            })
            .build()?;

        // Generate spans and logs for ClickStack trace verification.
        let _ = cache.get(&42, &ReadOptions::default()).await?;
        let _ = cache.get(&42, &ReadOptions::default()).await?;
        let _ = cache.mget(&[42, 43, 44], &ReadOptions::default()).await?;
        cache.del(&42).await?;

        println!(
            "Generated cache traces/logs. Open http://127.0.0.1:8080 and query service.name=accelerator-example."
        );

        guard.shutdown();
        Ok(())
    }
}

#[cfg(feature = "otlp")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run().await
}

#[cfg(not(feature = "otlp"))]
fn main() {
    eprintln!(
        "Enable feature `otlp` to run this example: cargo run --features otlp --example clickstack_otlp"
    );
}
