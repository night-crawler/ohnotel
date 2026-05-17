use num_traits::ToPrimitive;
use opentelemetry_proto::tonic::common::v1 as pb;
use opentelemetry_proto::tonic::metrics::v1 as proto;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::atomic::Measure;
use crate::atomic::histogram::Snapshot;
use crate::dto::{IntoWire, Kind, Series};
use crate::model::{KeyValue, Value};
use crate::observe::Mode;

impl From<&Value> for pb::any_value::Value {
    fn from(v: &Value) -> Self {
        match v {
            Value::String(s) => pb::any_value::Value::StringValue(s.to_string()),
            &Value::Bool(b) => pb::any_value::Value::BoolValue(b),
            &Value::Int(i) => pb::any_value::Value::IntValue(i),
            Value::Double(d) => pb::any_value::Value::DoubleValue(d.0),
            Value::ArrayAny(arr) => pb::any_value::Value::ArrayValue(pb::ArrayValue {
                values: arr
                    .iter()
                    .map(|v| pb::AnyValue {
                        value: Some(v.into()),
                    })
                    .collect(),
            }),
            Value::ArrayKv(kvs) => pb::any_value::Value::KvlistValue(pb::KeyValueList {
                values: kvs.iter().map(Into::into).collect(),
            }),
            Value::Bytes(b) => pb::any_value::Value::BytesValue(b.clone()),
        }
    }
}

impl From<&Value> for pb::AnyValue {
    fn from(v: &Value) -> Self {
        pb::AnyValue {
            value: Some(v.into()),
        }
    }
}

impl From<&KeyValue> for pb::KeyValue {
    fn from(kv: &KeyValue) -> Self {
        pb::KeyValue {
            key: kv.key.to_string(),
            value: kv.value.as_ref().map(Into::into),
            key_strindex: 0,
        }
    }
}

impl<T, S> IntoWire<proto::Metric> for Series<Snapshot<T>, S>
where
    T: Measure + ToPrimitive,
    S: Clone,
{
    type Error = crate::Error;

    fn into_wire(self, align: Option<Duration>) -> Result<Option<proto::Metric>, Self::Error> {
        if self.series.is_empty() {
            return Ok(None);
        }

        let temporality = match self.observe_mode {
            Mode::Direct => proto::AggregationTemporality::Cumulative,
            Mode::Delta | Mode::Destructive => proto::AggregationTemporality::Delta,
        };

        let start_time = self.start_time;
        let start_time_unix_nano = map_time(start_time)?;

        let mut data_points = vec![];

        for (attrs, snapshots) in self.series {
            let attrs = map_attrs(&attrs);
            for snapshot in snapshots {
                let time_unix_nano = map_time(snapshot.align_ts(start_time, align))?;

                let sum = snapshot.value.sum.to_f64().ok_or(Self::Error::ValueToF64)?;

                let mut explicit_bounds = Vec::with_capacity(snapshot.value.boundaries.len());
                for b in snapshot.value.boundaries.iter() {
                    explicit_bounds.push(b.to_f64().ok_or(Self::Error::BoundaryToF64)?);
                }

                let dp = proto::HistogramDataPoint {
                    attributes: attrs.clone(),
                    start_time_unix_nano,
                    time_unix_nano,
                    count: snapshot.value.count,
                    sum: Some(sum),
                    bucket_counts: snapshot.value.bucket_counts,
                    explicit_bounds,
                    exemplars: vec![],
                    flags: 0,
                    min: snapshot
                        .value
                        .min
                        .map(|v| v.to_f64().ok_or(Self::Error::ValueToF64))
                        .transpose()?,
                    max: snapshot
                        .value
                        .max
                        .map(|v| v.to_f64().ok_or(Self::Error::ValueToF64))
                        .transpose()?,
                };

                data_points.push(dp);
            }
        }

        if data_points.is_empty() {
            return Ok(None);
        }

        Ok(Some(proto::Metric {
            name: self.id.name.to_string(),
            description: self.id.description.to_string(),
            unit: self.id.unit.to_string(),
            metadata: vec![],
            data: Some(proto::metric::Data::Histogram(proto::Histogram {
                data_points,
                aggregation_temporality: temporality as i32,
            })),
        }))
    }
}

