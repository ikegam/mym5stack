//! Single-cell Li-ion state of charge, estimated from the cell voltage.

/// (mV, %) points of a typical Li-ion discharge curve; linear in between.
const CURVE: [(u32, u32); 11] = [
    (3300, 0),
    (3500, 5),
    (3600, 10),
    (3700, 25),
    (3750, 40),
    (3800, 50),
    (3850, 60),
    (3900, 70),
    (4000, 80),
    (4100, 90),
    (4200, 100),
];

pub fn percent(mv: u32) -> u8 {
    let (lo, hi) = (CURVE[0], CURVE[CURVE.len() - 1]);
    if mv <= lo.0 {
        return lo.1 as u8;
    }
    if mv >= hi.0 {
        return hi.1 as u8;
    }
    let i = CURVE.iter().position(|&(v, _)| v > mv).unwrap();
    let ((v0, p0), (v1, p1)) = (CURVE[i - 1], CURVE[i]);
    (p0 + (mv - v0) * (p1 - p0) / (v1 - v0)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_and_interpolates() {
        assert_eq!(percent(3000), 0);
        assert_eq!(percent(4350), 100); // charging
        assert_eq!(percent(3800), 50);
        assert_eq!(percent(3950), 75);
        assert!((1..=100).all(|p| percent(3300 + p * 9) >= percent(3300 + (p - 1) * 9)));
    }
}
