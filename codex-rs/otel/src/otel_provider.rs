use crate::config::OtelExporter;
use crate::config::OtelHttpProtocol;
use crate::config::OtelSampler;
use crate::config::OtelSettings;
use crate::file_exporter::FileExporter;
use crate::file_exporter::create_log_file;
use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry::propagation::TextMapPropagator;
use opentelemetry::trace::TracerProvider;
use opentelemetry_http::HeaderInjector;
use opentelemetry_otlp::Protocol;
use opentelemetry_otlp::SpanExporter;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_otlp::WithHttpConfig;
use opentelemetry_otlp::WithTonicConfig;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::Sampler;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::trace::Tracer;
use opentelemetry_semantic_conventions as semconv;
use reqwest::header::HeaderMap;
use tonic::metadata::Ascii;
use tonic::metadata::MetadataKey;
use tonic::metadata::MetadataMap;
use tonic::metadata::MetadataValue;
use tracing::Span;
use tracing::debug;
use tracing_opentelemetry::OpenTelemetrySpanExt;

const ENV_ATTRIBUTE: &str = "env";

pub struct OtelProvider {
    pub name: String,
    pub provider: SdkTracerProvider,
}

impl OtelProvider {
    pub fn tracer(&self) -> Tracer {
        self.provider.tracer(self.name.clone())
    }

    pub fn shutdown(&self) {
        let _ = self.provider.shutdown();
    }

    pub fn headers(span: &Span) -> HeaderMap {
        let mut injector = HeaderMap::new();
        TraceContextPropagator::default()
            .inject_context(&span.context(), &mut HeaderInjector(&mut injector));
        injector
    }

    pub fn from(settings: &OtelSettings) -> Option<Self> {
        if !settings.enabled {
            return None;
        }

        let sampler = match settings.sampler {
            OtelSampler::AlwaysOn => Sampler::AlwaysOn,
            OtelSampler::TraceIdRatioBased(ratio) => Sampler::TraceIdRatioBased(ratio),
        };

        let resource = Resource::builder()
            .with_service_name(settings.service_name.clone())
            .with_attributes(vec![
                KeyValue::new(
                    semconv::attribute::SERVICE_VERSION,
                    settings.service_version.clone(),
                ),
                KeyValue::new(ENV_ATTRIBUTE, settings.environment.clone()),
            ])
            .build();

        let mut builder = SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .with_sampler(sampler);

        match &settings.exporter {
            OtelExporter::None => {
                debug!("No exporter enabled in OTLP settings.");
            }
            OtelExporter::OtlpFile => {
                let (log_file, log_path) =
                    create_log_file(settings).expect("Could not create trace log file.");

                debug!("Using OTLP File exporter: {}", log_path.display());

                let exporter = FileExporter::new(log_file, resource);
                builder = builder.with_batch_exporter(exporter);
            }
            OtelExporter::OtlpGrpc { endpoint, headers } => {
                debug!("Using OTLP Grpc exporter: {}", endpoint);

                let mut metadata = MetadataMap::new();
                for (k, v) in headers {
                    let key = k.parse::<MetadataKey<Ascii>>().unwrap();
                    let value = v.parse::<MetadataValue<Ascii>>().unwrap();
                    metadata.insert(key, value);
                }

                let exporter = SpanExporter::builder()
                    .with_tonic()
                    .with_endpoint(endpoint)
                    .with_metadata(metadata)
                    .build()
                    .expect("Could not create trace exporter.");

                builder = builder.with_batch_exporter(exporter);
            }
            OtelExporter::OtlpHttp {
                endpoint,
                headers,
                protocol,
            } => {
                debug!("Using OTLP Http exporter: {}", endpoint);

                let protocol = match protocol {
                    OtelHttpProtocol::Binary => Protocol::HttpBinary,
                    OtelHttpProtocol::Json => Protocol::HttpJson,
                };

                let exporter = SpanExporter::builder()
                    .with_http()
                    .with_endpoint(endpoint)
                    .with_protocol(protocol)
                    .with_headers(headers.clone())
                    .build()
                    .expect("Could not create trace exporter.");

                builder = builder.with_batch_exporter(exporter);
            }
        }

        let provider = builder.build();

        global::set_tracer_provider(provider.clone());

        Some(Self {
            name: settings.service_name.clone(),
            provider,
        })
    }
}

impl Drop for OtelProvider {
    fn drop(&mut self) {
        let _ = self.provider.shutdown();
    }
}
