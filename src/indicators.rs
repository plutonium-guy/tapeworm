//! Incremental indicator implementations: EMA, RSI (Wilder), MACD, and the
//! Delta Momentum Oscillator. All operate on `f64` because terminal display
//! and the smoothing math don't require Decimal precision (and Decimal would
//! drag in extra cost). Reference values are verified in tests.

#[derive(Debug, Clone)]
pub struct Ema {
    period: usize,
    alpha: f64,
    seed: Vec<f64>,
    value: Option<f64>,
}

impl Ema {
    pub fn new(period: usize) -> Self {
        assert!(period >= 1, "EMA period must be ≥ 1");
        Self {
            period,
            alpha: 2.0 / (period as f64 + 1.0),
            seed: Vec::with_capacity(period),
            value: None,
        }
    }

    pub fn period(&self) -> usize {
        self.period
    }

    pub fn alpha(&self) -> f64 {
        self.alpha
    }

    /// Push a new sample; returns the EMA after this sample (None until
    /// the seed window has filled to `period` samples).
    pub fn update(&mut self, x: f64) -> Option<f64> {
        match self.value {
            None => {
                self.seed.push(x);
                if self.seed.len() == self.period {
                    let mean: f64 = self.seed.iter().copied().sum::<f64>() / self.period as f64;
                    self.value = Some(mean);
                    self.seed.clear();
                }
                self.value
            }
            Some(v) => {
                let nv = self.alpha * x + (1.0 - self.alpha) * v;
                self.value = Some(nv);
                self.value
            }
        }
    }

    pub fn value(&self) -> Option<f64> {
        self.value
    }

    pub fn reset(&mut self) {
        self.seed.clear();
        self.value = None;
    }
}

/// Wilder's RSI implementation (the textbook RSI most platforms use).
#[derive(Debug, Clone)]
pub struct Rsi {
    period: usize,
    last_close: Option<f64>,
    seed_gain: f64,
    seed_loss: f64,
    seed_count: usize,
    avg_gain: Option<f64>,
    avg_loss: Option<f64>,
    last_value: Option<f64>,
}

impl Rsi {
    pub fn new(period: usize) -> Self {
        assert!(period >= 2, "RSI period must be ≥ 2");
        Self {
            period,
            last_close: None,
            seed_gain: 0.0,
            seed_loss: 0.0,
            seed_count: 0,
            avg_gain: None,
            avg_loss: None,
            last_value: None,
        }
    }

    pub fn period(&self) -> usize {
        self.period
    }

    /// Push a new close price; returns RSI value (None until enough data).
    pub fn update(&mut self, close: f64) -> Option<f64> {
        let last = match self.last_close {
            None => {
                self.last_close = Some(close);
                return None;
            }
            Some(l) => l,
        };
        let change = close - last;
        let gain = change.max(0.0);
        let loss = (-change).max(0.0);
        self.last_close = Some(close);

        match (self.avg_gain, self.avg_loss) {
            (Some(ag), Some(al)) => {
                let p = self.period as f64;
                let nag = (ag * (p - 1.0) + gain) / p;
                let nal = (al * (p - 1.0) + loss) / p;
                self.avg_gain = Some(nag);
                self.avg_loss = Some(nal);
                let value = if nal == 0.0 {
                    100.0
                } else if nag == 0.0 {
                    0.0
                } else {
                    let rs = nag / nal;
                    100.0 - 100.0 / (1.0 + rs)
                };
                self.last_value = Some(value);
                Some(value)
            }
            _ => {
                self.seed_gain += gain;
                self.seed_loss += loss;
                self.seed_count += 1;
                if self.seed_count == self.period {
                    let p = self.period as f64;
                    let ag = self.seed_gain / p;
                    let al = self.seed_loss / p;
                    self.avg_gain = Some(ag);
                    self.avg_loss = Some(al);
                    let value = if al == 0.0 {
                        100.0
                    } else if ag == 0.0 {
                        0.0
                    } else {
                        let rs = ag / al;
                        100.0 - 100.0 / (1.0 + rs)
                    };
                    self.last_value = Some(value);
                    Some(value)
                } else {
                    None
                }
            }
        }
    }

