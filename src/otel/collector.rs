use crate::collect;
use crate::{Error, atomic, dto};

use crate::observe::{MetricSource, Mode, SyncObserver};
use crate::otel::ProtoSender;
use crate::otel::timer::{CollectorTime, GridTimer};
use log::{error, warn};
use opentelemetry_proto::tonic::metrics::v1 as proto;
use std::hash::BuildHasher;
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::mpsc as tmpsc;

type BoxedOtelProtoObserver = collect::BoxedDynObserver<proto::Metric>;

/// Configuration for a [`Collector`].
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Config {
    /// Call [`DynObserver::observe`] on all observers each `poll_period` duration.
    pub poll_period: Duration,

    /// Call [`DynObserver::export`] on accumulated metrics each `export_period` duration.
    pub export_period: Duration,

    /// Use the observers' start time and `seq_id` to generate perfect timestamps (i.e., for data
    /// compressibility).
    pub align_timestamps: bool,
}

#[derive(Clone, Debug)]
pub struct Collector {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    cmd_tx: mpsc::Sender<CollectorCmd>,
    handle: Option<JoinHandle<()>>,
    sender_task: tokio::task::JoinHandle<()>,
}

enum CollectorCmd {
    Add(BoxedOtelProtoObserver),
    Shutdown,
}

impl std::fmt::Debug for CollectorCmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Add(_) => f.debug_tuple("Add").finish(),
            Self::Shutdown => f.write_str("Shutdown"),
        }
    }
}

impl Collector {
    /// - `sender` - sender implementation used to send accumulated metric batches.
    pub fn new<S>(sender: S, config: Config) -> Result<Self, Error>
    where
        S: ProtoSender,
    {
        let Config {
            poll_period,
            export_period,
            align_timestamps,
        } = config;

        if poll_period.is_zero() {
            return Err(Error::ZeroPollPeriod);
        }
        if export_period.is_zero() {
            return Err(Error::ZeroExportPeriod);
        }
        if poll_period > export_period {
            return Err(Error::PollPeriodExceedsExportPeriod);
        }

        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (batch_tx, batch_rx) = tmpsc::unbounded_channel();

        let sender_task = tokio::spawn(process_send(sender, batch_rx));

        let start = CollectorTime::now();
        let handle = std::thread::spawn(move || {
            Worker {
                cmd_rx,
                batch_tx,
                observers: Vec::new(),
                start,
                observe_timer: GridTimer::new(start.instant, poll_period),
                export_timer: GridTimer::new(start.instant, export_period),
                poll_period,
                align_timestamps,
            }
            .block();
        });

        Ok(Self {
            inner: Arc::new(Inner {
                cmd_tx,
                handle: Some(handle),
                sender_task,
            }),
        })
    }

    pub fn add<Src, T, S, A>(&self, source: Src, mode: Mode) -> Result<Src, Error>
    where
        Src: MetricSource<Measure = T, Hasher = S, Cell = A>,
        T: atomic::Measure + Send + Sync + 'static,
        S: BuildHasher + Clone + Send + Sync + 'static,
        A: atomic::Record<T>,
        dto::Series<A::Snapshot, S>: dto::IntoWire<proto::Metric, Error = Error>,
    {
        let observer = SyncObserver::new(&source, mode)?;
        self.add_observer(observer);
        Ok(source)
    }

    pub fn add_observer<T, S, A>(&self, observer: SyncObserver<T, S, A>)
    where
        T: atomic::Measure + Send + Sync + 'static,
        S: BuildHasher + Clone + Send + Sync + 'static,
        A: atomic::Record<T>,
        dto::Series<A::Snapshot, S>: dto::IntoWire<proto::Metric, Error = Error>,
    {
        self.add_boxed_observer(Box::new(observer));
    }

    pub fn add_boxed_observer(&self, observer: BoxedOtelProtoObserver) {
        self.inner
            .cmd_tx
            .send(CollectorCmd::Add(observer))
            .expect("collector worker stopped");
    }
}

impl collect::Collector for Collector {
    type Wire = proto::Metric;

    fn register(&self, observer: BoxedOtelProtoObserver) {
        self.add_boxed_observer(observer);
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(CollectorCmd::Shutdown);

        if let Some(handle) = self.handle.take()
            && handle.join().is_err()
        {
            error!("collector worker thread panicked");
        }

        self.sender_task.abort();
    }
}

struct Worker {
    cmd_rx: mpsc::Receiver<CollectorCmd>,
    batch_tx: tmpsc::UnboundedSender<Vec<proto::Metric>>,
    observers: Vec<BoxedOtelProtoObserver>,
    start: CollectorTime,
    observe_timer: GridTimer,
    export_timer: GridTimer,
    poll_period: Duration,
    align_timestamps: bool,
}

impl Worker {
    fn block(&mut self) {
        loop {
            let now = Instant::now();
            self.maybe_observe(now);
            self.maybe_export(now);

            let now = Instant::now();
            let deadline = self
                .observe_timer
                .deadline()
                .min(self.export_timer.deadline());
            let timeout = deadline.saturating_duration_since(now);

            let mut cmd = match self.cmd_rx.recv_timeout(timeout) {
                Ok(cmd) => Some(cmd),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            };

            while let Some(next) = cmd.take().or_else(|| self.cmd_rx.try_recv().ok()) {
                if !self.handle(next) {
                    return;
                }
            }
        }
    }

    fn maybe_observe(&mut self, now: Instant) {
        if !self.observe_timer.due(now) {
            return;
        }

        let ts = self.start.at(now).system;
        for observer in &mut self.observers {
            observer.observe(ts);
        }
        self.observe_timer.advance(Instant::now());
    }

