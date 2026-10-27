//! Experiment only. The original `sog::remap` handles unordered or NaN inputs.
pub fn candidate(from: &[f32], to: &[f32]) -> [u8; 256] {
    let from = &from[..from.len().min(256)];
    // A NaN fails the adjacent comparison; the first check also catches a
    // one-element slice containing NaN. Signed zeros compare equal as before.
    let sorted = |values: &[f32]| {
        values.first().is_none_or(|v| !v.is_nan()) && values.windows(2).all(|p| p[0] <= p[1])
    };
    if !sorted(from) || !sorted(to) {
        return crate::sog::remap(from, to);
    }
    let mut map = [0; 256];
    let mut cursor = 0;
    for (i, &v) in from.iter().enumerate() {
        while cursor < to.len() && to[cursor] < v {
            cursor += 1;
        }
        let j = cursor.min(to.len() - 1);
        let best = if j > 0 && (v - to[j - 1]).abs() <= (to[j] - v).abs() { j - 1 } else { j };
        map[i] = best as u8;
    }
    map
}
