use crate::Error;
use crate::otel::ProtoSender;
use log::warn;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::metrics_service_client::MetricsServiceClient;
use opentelemetry_proto::tonic::common::v1 as pb;
use opentelemetry_proto::tonic::metrics::v1 as proto;
use opentelemetry_proto::tonic::resource::v1 as res;
use tonic::transport::Channel;

#[derive(Debug)]
pub struct Tonic {
    client: MetricsServiceClient<Channel>,
    resource: Option<res::Resource>,
    scope: Option<pb::InstrumentationScope>,
    schema_url: String,
}

impl Tonic {
    pub fn new(channel: Channel) -> Self {
        Self {
            client: MetricsServiceClient::new(channel),
            resource: None,
            scope: None,
            schema_url: String::new(),
        }
    }

    #[must_use]
    pub fn with_resource(mut self, resource: res::Resource) -> Self {
        self.resource = Some(resource);
        self
    }

    #[must_use]
    pub fn with_scope(mut self, scope: pb::InstrumentationScope) -> Self {
        self.scope = Some(scope);
        self
    }

    #[must_use]
    pub fn with_schema_url(mut self, url: impl Into<String>) -> Self {
        self.schema_url = url.into();
        self
    }

    fn build_request(&self, metrics: Vec<proto::Metric>) -> ExportMetricsServiceRequest {
        ExportMetricsServiceRequest {
            resource_metrics: vec![proto::ResourceMetrics {
                resource: self.resource.clone(),
                scope_metrics: vec![proto::ScopeMetrics {
                    scope: self.scope.clone(),
                    metrics,
                    schema_url: self.schema_url.clone(),
                }],
                schema_url: self.schema_url.clone(),
            }],
        }
    }
}

impl ProtoSender for Tonic {
    type Error = Error;

    async fn send(&mut self, metrics: Vec<proto::Metric>) -> Result<(), Self::Error> {
        let request = self.build_request(metrics);
        let response = self.client.export(request).await?.into_inner();

        let Some(partial_success) = response.partial_success else {
            return Ok(());
        };

        // rejected_data_points > 0 means the collector dropped data.
        // rejected_data_points == 0 with a non-empty error_message is a warning.
        if partial_success.rejected_data_points != 0 {
            return Err(Error::OtlpPartialExport {
                rejected_data_points: partial_success.rejected_data_points,
                error_message: partial_success.error_message,
            });
        }

        if !partial_success.error_message.is_empty() {
            warn!(
                "OTLP collector partial-success warning: {}",
                partial_success.error_message
            );
        }

        Ok(())
    }
}
