//! Temperature → fan duty curves with hysteresis, ramp limiting and a floor.

use crate::{Error, Result};

pub type Point = [f64; 2]; // [temp °C, duty %]

/// Built-in fan presets: (mode key, points).
pub const PRESETS: &[(&str, &[Point])] = &[
    ("silent", &[[50.0, 0.0], [60.0, 20.0], [70.0, 35.0], [78.0, 55.0], [86.0, 80.0], [92.0, 100.0]]),
    ("balanced", &[[45.0, 0.0], [55.0, 25.0], [65.0, 40.0], [75.0, 60.0], [85.0, 85.0], [90.0, 100.0]]),
    ("performance", &[[40.0, 20.0], [50.0, 35.0], [60.0, 50.0], [70.0, 70.0], [80.0, 90.0], [86.0, 100.0]]),
];

pub fn preset(mode: &str) -> Option<Vec<Point>> {
    PRESETS.iter().find(|(k, _)| *k == mode).map(|(_, p)| p.to_vec())
}

pub fn validate_points(points: &[Point]) -> Result<Vec<Point>> {
    if points.len() < 2 {
        return Err(Error::Invalid("curve needs at least 2 points".into()));
    }
    for &[t, d] in points {
        if !(20.0..=105.0).contains(&t) {
            return Err(Error::Invalid(format!("temperature {t} out of range 20..105")));
        }
        if !(0.0..=100.0).contains(&d) {
            return Err(Error::Invalid(format!("duty {d} out of range 0..100")));
        }
    }
    for w in points.windows(2) {
        if w[1][0] <= w[0][0] {
            return Err(Error::Invalid("temperatures must strictly increase".into()));
        }
        if w[1][1] < w[0][1] {
            return Err(Error::Invalid("duty must not decrease as temperature rises".into()));
        }
    }
    Ok(points.to_vec())
}

#[derive(Debug, Clone)]
pub struct Curve {
    pub points: Vec<Point>,
}

impl Curve {
    pub fn new(points: &[Point]) -> Result<Curve> {
        Ok(Curve { points: validate_points(points)? })
    }

    pub fn duty(&self, temp: f64) -> f64 {
        let p = &self.points;
        if temp <= p[0][0] {
            return p[0][1];
        }
        if temp >= p[p.len() - 1][0] {
            return p[p.len() - 1][1];
        }
        for w in p.windows(2) {
            let ([t0, d0], [t1, d1]) = (w[0], w[1]);
            if t0 <= temp && temp <= t1 {
                return d0 + (d1 - d0) * (temp - t0) / (t1 - t0);
            }
        }
        p[p.len() - 1][1]
    }
}

/// Turns temperatures into a stable duty:
/// - rises jump straight to the target;
/// - falls wait until the temperature has dropped `hysteresis` °C below the
///   temperature that set the current duty, then fall at most `ramp_down` %/tick;
/// - never below `floor_pct` while temp ≥ `floor_temp`.
#[derive(Debug, Clone)]
pub struct Controller {
    pub curve: Curve,
    pub hysteresis: f64,
    pub ramp_down: f64,
    pub floor_pct: f64,
    pub floor_temp: f64,
    current: Option<f64>,
    peak_temp: f64,
}

impl Controller {
    pub fn new(curve: Curve, hysteresis: f64, ramp_down: f64, floor_pct: f64, floor_temp: f64) -> Self {
        Controller { curve, hysteresis, ramp_down, floor_pct, floor_temp, current: None, peak_temp: -1.0 }
    }

    pub fn step(&mut self, temp: f64) -> u8 {
        let target = self.curve.duty(temp);
        let mut cur = match self.current {
            None => {
                self.peak_temp = temp;
                target
            }
            Some(c) if target > c => {
                self.peak_temp = temp;
                target
            }
            Some(c) if target == c => c,
            Some(c) => {
                if temp <= self.peak_temp - self.hysteresis {
                    let n = target.max(c - self.ramp_down);
                    if n == target {
                        self.peak_temp = temp;
                    }
                    n
                } else {
                    c
                }
            }
        };
        if temp >= self.floor_temp {
            cur = cur.max(self.floor_pct);
        }
        self.current = Some(cur);
        cur.round() as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation() {
        let c = Curve::new(&[[40.0, 0.0], [60.0, 50.0], [80.0, 100.0]]).unwrap();
        assert_eq!(c.duty(30.0), 0.0);
        assert_eq!(c.duty(50.0), 25.0);
        assert_eq!(c.duty(70.0), 75.0);
        assert_eq!(c.duty(95.0), 100.0);
    }

    #[test]
    fn presets_valid() {
        for (_, p) in PRESETS {
            validate_points(p).unwrap();
        }
    }

    #[test]
    fn validation() {
        for bad in [
            vec![[40.0, 0.0]],
            vec![[60.0, 10.0], [50.0, 20.0]],
            vec![[40.0, 50.0], [60.0, 20.0]],
            vec![[10.0, 0.0], [60.0, 20.0]],
            vec![[40.0, 0.0], [60.0, 120.0]],
        ] {
            assert!(validate_points(&bad).is_err(), "{bad:?}");
        }
    }

    fn ctl() -> Controller {
        Controller::new(Curve::new(&[[40.0, 0.0], [80.0, 100.0]]).unwrap(), 4.0, 5.0, 20.0, 60.0)
    }

    #[test]
    fn rises_immediately() {
        let mut c = ctl();
        assert_eq!(c.step(50.0), 25);
        assert_eq!(c.step(70.0), 75);
    }

    #[test]
    fn hysteresis_holds() {
        let mut c = ctl();
        c.step(70.0);
        assert_eq!(c.step(68.0), 75);
    }

    #[test]
    fn ramp_down_limited() {
        let mut c = ctl();
        c.step(70.0);
        assert_eq!(c.step(50.0), 70);
        assert_eq!(c.step(50.0), 65);
    }

    #[test]
    fn floor() {
        let mut c = Controller::new(Curve::new(&[[40.0, 0.0], [90.0, 10.0]]).unwrap(), 4.0, 5.0, 20.0, 60.0);
        assert_eq!(c.step(65.0), 20);
        assert!(c.step(45.0) < 20);
    }
}
