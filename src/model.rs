use crate::atomic::{AtomicMeasure, AtomicNumOps};
use opentelemetry_proto::tonic::common::v1 as pb;
use ordered_float::OrderedFloat;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone)]
pub enum Str {
    Cow(Cow<'static, str>),
    ArcStr(Arc<str>),
}

impl From<&'static str> for Str {
    fn from(value: &'static str) -> Self {
        Self::Cow(Cow::Borrowed(value))
    }
}

impl From<String> for Str {
    fn from(value: String) -> Self {
        Self::Cow(Cow::Owned(value))
    }
}

impl From<Arc<str>> for Str {
    fn from(value: Arc<str>) -> Self {
        Self::ArcStr(value)
    }
}

impl From<&Arc<str>> for Str {
    fn from(value: &Arc<str>) -> Self {
        Self::ArcStr(Arc::clone(value))
    }
}

impl From<Cow<'static, str>> for Str {
    fn from(value: Cow<'static, str>) -> Self {
        Self::Cow(value)
    }
}

impl Str {
    #[inline]
    pub fn as_str(&self) -> &str {
        match self {
            Str::Cow(s) => s.as_ref(),
            Str::ArcStr(s) => s.as_ref(),
        }
    }

    #[inline]
    fn ptr_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::ArcStr(l), Self::ArcStr(r)) => Arc::ptr_eq(l, r),

            (Self::Cow(Cow::Borrowed(l)), Self::Cow(Cow::Borrowed(r))) => {
                l.as_ptr() == r.as_ptr() && l.len() == r.len()
            }

            _ => false,
        }
    }
}

impl PartialEq for Str {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.ptr_eq(other) || self.as_str() == other.as_str()
    }
}

impl Eq for Str {}

impl PartialOrd for Str {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Str {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        if self.ptr_eq(other) {
            Ordering::Equal
        } else {
            self.as_str().cmp(other.as_str())
        }
    }
}

impl Hash for Str {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Value {
    String(Str),
    Bool(bool),
    Int(i64),
    Double(OrderedFloat<f64>),
    ArrayAny(Vec<Value>),
    ArrayKv(Vec<KeyValue>),
    Bytes(Vec<u8>),
}

impl From<&'static str> for Value {
    fn from(value: &'static str) -> Self {
        Self::String(Str::from(value))
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(Str::from(value))
    }
}

impl From<Arc<str>> for Value {
    fn from(value: Arc<str>) -> Self {
        Self::String(Str::from(value))
    }
}

impl From<&Arc<str>> for Value {
    fn from(value: &Arc<str>) -> Self {
        Self::String(Str::from(value))
    }
}

impl From<Cow<'static, str>> for Value {
    fn from(value: Cow<'static, str>) -> Self {
        Self::String(Str::from(value))
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Double(value.into())
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct KeyValue {
    pub key: Str,
    pub value: Option<Value>,
}

impl From<&KeyValue> for KeyValue {
    fn from(value: &KeyValue) -> Self {
        value.clone()
    }
}

impl From<&&KeyValue> for KeyValue {
    fn from(value: &&KeyValue) -> Self {
        (*value).clone()
    }
}

impl From<Str> for String {
    fn from(s: Str) -> Self {
        match s {
            Str::Cow(s) => match s {
                Cow::Borrowed(s) => s.to_owned(),
                Cow::Owned(s) => s,
            },
            Str::ArcStr(s) => s.as_ref().to_owned(),
        }
    }
}

impl From<Value> for pb::any_value::Value {
    fn from(v: Value) -> Self {
        match v {
            Value::String(s) => pb::any_value::Value::StringValue(s.into()),
            Value::Bool(b) => pb::any_value::Value::BoolValue(b),
            Value::Int(i) => pb::any_value::Value::IntValue(i),
            Value::Double(d) => pb::any_value::Value::DoubleValue(d.0),
            Value::ArrayAny(arr) => pb::any_value::Value::ArrayValue(pb::ArrayValue {
                values: arr
                    .into_iter()
                    .map(|v| pb::AnyValue {
                        value: Some(v.into()),
                    })
                    .collect(),
            }),
            Value::ArrayKv(kvs) => pb::any_value::Value::KvlistValue(pb::KeyValueList {
                values: kvs.into_iter().map(Into::into).collect(),
            }),
            Value::Bytes(b) => pb::any_value::Value::BytesValue(b),
        }
    }
}

impl From<Value> for pb::AnyValue {
    fn from(v: Value) -> Self {
        pb::AnyValue {
            value: Some(v.into()),
        }
    }
}

impl From<KeyValue> for pb::KeyValue {
    fn from(kv: KeyValue) -> Self {
        pb::KeyValue {
            key: kv.key.into(),
            value: kv.value.map(|v| v.into()),
            key_strindex: 0,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct AttrIdentity {
    kvs: Vec<KeyValue>,
}

impl<I, T> From<I> for AttrIdentity
where
    I: IntoIterator<Item = T>,
    KeyValue: From<T>,
{
    fn from(value: I) -> Self {
        let mut kvs = value.into_iter().map(Into::into).collect::<Vec<KeyValue>>();
        kvs.sort_unstable();
        kvs.dedup();
        Self { kvs }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct NameIdentity {
    name: Str,
    description: Str,
    unit: Str,
}

#[derive(Debug)]
pub struct BucketMap<T>
where
    T: AtomicMeasure,
    T::Type: AtomicNumOps<T>,
{
    map: RwLock<HashMap<AttrIdentity, Arc<T>>>,
    no_attr: T,
    count: AtomicUsize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id1() {
        let kv1 = KeyValue {
            key: "k1".into(),
            value: Some("v1".into()),
        };

        let kv2 = KeyValue {
            key: "k2".into(),
            value: Some(Value::String("v2".into())),
        };

        let _ = AttrIdentity::from([&kv1, &kv2]);
    }
}