    fn maybe_export(&mut self, now: Instant) {
        if !self.export_timer.due(now) {
            return;
        }
        let align = self.align_timestamps.then_some(self.poll_period);
        let mut batch = Vec::with_capacity(self.observers.len());

        for observer in &mut self.observers {
            match observer.export(align) {
                Ok(Some(metric)) => batch.push(metric),
                Ok(_) => {}
                Err(err) => error!("failed to export metric to OTLP wire format: {err}"),
            }
        }

        if !batch.is_empty() && self.batch_tx.send(batch).is_err() {
            warn!("sender task is gone; discarding metrics batch");
        }

        self.export_timer.advance(Instant::now());
    }

    fn handle(&mut self, cmd: CollectorCmd) -> bool {
        match cmd {
            CollectorCmd::Add(mut observer) => {
                // Reset a newly added observer to the next grid tick.
                let next_tick = self.observe_timer.deadline();
                observer.reset(self.start.at(next_tick).system);
                self.observers.push(observer);
                true
            }
            CollectorCmd::Shutdown => false,
        }
    }
}

async fn process_send<S>(mut sender: S, mut batch_rx: tmpsc::UnboundedReceiver<Vec<proto::Metric>>)
where
    S: ProtoSender,
{
    while let Some(batch) = batch_rx.recv().await {
        if let Err(err) = sender.send(batch).await {
            error!("failed to send metrics batch: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metric::Counter;
    use crate::metric::tests::ID;
    use crate::otel::sender;
    use opentelemetry_proto::tonic::collector::metrics::v1::metrics_service_server::{
        MetricsService, MetricsServiceServer,
    };
    use opentelemetry_proto::tonic::collector::metrics::v1::{
        ExportMetricsServiceRequest, ExportMetricsServiceResponse,
    };
    use testresult::TestResult;
    use tokio::net::TcpListener;
    use tokio_stream::wrappers::TcpListenerStream;
    use tonic::transport::{Endpoint, Server};
    use tonic::{Request, Response, Status};

    struct RecordingSender {
        tx: tmpsc::UnboundedSender<Vec<proto::Metric>>,
    }

    impl ProtoSender for RecordingSender {
        type Error = std::convert::Infallible;

        async fn send(&mut self, metrics: Vec<proto::Metric>) -> Result<(), Self::Error> {
            let _ = self.tx.send(metrics);
            Ok(())
        }
    }

    fn recording_sender() -> (
        RecordingSender,
        tmpsc::UnboundedReceiver<Vec<proto::Metric>>,
    ) {
        let (tx, rx) = tmpsc::unbounded_channel();
        (RecordingSender { tx }, rx)
    }

    #[tokio::test]
    async fn pushes_batches() -> TestResult {
        let (sender, mut rx) = recording_sender();

        let collector = Collector::new(
            sender,
            Config {
                poll_period: Duration::from_millis(10),
                export_period: Duration::from_millis(20),
                align_timestamps: false,
            },
        )?;

        let counter = Counter::<u64>::new(ID);
        counter.add(3, &[]);
        let _ = collector.add(counter, Mode::Direct)?;

        let batch = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await?
            .expect("sender task stopped");

        println!("{:#?}", batch);

        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].name, ID.name.to_string());
        Ok(())
    }

    #[tokio::test]
    async fn no_empty_exports() -> TestResult {
        let (sender, mut rx) = recording_sender();

        let _collector = Collector::new(
            sender,
            Config {
                poll_period: Duration::from_millis(10),
                export_period: Duration::from_millis(20),
                align_timestamps: false,
            },
        )?;

        let got = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(got.is_err(), "empty collector dispatched a batch");
        Ok(())
    }

    #[derive(Clone)]
    struct StubMetricsService {
        tx: tmpsc::UnboundedSender<ExportMetricsServiceRequest>,
    }

    #[tonic::async_trait]
    impl MetricsService for StubMetricsService {
        async fn export(
            &self,
            request: Request<ExportMetricsServiceRequest>,
        ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
            let _ = self.tx.send(request.into_inner());
            Ok(Response::new(ExportMetricsServiceResponse::default()))
        }
    }

    async fn spawn_stub_server() -> TestResult<(
        String,
        tmpsc::UnboundedReceiver<ExportMetricsServiceRequest>,
        tokio::task::JoinHandle<()>,
    )> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        let (tx, rx) = tmpsc::unbounded_channel();
        let service = StubMetricsService { tx };
        let stream = TcpListenerStream::new(listener);

        let handle = tokio::spawn(async move {
            Server::builder()
                .add_service(MetricsServiceServer::new(service))
                .serve_with_incoming(stream)
                .await
                .expect("stub server failed");
        });

        Ok((format!("http://{addr}"), rx, handle))
    }

    #[tokio::test]
    async fn pushes_batches_tonic() -> TestResult {
        let (uri, mut rx, server) = spawn_stub_server().await?;

        let channel = Endpoint::from_shared(uri)?.connect_lazy();

        let collector = Collector::new(
            sender::Tonic::new(channel),
            Config {
                poll_period: Duration::from_millis(10),
                export_period: Duration::from_millis(20),
                align_timestamps: false,
            },
        )?;

        let counter = Counter::<u64>::new(ID);
        counter.add(42, &[]);
        let _ = collector.add(counter, Mode::Direct)?;

        let req = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await?
            .expect("server channel closed");

        let resource_metrics = &req.resource_metrics;
        assert_eq!(resource_metrics.len(), 1);
        assert_eq!(resource_metrics[0].scope_metrics.len(), 1);
        assert_eq!(resource_metrics[0].scope_metrics[0].metrics.len(), 1);
        assert_eq!(
            resource_metrics[0].scope_metrics[0].metrics[0].name,
            ID.name.to_string()
        );

        drop(collector);
        server.abort();
        Ok(())
    }
}