    pub fn value(&self) -> Option<f64> {
        self.last_value
    }

    pub fn reset(&mut self) {
        self.last_close = None;
        self.seed_gain = 0.0;
        self.seed_loss = 0.0;
        self.seed_count = 0;
        self.avg_gain = None;
        self.avg_loss = None;
        self.last_value = None;
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MacdValue {
    pub macd: f64,
    pub signal: f64,
    pub histogram: f64,
}

/// Standard MACD: fast/slow EMA difference plus signal-line EMA.
#[derive(Debug, Clone)]
pub struct Macd {
    fast: Ema,
    slow: Ema,
    signal: Ema,
    last: Option<MacdValue>,
}

impl Macd {
    pub fn new(fast: usize, slow: usize, signal: usize) -> Self {
        assert!(fast < slow, "MACD fast period must be < slow period");
        Self {
            fast: Ema::new(fast),
            slow: Ema::new(slow),
            signal: Ema::new(signal),
            last: None,
        }
    }

    pub fn update(&mut self, close: f64) -> Option<MacdValue> {
        let fast = self.fast.update(close);
        let slow = self.slow.update(close);
        if let (Some(f), Some(s)) = (fast, slow) {
            let macd_line = f - s;
            if let Some(sig) = self.signal.update(macd_line) {
                let v = MacdValue {
                    macd: macd_line,
                    signal: sig,
                    histogram: macd_line - sig,
                };
                self.last = Some(v);
                return Some(v);
            }
        }
        None
    }

    pub fn value(&self) -> Option<MacdValue> {
        self.last
    }

    pub fn reset(&mut self) {
        self.fast.reset();
        self.slow.reset();
        self.signal.reset();
        self.last = None;
    }
}

/// Delta Momentum Oscillator: rate-of-change of cumulative delta over `period`
/// samples. A simple `(current - prev_period_ago) / period` rate suffices for
/// the visualisation; sign is what matters most.
#[derive(Debug, Clone)]
pub struct DeltaMomentum {
    period: usize,
    history: std::collections::VecDeque<f64>,
}

impl DeltaMomentum {
    pub fn new(period: usize) -> Self {
        assert!(period >= 1);
        Self {
            period,
            history: std::collections::VecDeque::with_capacity(period + 1),
        }
    }

    pub fn period(&self) -> usize {
        self.period
    }

    /// Pushes the latest cumulative-delta value; returns rate-of-change
    /// over the trailing `period` samples (None until enough samples).
    pub fn update(&mut self, cum_delta: f64) -> Option<f64> {
        self.history.push_back(cum_delta);
        if self.history.len() > self.period + 1 {
            self.history.pop_front();
        }
        if self.history.len() == self.period + 1 {
            let oldest = *self.history.front().unwrap();
            let newest = *self.history.back().unwrap();
            Some((newest - oldest) / self.period as f64)
        } else {
            None
        }
    }

    pub fn reset(&mut self) {
        self.history.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) {
        assert!(
            (a - b).abs() <= tol,
            "expected {b} got {a} (Δ={:.6})",
            (a - b).abs()
        );
    }

    #[test]
    fn ema_seeds_with_simple_average() {
        let mut e = Ema::new(3);
        assert_eq!(e.update(1.0), None);
        assert_eq!(e.update(2.0), None);
        assert_eq!(e.update(3.0), Some(2.0)); // SMA(1,2,3) = 2.0
        // alpha = 2/(3+1) = 0.5
        approx(e.update(4.0).unwrap(), 0.5 * 4.0 + 0.5 * 2.0, 1e-12); // 3.0
        approx(e.update(5.0).unwrap(), 0.5 * 5.0 + 0.5 * 3.0, 1e-12); // 4.0
    }

    #[test]
    fn ema_alpha_for_period_9() {
        let e = Ema::new(9);
        approx(e.alpha(), 2.0 / 10.0, 1e-12);
    }

    #[test]
    fn ema_constant_input_returns_constant() {
        let mut e = Ema::new(10);
        let mut last = None;
        for _ in 0..30 {
            last = e.update(50.0);
        }
        approx(last.unwrap(), 50.0, 1e-9);
    }

    #[test]
    fn rsi_classic_textbook_sequence() {
        // From Wilder's "New Concepts in Technical Trading Systems" (1978),
        // table of 14-period RSI computed from a known closing-price series.
        // Source: cuttingedgesfx.com / many TA references.
        let closes: [f64; 16] = [
            44.34, 44.09, 44.15, 43.61, 44.33, 44.83, 45.10, 45.42, 45.84, 46.08,
            45.89, 46.03, 45.61, 46.28, 46.28, 46.00,
        ];
        let mut rsi = Rsi::new(14);
        let mut values: Vec<Option<f64>> = closes.iter().map(|c| rsi.update(*c)).collect();
        // First 14 returns are None (need 14 changes after seeding).
        // Index 14 is the first RSI value.
        let first = values[14].take().expect("RSI defined at index 14");
        let second = values[15].take().expect("RSI defined at index 15");
        // Reference values (Wilder's smoothing): ~70.46 then ~66.25.
        approx(first, 70.46, 0.5);
        approx(second, 66.25, 0.5);
    }

    #[test]
    fn rsi_constant_price_is_neutral_or_undefined() {
        // No gains and no losses → avg_loss=0, avg_gain=0. Implementation
        // treats this as 100 (no losses). Test documents the contract.
        let mut rsi = Rsi::new(14);
        for _ in 0..20 {
            rsi.update(100.0);
        }
        let v = rsi.value().expect("RSI defined");
        // With both zero, our implementation returns 100. This matches
        // most popular trading platforms (e.g. TradingView).
        approx(v, 100.0, 0.0);
    }

    #[test]
    fn rsi_only_gains_clamps_to_100() {
        let mut rsi = Rsi::new(14);
        for i in 1..=20 {
            rsi.update(i as f64);
        }
        approx(rsi.value().unwrap(), 100.0, 1e-9);
    }

    #[test]
    fn macd_zero_for_constant_input() {
        let mut m = Macd::new(12, 26, 9);
        let mut last = None;
        for _ in 0..60 {
            last = m.update(100.0);
        }
        let v = last.expect("MACD defined");
        approx(v.macd, 0.0, 1e-9);
        approx(v.signal, 0.0, 1e-9);
        approx(v.histogram, 0.0, 1e-9);
    }

    #[test]
    fn macd_returns_some_only_after_warmup() {
        let mut m = Macd::new(3, 6, 2);
        // slow=6 EMA needs 6 samples to seed, then signal EMA needs 2 MACD
        // values. So MACD is defined from sample 7 onward.
        for i in 1..=6 {
            assert!(m.update(i as f64).is_none(), "MACD too early at i={i}");
        }
        assert!(m.update(7.0).is_some());
    }

    #[test]
    fn delta_momentum_simple_slope() {
        let mut dmo = DeltaMomentum::new(4);
        // cumulative delta: 0, 1, 3, 6, 10
        assert!(dmo.update(0.0).is_none());
        assert!(dmo.update(1.0).is_none());
        assert!(dmo.update(3.0).is_none());
        assert!(dmo.update(6.0).is_none());
        let v = dmo.update(10.0).expect("DMO defined");
        // (10 - 0) / 4 = 2.5
        approx(v, 2.5, 1e-12);
    }

    #[test]
    fn ema_reset_starts_over() {
        let mut e = Ema::new(3);
        e.update(1.0);
        e.update(2.0);
        e.update(3.0);
        e.reset();
        assert!(e.update(10.0).is_none());
        assert!(e.update(20.0).is_none());
        assert_eq!(e.update(30.0), Some(20.0));
    }
}
