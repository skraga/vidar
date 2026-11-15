//! Allocation-free order statistics matching NumPy's defaults.

/// Sort `scratch[..n]` in place and return NumPy's `percentile(values, q)` ("linear" method).
pub(crate) fn percentile_in_place(values: &mut [f64], q: f64) -> f64 {
    debug_assert!(!values.is_empty());
    values.sort_unstable_by(|a, b| a.total_cmp(b));
    let n = values.len();
    let pos = (n - 1) as f64 * (q / 100.0);
    let lo = pos.floor() as usize;
    let hi = (lo + 1).min(n - 1);
    let t = pos - lo as f64;
    lerp(values[lo], values[hi], t)
}

/// NumPy's `median`: mean of the two middle values for even counts.
pub(crate) fn median_in_place(values: &mut [f64]) -> f64 {
    debug_assert!(!values.is_empty());
    values.sort_unstable_by(|a, b| a.total_cmp(b));
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

/// NumPy's `_lerp`: switches formula at t >= 0.5 for better accuracy near `b`.
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    let diff = b - a;
    if t >= 0.5 {
        b - diff * (1.0 - t)
    } else {
        a + diff * t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_numpy_examples() {
        // np.percentile([1, 2, 3, 4], 25) == 1.75 ; 90 -> 3.7
        assert_eq!(percentile_in_place(&mut [4.0, 1.0, 3.0, 2.0], 25.0), 1.75);
        assert!((percentile_in_place(&mut [1.0, 2.0, 3.0, 4.0], 90.0) - 3.7).abs() < 1e-12);
        assert_eq!(median_in_place(&mut [3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median_in_place(&mut [4.0, 1.0, 3.0, 2.0]), 2.5);
        assert_eq!(percentile_in_place(&mut [5.0], 90.0), 5.0);
    }
}
