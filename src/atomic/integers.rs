use crate::atomic;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

impl atomic::Record<u64> for AtomicU64 {
    type Snapshot = u64;

    #[inline(always)]
    fn add(&self, value: u64) {
        let _ = self.fetch_add(value, Ordering::Relaxed);
    }

    #[inline(always)]
    fn sub(&self, value: u64) {
        let _ = self.fetch_sub(value, Ordering::Relaxed);
    }

    #[inline(always)]
    fn clear(&self) {
        AtomicU64::store(self, 0, Ordering::Relaxed);
    }

    #[inline(always)]
    fn current(&self) -> u64 {
        AtomicU64::load(self, Ordering::Relaxed)
    }
}

impl atomic::Scalar<u64> for AtomicU64 {
    #[inline(always)]
    fn fetch_min(&self, value: u64) {
        let _ = AtomicU64::fetch_min(self, value, Ordering::Relaxed);
    }

    #[inline(always)]
    fn fetch_max(&self, value: u64) {
        let _ = AtomicU64::fetch_max(self, value, Ordering::Relaxed);
    }

    #[inline(always)]
    fn swap(&self, value: u64) -> u64 {
        AtomicU64::swap(self, value, Ordering::Relaxed)
    }

    #[inline(always)]
    fn load(&self) -> u64 {
        AtomicU64::load(self, Ordering::Relaxed)
    }

    #[inline(always)]
    fn store(&self, value: u64) {
        AtomicU64::store(self, value, Ordering::Relaxed);
    }

    #[inline(always)]
    fn reset(&self) -> u64 {
        AtomicU64::swap(self, 0, Ordering::Relaxed)
    }
}

impl atomic::Measure for u64 {
    type Type = AtomicU64;

    fn min_identity() -> Self {
        u64::MAX
    }

    fn max_identity() -> Self {
        0
    }
}

impl atomic::Record<i64> for AtomicI64 {
    type Snapshot = i64;

    #[inline(always)]
    fn add(&self, value: i64) {
        let _ = self.fetch_add(value, Ordering::Relaxed);
    }

    #[inline(always)]
    fn sub(&self, value: i64) {
        let _ = self.fetch_sub(value, Ordering::Relaxed);
    }

    #[inline(always)]
    fn clear(&self) {
        AtomicI64::store(self, 0, Ordering::Relaxed);
    }

    #[inline(always)]
    fn current(&self) -> i64 {
        AtomicI64::load(self, Ordering::Relaxed)
    }
}

impl atomic::Scalar<i64> for AtomicI64 {
    #[inline(always)]
    fn fetch_min(&self, value: i64) {
        let _ = AtomicI64::fetch_min(self, value, Ordering::Relaxed);
    }

    #[inline(always)]
    fn fetch_max(&self, value: i64) {
        let _ = AtomicI64::fetch_max(self, value, Ordering::Relaxed);
    }

    #[inline(always)]
    fn swap(&self, value: i64) -> i64 {
        AtomicI64::swap(self, value, Ordering::Relaxed)
    }

    #[inline(always)]
    fn load(&self) -> i64 {
        AtomicI64::load(self, Ordering::Relaxed)
    }

    #[inline(always)]
    fn store(&self, value: i64) {
        AtomicI64::store(self, value, Ordering::Relaxed);
    }

    #[inline(always)]
    fn reset(&self) -> i64 {
        AtomicI64::swap(self, 0, Ordering::Relaxed)
    }
}

impl atomic::Measure for i64 {
    type Type = AtomicI64;

    fn min_identity() -> Self {
        i64::MAX
    }

    fn max_identity() -> Self {
        i64::MIN
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atomic::Record as _;
    use std::sync::atomic::AtomicU64;

    #[test]
    fn bucket() {
        let a = AtomicU64::new(0);
        a.add(1u64);
        a.add(4u64);
        assert_eq!(atomic::Scalar::get(&a), 5);
        assert_eq!(atomic::Scalar::reset(&a), 5);
        assert_eq!(atomic::Scalar::get(&a), 0);

        let a = AtomicU64::from(0);
        a.add(1u64);
        assert_eq!(atomic::Scalar::get(&a), 1);
    }

    #[test]
    fn sample() {
        let a = AtomicU64::new(100);
        atomic::Scalar::fetch_min(&a, 50);
        assert_eq!(atomic::Scalar::get(&a), 50);
        atomic::Scalar::fetch_min(&a, 80);
        assert_eq!(atomic::Scalar::get(&a), 50);
        atomic::Scalar::fetch_max(&a, 70);
        assert_eq!(atomic::Scalar::get(&a), 70);
        atomic::Scalar::fetch_max(&a, 200);
        assert_eq!(atomic::Scalar::get(&a), 200);
    }
}
