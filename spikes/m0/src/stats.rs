//! Fixed-size statistics: nothing here allocates after construction, so the
//! block thread can record into it.

/// Histogram of non-negative durations in 1 µs buckets up to `BUCKETS` µs; larger
/// values land in the last bucket. The exact maximum is kept separately.
pub struct Histogram {
    buckets: Box<[u64; BUCKETS]>,
    count: u64,
    sum_ns: u128,
    max_ns: u64,
    min_ns: u64,
}

pub const BUCKETS: usize = 20_000;

impl Histogram {
    pub fn new() -> Self {
        Self {
            buckets: Box::new([0; BUCKETS]),
            count: 0,
            sum_ns: 0,
            max_ns: 0,
            min_ns: u64::MAX,
        }
    }

    pub fn record(&mut self, ns: u64) {
        let bucket = ((ns / 1_000) as usize).min(BUCKETS - 1);
        self.buckets[bucket] += 1;
        self.count += 1;
        self.sum_ns += u128::from(ns);
        self.max_ns = self.max_ns.max(ns);
        self.min_ns = self.min_ns.min(ns);
    }

    /// The upper edge (µs) of the bucket holding quantile `q`.
    pub fn quantile_us(&self, q: f64) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let target = ((self.count as f64) * q).ceil().max(1.0) as u64;
        let mut seen = 0;
        for (i, n) in self.buckets.iter().enumerate() {
            seen += n;
            if seen >= target {
                return (i + 1) as f64;
            }
        }
        BUCKETS as f64
    }

    /// How many values exceeded `limit_ns` (bucket resolution: 1 µs).
    pub fn above_us(&self, limit_us: usize) -> u64 {
        self.buckets[limit_us.min(BUCKETS - 1)..].iter().sum()
    }

    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "count": self.count,
            "min_us": if self.count == 0 { 0.0 } else { self.min_ns as f64 / 1e3 },
            "mean_us": if self.count == 0 { 0.0 } else { self.sum_ns as f64 / self.count as f64 / 1e3 },
            "p50_us": self.quantile_us(0.50),
            "p99_us": self.quantile_us(0.99),
            "p999_us": self.quantile_us(0.999),
            "p9999_us": self.quantile_us(0.9999),
            "max_us": self.max_ns as f64 / 1e3,
        })
    }
}
