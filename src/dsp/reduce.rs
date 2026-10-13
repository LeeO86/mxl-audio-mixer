//! Reductions over a block that vectorise: the largest |sample| and the sum of
//! squares in eight independent lanes. A plain `fold` with `f32::max` or an f64
//! sum is one long dependency chain (the compiler may not reorder it), about four
//! cycles a sample; the DSP thread runs them for every meter and node.

/// The largest |value| of `x` (as a fold with `f32::max`: NaN is ignored).
#[inline]
pub fn peak(x: &[f32]) -> f32 {
    let mut lanes = [0f32; 8];
    let mut chunks = x.chunks_exact(8);
    for c in &mut chunks {
        for j in 0..8 {
            lanes[j] = lanes[j].max(c[j].abs());
        }
    }
    let p = lanes.iter().fold(0f32, |a, b| a.max(*b));
    chunks.remainder().iter().fold(p, |a, v| a.max(v.abs()))
}

/// The sum of squares of `x`, in f64.
#[inline]
pub fn energy(x: &[f32]) -> f64 {
    let mut lanes = [0f64; 8];
    let mut chunks = x.chunks_exact(8);
    for c in &mut chunks {
        for j in 0..8 {
            let v = f64::from(c[j]);
            lanes[j] += v * v;
        }
    }
    let e: f64 = lanes.iter().sum();
    chunks
        .remainder()
        .iter()
        .fold(e, |a, v| a + f64::from(*v) * f64::from(*v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_the_plain_folds() {
        let x: Vec<f32> = (0..101)
            .map(|i| ((i * 37 % 23) as f32 - 11.0) * 0.07)
            .collect();
        for n in [0, 1, 7, 8, 9, 96, 101] {
            let s = &x[..n];
            assert_eq!(peak(s), s.iter().fold(0f32, |a, v| a.max(v.abs())));
            let e: f64 = s.iter().map(|v| f64::from(*v) * f64::from(*v)).sum();
            assert!((energy(s) - e).abs() <= 1e-12 * e.max(1.0));
        }
    }
}
