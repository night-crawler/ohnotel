use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

pub trait AtomicMeasure: Sized {
    type Type: AtomicNumOps<Self>;
    fn init(value: Self) -> Self::Type;
}

pub trait AtomicNumOps<T> {
    fn add(&self, value: T);
    fn sub(&self, value: T);
    fn reset(&self) -> T;
    fn get(&self) -> T;
}

impl AtomicNumOps<u64> for AtomicU64 {
    #[inline(always)]
    fn add(&self, value: u64) {
        self.fetch_add(value, Ordering::Relaxed);
    }

    #[inline(always)]
    fn sub(&self, value: u64) {
        self.fetch_sub(value, Ordering::Relaxed);
    }

    #[inline(always)]
    fn reset(&self) -> u64 {
        self.swap(0, Ordering::Relaxed)
    }

    #[inline(always)]
    fn get(&self) -> u64 {
        self.load(Ordering::Relaxed)
    }
}

impl AtomicMeasure for u64 {
    type Type = AtomicU64;

    fn init(value: Self) -> Self::Type {
        Self::Type::new(value)
    }
}

impl AtomicNumOps<i64> for AtomicI64 {
    #[inline(always)]
    fn add(&self, value: i64) {
        self.fetch_add(value, Ordering::Relaxed);
    }
    #[inline(always)]
    fn sub(&self, value: i64) {
        self.fetch_sub(value, Ordering::Relaxed);
    }
    #[inline(always)]
    fn reset(&self) -> i64 {
        self.swap(0, Ordering::Relaxed)
    }
    #[inline(always)]
    fn get(&self) -> i64 {
        self.load(Ordering::Relaxed)
    }
}

impl AtomicMeasure for i64 {
    type Type = AtomicI64;

    fn init(value: Self) -> Self::Type {
        Self::Type::new(value)
    }
}

pub struct AtomicF64 {
    bits: AtomicU64,
}

impl AtomicF64 {
    #[inline]
    pub fn new(value: f64) -> Self {
        Self {
            bits: AtomicU64::new(value.to_bits()),
        }
    }
}

impl AtomicNumOps<f64> for AtomicF64 {
    #[inline(always)]
    fn add(&self, value: f64) {
        let mut old = self.bits.load(Ordering::Relaxed);

        loop {
            let current = f64::from_bits(old);
            let new = (current + value).to_bits();

            match self
                .bits
                .compare_exchange_weak(old, new, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return,
                Err(next) => old = next,
            }
        }
    }

    #[inline(always)]
    fn sub(&self, value: f64) {
        self.add(-value);
    }

    #[inline(always)]
    fn reset(&self) -> f64 {
        f64::from_bits(self.bits.swap(0.0f64.to_bits(), Ordering::Relaxed))
    }

    #[inline(always)]
    fn get(&self) -> f64 {
        f64::from_bits(self.bits.load(Ordering::Relaxed))
    }
}

impl AtomicMeasure for f64 {
    type Type = AtomicF64;

    fn init(value: Self) -> Self::Type {
        Self::Type::new(value)
    }
}

pub struct AtomicBucket<T>
where
    T: AtomicMeasure,
    T::Type: AtomicNumOps<T>,
{
    access_counter: AtomicU64,
    value: T::Type,
}

impl<T> AtomicBucket<T>
where
    T: AtomicMeasure,
    T::Type: AtomicNumOps<T>,
{
    pub fn new(value: T) -> Self {
        Self {
            access_counter: Default::default(),
            value: T::init(value),
        }
    }
}

impl<T> AtomicNumOps<T> for AtomicBucket<T>
where
    T: AtomicMeasure,
    T::Type: AtomicNumOps<T>,
{
    #[inline(always)]
    fn add(&self, value: T) {
        self.access_counter.fetch_add(1, Ordering::Relaxed);
        self.value.add(value);
    }

    #[inline(always)]
    fn sub(&self, value: T) {
        self.access_counter.fetch_add(1, Ordering::Relaxed);
        self.value.sub(value);
    }
    #[inline(always)]
    fn reset(&self) -> T {
        self.access_counter.fetch_add(1, Ordering::Relaxed);
        self.value.reset()
    }
    #[inline(always)]
    fn get(&self) -> T {
        self.access_counter.fetch_add(1, Ordering::Relaxed);
        self.value.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test1() {
        let a = AtomicBucket::new(0u64);
        a.add(1u64);
    }
}
