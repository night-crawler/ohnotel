use crate::atomic;
use crate::atomic::IsInitial as _;
use crate::model::KeyValue;
use hashbrown::hash_table::Entry;
use hashbrown::{DefaultHashBuilder, HashMap, HashTable};
use smallvec::SmallVec;
use std::hash::{BuildHasher, Hash};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

const STACK_LABELS: usize = 8;

/// Order-independent attr set fingerprint.
#[derive(Debug, Clone, Default, Eq, PartialEq, Hash)]
pub struct AttrFingerprint {
    pub xor: u64,
    pub sum: u64,
    pub len: u16,
}

impl AttrFingerprint {
    #[inline]
    fn commutative_hash<S: BuildHasher>(attrs: &[KeyValue], build: &S) -> Self {
        let mut xor: u64 = 0;
        let mut sum: u64 = 0;
        for kv in attrs {
            let h = build.hash_one(kv);
            xor ^= h;
            sum = sum.wrapping_add(h);
        }

        let len = u16::try_from(attrs.len()).expect("sane amount of key value pairs");

        Self { xor, sum, len }
    }

    #[inline]
    pub fn table_hash<S: BuildHasher>(&self, build: &S) -> u64 {
        build.hash_one(self)
    }
}

#[derive(Debug)]
pub struct CanonicalEntry<A> {
    pub attrs: Arc<[KeyValue]>,
    pub bucket: Arc<A>,
    pub aliases: Vec<Arc<AttrFingerprint>>,
}

type AliasMap<A, S> = HashMap<Arc<AttrFingerprint>, Arc<A>, S>;

pub struct BucketMap<T, S = DefaultHashBuilder, A = <T as atomic::Measure>::Type>
where
    T: atomic::Measure,
    S: BuildHasher + Clone,
    A: atomic::Record<T> + Send + Sync + 'static,
{
    /// Entry labels are always sorted and deduplicated before insertion.
    canonical: RwLock<HashTable<CanonicalEntry<A>>>,

    /// Dupes map.
    aliases: RwLock<AliasMap<A, S>>,

    no_attr_entry: CanonicalEntry<A>,

    count: AtomicUsize,
    hasher: S,
    init_bucket: Arc<dyn Fn() -> A + Send + Sync + 'static>,
    _value: PhantomData<T>,
}

