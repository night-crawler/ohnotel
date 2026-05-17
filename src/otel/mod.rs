pub mod collector;
mod dto;
pub mod sender;
mod timer;
use opentelemetry_proto::tonic::metrics::v1 as proto;

pub trait ProtoSender: Send + 'static {
    type Error: std::fmt::Display + Send;

    fn send(
        &mut self,
        metrics: Vec<proto::Metric>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
