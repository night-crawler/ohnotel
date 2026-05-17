use crate::model::NameIdentity;
use crate::observe::{Mode, SeriesMap};
use hashbrown::DefaultHashBuilder;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

pub trait IntoWire<W> {
    type Error;

    /// When `align` is provided, the time is calculated from the start time multiplied by the
    /// seq_id.
    fn into_wire(self, align: Option<Duration>) -> Result<Option<W>, Self::Error>;
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Kind {
    Counter,
    Gauge,
    Histogram,
}

#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Snapshot<T> {
    pub ts: SystemTime,
    pub seq_id: u64,
    pub value: T,
}

impl<T> Snapshot<T> {
    pub fn align_ts(&self, start_ts: SystemTime, poll_period: Option<Duration>) -> SystemTime {
        let Some(poll_period) = poll_period else {
            return self.ts;
        };

        let Ok(seq_id) = u32::try_from(self.seq_id) else {
            return self.ts;
        };

        start_ts + poll_period * seq_id
    }
}

pub struct Series<V: Clone, S: Clone = DefaultHashBuilder> {
    pub start_time: SystemTime,
    pub id: Arc<NameIdentity>,
    pub series: SeriesMap<V, S>,
    pub observe_mode: Mode,
    pub kind: Kind,
}

impl<V: Clone, S: Clone> Clone for Series<V, S> {
    fn clone(&self) -> Self {
        Self {
            start_time: self.start_time,
            id: Arc::clone(&self.id),
            series: self.series.clone(),
            observe_mode: self.observe_mode,
            kind: self.kind,
        }
    }
}

impl<V: fmt::Debug + Clone, S: fmt::Debug + Clone> fmt::Debug for Series<V, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Series")
            .field("start_time", &self.start_time)
            .field("id", &self.id)
            .field("series", &self.series)
            .field("observe_mode", &self.observe_mode)
            .finish()
    }
}