impl<T, S, A> std::fmt::Debug for BucketMap<T, S, A>
where
    T: atomic::Measure,
    S: BuildHasher + Clone,
    A: atomic::Record<T> + Send + Sync + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BucketMap")
            .field("no_attr", &self.no_attr_entry.bucket)
            .field("count", &self.count.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl<T> Default for BucketMap<T>
where
    T: atomic::Measure + 'static,
    T::Type: Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T> BucketMap<T>
where
    T: atomic::Measure + 'static,
    T::Type: Send + Sync + 'static,
{
    #[inline]
    pub fn new() -> Self {
        Self::with_hasher(DefaultHashBuilder::default())
    }
}

impl<T, S> BucketMap<T, S>
where
    T: atomic::Measure + 'static,
    T::Type: Send + Sync + 'static,
    S: BuildHasher + Clone,
{
    pub fn with_hasher(hasher: S) -> Self {
        Self::with_storage(hasher, || T::Type::from(T::default()))
    }
}

impl<T, S, A> BucketMap<T, S, A>
where
    T: atomic::Measure,
    S: BuildHasher + Clone,
    A: atomic::Record<T> + Send + Sync + 'static,
{
    pub fn with_storage(hasher: S, init_bucket: impl Fn() -> A + Send + Sync + 'static) -> Self {
        let init_bucket: Arc<dyn Fn() -> A + Send + Sync + 'static> = Arc::new(init_bucket);

        Self {
            canonical: RwLock::new(HashTable::new()),
            aliases: RwLock::new(HashMap::with_hasher(hasher.clone())),
            no_attr_entry: CanonicalEntry {
                attrs: Arc::new([]),
                bucket: Arc::new(init_bucket.as_ref()()),
                aliases: vec![],
            },
            count: AtomicUsize::new(0),
            hasher,
            init_bucket,
            _value: PhantomData,
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    #[inline]
    pub const fn hasher(&self) -> &S {
        &self.hasher
    }

    #[inline]
    pub fn get_bucket(&self, attrs: &[KeyValue]) -> Result<Arc<A>, (AttrFingerprint, u64)> {
        if attrs.is_empty() {
            return Ok(Arc::clone(&self.no_attr_entry.bucket));
        }

        let raw_fp = AttrFingerprint::commutative_hash(attrs, &self.hasher);
        let raw_hash = raw_fp.table_hash(&self.hasher);

        if attrs.len() == 1 {
            let Some(bucket) = self.lookup_canon(raw_hash, attrs) else {
                return Err((raw_fp, raw_hash));
            };
            return Ok(bucket);
        }

        if attrs.is_sorted() {
            if let Some(bucket) = self
                .lookup_canon(raw_hash, attrs)
                .or_else(|| self.lookup_dup(&raw_fp))
            {
                return Ok(bucket);
            }
        } else if let Some(bucket) = self
            .lookup_dup(&raw_fp)
            .or_else(|| self.lookup_canon(raw_hash, attrs))
        {
            return Ok(bucket);
        }

        Err((raw_fp, raw_hash))
    }

    pub fn get_or_create(&self, attrs: &[KeyValue]) -> Arc<A> {
        let (raw_fp, raw_hash) = match self.get_bucket(attrs) {
            Ok(bucket) => return bucket,
            Err(hashes) => hashes,
        };

        let mut buf: SmallVec<[KeyValue; STACK_LABELS]> = attrs.iter().cloned().collect();
        buf.sort_unstable();
        buf.dedup();

        let needs_alias = buf.as_slice() != attrs;

        // Since the hash is commutative, if the length is the same, it should remain the same
        // after sorting.
        let canon_hash = if buf.len() != raw_fp.len as usize {
            AttrFingerprint::commutative_hash(&buf, &self.hasher).table_hash(&self.hasher)
        } else {
            raw_hash
        };

        let alias = needs_alias.then(|| Arc::new(raw_fp));

        let bucket = self.insert_canon(buf, canon_hash, alias.as_ref());
        if let Some(alias) = alias {
            self.insert_dup(alias, &bucket);
        }

        bucket
    }

    #[inline]
    fn lookup_canon(&self, hash: u64, attrs: &[KeyValue]) -> Option<Arc<A>> {
        let table = self
            .canonical
            .read()
            .expect("canonical bucket map lock poisoned");
        let entry = table.find(hash, |e| &*e.attrs == attrs)?;
        Some(Arc::clone(&entry.bucket))
    }

    #[inline]
    fn lookup_dup(&self, fp: &AttrFingerprint) -> Option<Arc<A>> {
        let map = self.aliases.read().expect("alias bucket map lock poisoned");
        map.get(fp).map(Arc::clone)
    }

    fn insert_canon(
        &self,
        normalized: SmallVec<[KeyValue; STACK_LABELS]>,
        hash: u64,
        alias: Option<&Arc<AttrFingerprint>>,
    ) -> Arc<A> {
        let mut table = self
            .canonical
            .write()
            .expect("canonical bucket map lock poisoned");

        let rehash = |entry: &CanonicalEntry<A>| {
            AttrFingerprint::commutative_hash(&entry.attrs, &self.hasher).table_hash(&self.hasher)
        };

        let entry = table.entry(
            hash,
            |entry| entry.attrs.as_ref() == normalized.as_slice(),
            rehash,
        );

        match entry {
            Entry::Occupied(mut entry) => {
                // If it got called with non-empty alias, then we must have not had it before,
                // so we need to add it
                entry.get_mut().aliases.extend(alias.into_iter().cloned());
                Arc::clone(&entry.get().bucket)
            }
            Entry::Vacant(entry) => {
                let attrs = Arc::from_iter(normalized);
                let bucket = Arc::new(self.init_bucket.as_ref()());

                // `VacantEntry::insert` returns an `OccupiedEntry` for chaining; we don't chain.
                let _ = entry.insert(CanonicalEntry {
                    attrs,
                    bucket: Arc::clone(&bucket),
                    aliases: alias.into_iter().cloned().collect(),
                });
                let _ = self.count.fetch_add(1, Ordering::Relaxed);
                bucket
            }
        }
    }

    fn insert_dup(&self, raw_fp: Arc<AttrFingerprint>, bucket: &Arc<A>) {
        let mut map = self
            .aliases
            .write()
            .expect("alias bucket map lock poisoned");
        // `or_insert_with` returns `&mut V` for chaining; we only need the side effect.
        let _ = map.entry(raw_fp).or_insert_with(|| Arc::clone(bucket));
    }

    #[inline]
    pub fn add(&self, value: T, attrs: &[KeyValue]) {
        if attrs.is_empty() {
            self.no_attr_entry.bucket.add(value);
            return;
        }

        self.get_or_create(attrs).add(value);
    }

    #[inline]
    pub fn sub(&self, value: T, attrs: &[KeyValue]) {
        if attrs.is_empty() {
            self.no_attr_entry.bucket.sub(value);
            return;
        }

        self.get_or_create(attrs).sub(value);
    }

    #[inline]
    pub fn clear(&self) {
        let mut canon = self
            .canonical
            .write()
            .expect("canonical bucket map lock poisoned");
        canon.clear();

        let mut aliases = self
            .aliases
            .write()
            .expect("alias bucket map lock poisoned");
        aliases.clear();

        self.count.store(0, Ordering::Relaxed);
        self.no_attr_entry.bucket.clear();
    }

    pub fn visit_bucket<F>(&self, mut cb: F)
    where
        F: FnMut(&CanonicalEntry<A>) -> bool,
    {
        let mut table = self
            .canonical
            // Taking write lock to reduce (not eliminate) the number of buckets changed midair
            // while we're visiting.
            .write()
            .expect("canonical bucket map lock poisoned");
        let mut evicted: usize = 0;
        // Retain is used just in case we need to evict nodes based on some cardinality
        // observations later.
        table.retain(|e| {
            let retain = cb(e);
            if !retain {
                evicted += 1;
                if !e.aliases.is_empty() {
                    let mut aliases = self
                        .aliases
                        .write()
                        .expect("alias bucket map lock poisoned");
                    for alias in &e.aliases {
                        // We have no use for the removed alias entry; we're evicting the bucket.
                        let _ = aliases.remove(alias);
                    }
                }
            }

            retain
        });
        if evicted > 0 {
            let _ = self.count.fetch_sub(evicted, Ordering::Relaxed);
        }
        if !cb(&self.no_attr_entry) {
            self.no_attr_entry.bucket.clear();
        }
    }

    pub fn take(
        &self,
    ) -> (
        HashTable<CanonicalEntry<A>>,
        Option<<A as atomic::Record<T>>::Snapshot>,
    ) {
        let mut canon = self
            .canonical
            .write()
            .expect("canonical bucket map lock poisoned");
        let table = std::mem::replace(&mut *canon, HashTable::new());
        let mut aliases = self
            .aliases
            .write()
            .expect("alias bucket map lock poisoned");
        aliases.clear();

        let no_attr_val = self.no_attr_entry.bucket.current();
        let no_attr_val = (!no_attr_val.is_initial()).then_some(no_attr_val);

        self.no_attr_entry.bucket.clear();

        (table, no_attr_val)
    }
}

impl<T, S, A> BucketMap<T, S, A>
where
    T: atomic::Measure,
    S: BuildHasher + Clone,
    A: atomic::Scalar<T> + Send + Sync + 'static,
{
    #[inline]
    pub fn reset(&self, attrs: &[KeyValue]) -> T {
        if attrs.is_empty() {
            return self.no_attr_entry.bucket.reset();
        }
        self.get_or_create(attrs).reset()
    }

    #[inline]
    pub fn set(&self, value: T, attrs: &[KeyValue]) {
        if attrs.is_empty() {
            return self.no_attr_entry.bucket.store(value);
        }
        self.get_or_create(attrs).store(value);
    }

    pub fn get(&self, attrs: &[KeyValue]) -> Option<T> {
        if attrs.is_empty() {
            return Some(self.no_attr_entry.bucket.get());
        }

        match self.get_bucket(attrs) {
            Ok(bucket) => Some(bucket.get()),
            Err(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atomic::Scalar as _;
    use crate::model::{Str, Value};

    fn kv(k: impl Into<Str>, v: impl Into<Value>) -> KeyValue {
        KeyValue::new(k.into(), v.into())
    }

    #[test]
    fn same_bucket_no_attr() {
        let map: BucketMap<u64> = BucketMap::new();
        let a = map.get_or_create(&[]);
        let b = map.get_or_create(&[]);
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn same_bucket_attrs() {
        let map: BucketMap<u64> = BucketMap::new();
        let attrs = [kv("a", 1), kv("b", 2), kv("c", 3)];
        let a = map.get_or_create(&attrs);
        let b = map.get_or_create(&attrs);
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn same_bucket_unsorted_attrs() {
        let map: BucketMap<u64> = BucketMap::new();
        let sorted = [kv("a", 1), kv("b", 2), kv("c", 3)];
        let shuffled = [kv("c", 3), kv("a", 1), kv("b", 2)];

        let a = map.get_or_create(&sorted);
        let b = map.get_or_create(&shuffled);

        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn aliases() {
        let map: BucketMap<u64> = BucketMap::new();
        let canonical = [kv("a", 1), kv("b", 2)];
        let duped = [kv("b", 2), kv("a", 1), kv("a", 1)];

        let c = map.get_or_create(&canonical);
        let d = map.get_or_create(&duped);

        assert!(Arc::ptr_eq(&c, &d));
        assert_eq!(map.len(), 1);

        let d2 = map.get_or_create(&duped);
        assert!(Arc::ptr_eq(&c, &d2));
    }

    #[test]
    fn hashing_not_broken() {
        let map: BucketMap<u64> = BucketMap::new();
        let a = map.get_or_create(&[kv("a", 1)]);
        let b = map.get_or_create(&[kv("a", 2)]);
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn rehash() {
        let map: BucketMap<u64> = BucketMap::new();
        let n = 1025 * 16;
        for i in 0..n {
            map.add(1, &[kv("idx", i)]);
        }
        assert_eq!(map.len(), usize::try_from(n).expect("n fits in usize"));
        for i in 0..n {
            let b = map.get_or_create(&[kv("idx", i)]);
            assert_eq!(b.get(), 1);
        }
    }
}