impl<T, S> IntoWire<proto::Metric> for Series<T, S>
where
    T: Measure + IntoNumberDataPointValue,
    S: Clone,
{
    type Error = crate::Error;

    fn into_wire(self, align: Option<Duration>) -> Result<Option<proto::Metric>, Self::Error> {
        if self.series.is_empty() {
            return Ok(None);
        }

        let temporality = match self.observe_mode {
            Mode::Direct => proto::AggregationTemporality::Cumulative,
            Mode::Delta | Mode::Destructive => proto::AggregationTemporality::Delta,
        };

        let start_time = self.start_time;
        let start_time_unix_nano = map_time(start_time)?;
        let mut data_points = vec![];

        for (attrs, snapshots) in self.series {
            let attrs = map_attrs(&attrs);

            for snapshot in snapshots {
                let time_unix_nano = map_time(snapshot.align_ts(start_time, align))?;

                let dp = proto::NumberDataPoint {
                    attributes: attrs.clone(),
                    start_time_unix_nano,
                    time_unix_nano,
                    exemplars: vec![],
                    flags: 0,
                    value: Some(snapshot.value.into_number_value()?),
                };

                data_points.push(dp);
            }
        }

        if data_points.is_empty() {
            return Ok(None);
        }

        let data = match self.kind {
            Kind::Counter => proto::metric::Data::Sum(proto::Sum {
                data_points,
                aggregation_temporality: temporality as i32,
                // `is_monotonic` describes the metric kind (Counter vs UpDownCounter),
                // not its temporality: a delta Counter is still monotonic.
                is_monotonic: true,
            }),
            Kind::Gauge => proto::metric::Data::Gauge(proto::Gauge { data_points }),
            Kind::Histogram => unreachable!("bug: can't be"),
        };

        Ok(Some(proto::Metric {
            name: self.id.name.to_string(),
            description: self.id.description.to_string(),
            unit: self.id.unit.to_string(),
            metadata: vec![],
            data: Some(data),
        }))
    }
}

pub trait IntoNumberDataPointValue {
    fn into_number_value(self) -> Result<proto::number_data_point::Value, crate::Error>;
}

impl IntoNumberDataPointValue for i64 {
    #[inline(always)]
    fn into_number_value(self) -> Result<proto::number_data_point::Value, crate::Error> {
        Ok(proto::number_data_point::Value::AsInt(self))
    }
}

impl IntoNumberDataPointValue for u64 {
    #[inline(always)]
    fn into_number_value(self) -> Result<proto::number_data_point::Value, crate::Error> {
        let value = i64::try_from(self).map_err(|_| crate::Error::ValueToI64)?;
        Ok(proto::number_data_point::Value::AsInt(value))
    }
}

impl IntoNumberDataPointValue for f64 {
    #[inline(always)]
    fn into_number_value(self) -> Result<proto::number_data_point::Value, crate::Error> {
        Ok(proto::number_data_point::Value::AsDouble(self))
    }
}

#[inline]
fn map_time(t: SystemTime) -> Result<u64, crate::Error> {
    let nanos = t.duration_since(UNIX_EPOCH)?.as_nanos();
    u64::try_from(nanos).map_err(|_| crate::Error::TimeOverflowsU64)
}

#[inline]
fn map_attrs(attrs: &[KeyValue]) -> Vec<pb::KeyValue> {
    attrs.iter().map(From::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metric::Counter;
    use crate::metric::tests::ID;
    use crate::observe::{DynObserver, Mode, SyncObserver};

    #[test]
    fn counters_are_monotonic() {
        let c = Counter::<u64>::new(ID);
        let mut obs: Box<dyn DynObserver<proto::Metric, crate::Error>> =
            Box::new(SyncObserver::new(&c, Mode::Delta).expect("delta mode is supported"));
        c.add(7, &[]);
        obs.observe(SystemTime::now());
        let metric = obs
            .export(None)
            .expect("convert")
            .expect("metric was produced");
        match metric.data.expect("data") {
            proto::metric::Data::Sum(sum) => {
                assert_eq!(
                    sum.aggregation_temporality,
                    proto::AggregationTemporality::Delta as i32
                );
                assert!(
                    sum.is_monotonic,
                    "a counter must be monotonic regardless of temporality"
                );
            }
            other => panic!("expected sum, got {:?}", other),
        }
    }
}
