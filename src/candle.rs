//! Trade-driven candle engine.
//!
//! Maintains 1-minute base bars (the "native" resolution) and exposes an
//! aggregation API that re-bucketizes them into any of the supported chart
//! timeframes. VWAP and cumulative delta are computed from individual trades
//! so they remain correct regardless of timeframe.

use std::collections::VecDeque;

use crate::delta::Side;
use crate::feed::Trade;

pub const BASE_BAR_MS: i64 = 60_000;
pub const HISTORY_BARS: usize = 1500; // ~25 hours at 1-minute resolution

/// Supported chart timeframes (in ms).
pub const TIMEFRAMES_MS: [i64; 5] = [
    60_000,         // 1m
    180_000,        // 3m
    300_000,        // 5m
    900_000,        // 15m
    3_600_000,      // 1h
];

#[derive(Debug, Clone, Copy)]
pub struct CandleBar {
    pub start_ms: i64,
    pub end_ms: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub buy_volume: f64,
    pub sell_volume: f64,
    /// Sum of trade.price * trade.qty within this bar (for higher-TF VWAP rollup).
    pub price_volume: f64,
}

impl CandleBar {
    pub fn new(start_ms: i64, end_ms: i64, first_price: f64) -> Self {
        Self {
            start_ms,
            end_ms,
            open: first_price,
            high: first_price,
            low: first_price,
            close: first_price,
            buy_volume: 0.0,
            sell_volume: 0.0,
            price_volume: 0.0,
        }
    }

    pub fn total_volume(&self) -> f64 {
        self.buy_volume + self.sell_volume
    }

    pub fn delta(&self) -> f64 {
        self.buy_volume - self.sell_volume
    }

    pub fn typical_price(&self) -> f64 {
        (self.high + self.low + self.close) / 3.0
    }

    /// VWAP within this bar alone (price-volume / volume).
    pub fn bar_vwap(&self) -> Option<f64> {
        let v = self.total_volume();
        if v <= 0.0 { None } else { Some(self.price_volume / v) }
    }

    fn record_trade(&mut self, price: f64, qty: f64, side: Side) {
        self.close = price;
        if price > self.high { self.high = price; }
        if price < self.low { self.low = price; }
        match side {
            Side::Buy => self.buy_volume += qty,
            Side::Sell => self.sell_volume += qty,
        }
        self.price_volume += price * qty;
    }
}

/// Heikin Ashi candle — derived from raw OHLC; never used to feed indicators.
#[derive(Debug, Clone, Copy)]
pub struct HeikinAshi {
    pub start_ms: i64,
    pub end_ms: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

/// Convert a series of standard OHLC bars to Heikin Ashi.
///
/// Definition:
///   HA-Close = (O + H + L + C) / 4
///   HA-Open  = (prev HA-Open + prev HA-Close) / 2  (seeded to (O+C)/2 for the first bar)
///   HA-High  = max(H, HA-Open, HA-Close)
///   HA-Low   = min(L, HA-Open, HA-Close)
pub fn heikin_ashi_series(bars: &[CandleBar]) -> Vec<HeikinAshi> {
    let mut out = Vec::with_capacity(bars.len());
    let mut prev_open: Option<f64> = None;
    let mut prev_close: Option<f64> = None;
    for b in bars {
        let ha_close = (b.open + b.high + b.low + b.close) / 4.0;
        let ha_open = match (prev_open, prev_close) {
            (Some(po), Some(pc)) => (po + pc) / 2.0,
            _ => (b.open + b.close) / 2.0,
        };
        let ha_high = b.high.max(ha_open).max(ha_close);
        let ha_low = b.low.min(ha_open).min(ha_close);
        out.push(HeikinAshi {
            start_ms: b.start_ms,
            end_ms: b.end_ms,
            open: ha_open,
            high: ha_high,
            low: ha_low,
            close: ha_close,
        });
        prev_open = Some(ha_open);
        prev_close = Some(ha_close);
    }
    out
}

/// Trade-driven engine. Owns the base 1-minute bars and the running
/// session VWAP + cumulative delta values that are computed from
/// raw trades (never from candles).
#[derive(Debug, Clone)]
pub struct CandleEngine {
    base_bar_ms: i64,
    completed: VecDeque<CandleBar>,
    forming: Option<CandleBar>,

    // Session-wide running totals (from raw trades).
    sum_pv: f64,    // Σ price * qty
    sum_v: f64,     // Σ qty
    sum_pv2: f64,   // Σ price^2 * qty (for VWAP std dev)
    cum_delta: f64, // Σ buy - sell

    // Trail of running cumulative delta sampled at bar close — used for
    // divergence detection alongside completed bars.
    cum_delta_at_close: VecDeque<f64>,
}

impl Default for CandleEngine {
    fn default() -> Self {
        Self::new(BASE_BAR_MS)
    }
}

impl CandleEngine {
    pub fn new(base_bar_ms: i64) -> Self {
        assert!(base_bar_ms > 0);
        Self {
            base_bar_ms,
            completed: VecDeque::with_capacity(HISTORY_BARS + 1),
            forming: None,
            sum_pv: 0.0,
            sum_v: 0.0,
            sum_pv2: 0.0,
            cum_delta: 0.0,
            cum_delta_at_close: VecDeque::with_capacity(HISTORY_BARS + 1),
        }
    }

    pub fn base_bar_ms(&self) -> i64 {
        self.base_bar_ms
    }

    pub fn record(&mut self, trade: &Trade) {
        let price: f64 = trade.price.try_into().unwrap_or(0.0);
        let qty: f64 = trade.qty.try_into().unwrap_or(0.0);
        if qty <= 0.0 || !price.is_finite() {
            return;
        }

        let bar_start = bar_start_for(trade.time_ms, self.base_bar_ms);
        let bar_end = bar_start + self.base_bar_ms;

        // 1. Seal the previous bar BEFORE this trade is mixed into running
        //    totals — the cum_delta-at-close trail must capture the value as
        //    it stood at the moment the bar closed, not afterwards.
        let needs_seal = match self.forming.as_ref() {
            Some(b) if bar_start < b.start_ms => return, // out-of-order older trade
            Some(b) if bar_start > b.start_ms => true,
            _ => false,
        };
        if needs_seal {
            let old = self.forming.take().expect("forming present");
            self.completed.push_back(old);
            self.cum_delta_at_close.push_back(self.cum_delta);
            while self.completed.len() > HISTORY_BARS {
                self.completed.pop_front();
                self.cum_delta_at_close.pop_front();
            }
        }

        // 2. Apply this trade to session running totals.
        self.sum_pv += price * qty;
        self.sum_v += qty;
        self.sum_pv2 += price * price * qty;
        self.cum_delta += match trade.side {
            Side::Buy => qty,
            Side::Sell => -qty,
        };

        // 3. Apply to the (new or existing) forming bar.
        let bar = self
            .forming
            .get_or_insert_with(|| CandleBar::new(bar_start, bar_end, price));
        bar.record_trade(price, qty, trade.side);
    }

    pub fn completed(&self) -> &VecDeque<CandleBar> {
        &self.completed
    }

    pub fn forming(&self) -> Option<&CandleBar> {
        self.forming.as_ref()
    }

    pub fn cum_delta(&self) -> f64 {
        self.cum_delta
    }

    /// Trail of cumulative delta sampled at each base bar close.
    /// Same length as `completed()` — index aligned.
    pub fn cum_delta_trail(&self) -> &VecDeque<f64> {
        &self.cum_delta_at_close
    }

    /// Session VWAP from raw trades. None until at least one trade.
    pub fn session_vwap(&self) -> Option<f64> {
        if self.sum_v <= 0.0 { None } else { Some(self.sum_pv / self.sum_v) }
    }

    /// Trade-volume-weighted standard deviation of price across the
    /// session. Used for VWAP bands. None until at least one trade.
    pub fn session_vwap_stddev(&self) -> Option<f64> {
        if self.sum_v <= 0.0 { return None; }
        let mean = self.sum_pv / self.sum_v;
        let var = (self.sum_pv2 / self.sum_v) - mean * mean;
        if var.is_sign_negative() { Some(0.0) } else { Some(var.sqrt()) }
    }

    pub fn reset_session(&mut self) {
        self.sum_pv = 0.0;
        self.sum_v = 0.0;
        self.sum_pv2 = 0.0;
        self.cum_delta = 0.0;
    }

    pub fn reset_all(&mut self) {
        self.completed.clear();
        self.forming = None;
        self.cum_delta_at_close.clear();
        self.reset_session();
    }

    /// Aggregate base 1-minute bars into the requested timeframe.
    /// Returns chronological completed bars + the in-progress aggregated
    /// "forming" bar (if any data lies in the open window).
    pub fn aggregate(&self, tf_ms: i64) -> AggregatedSeries {
        assert!(tf_ms >= self.base_bar_ms && tf_ms % self.base_bar_ms == 0);
        let mut completed: Vec<CandleBar> = Vec::new();
        let mut current: Option<CandleBar> = None;

        let push_or_extend = |current: &mut Option<CandleBar>, bar: &CandleBar, tf: i64, completed: &mut Vec<CandleBar>| {
            let bs = bar_start_for(bar.start_ms, tf);
            let be = bs + tf;
            match current.as_mut() {
                None => {
                    let mut nb = CandleBar::new(bs, be, bar.open);
                    nb.high = bar.high;
                    nb.low = bar.low;
                    nb.close = bar.close;
                    nb.buy_volume = bar.buy_volume;
                    nb.sell_volume = bar.sell_volume;
                    nb.price_volume = bar.price_volume;
                    *current = Some(nb);
                }
                Some(c) if c.start_ms == bs => {
                    if bar.high > c.high { c.high = bar.high; }
                    if bar.low < c.low { c.low = bar.low; }
                    c.close = bar.close;
                    c.buy_volume += bar.buy_volume;
                    c.sell_volume += bar.sell_volume;
                    c.price_volume += bar.price_volume;
                }
                Some(_) => {
                    let prev = current.take().expect("current present");
                    completed.push(prev);
                    let mut nb = CandleBar::new(bs, be, bar.open);
                    nb.high = bar.high;
                    nb.low = bar.low;
                    nb.close = bar.close;
                    nb.buy_volume = bar.buy_volume;
                    nb.sell_volume = bar.sell_volume;
                    nb.price_volume = bar.price_volume;
                    *current = Some(nb);
                }
            }
        };

        for bar in self.completed.iter() {
            push_or_extend(&mut current, bar, tf_ms, &mut completed);
        }
        if let Some(forming) = self.forming.as_ref() {
            push_or_extend(&mut current, forming, tf_ms, &mut completed);
        }
        AggregatedSeries { completed, forming: current, tf_ms }
    }
}

#[derive(Debug, Clone)]
pub struct AggregatedSeries {
    pub completed: Vec<CandleBar>,
    pub forming: Option<CandleBar>,
    pub tf_ms: i64,
}

impl AggregatedSeries {
    pub fn iter(&self) -> impl Iterator<Item = &CandleBar> {
        self.completed.iter().chain(self.forming.as_ref())
    }

    pub fn len(&self) -> usize {
        self.completed.len() + if self.forming.is_some() { 1 } else { 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.completed.is_empty() && self.forming.is_none()
    }
}

fn bar_start_for(time_ms: i64, bar_ms: i64) -> i64 {
    if time_ms >= 0 {
        (time_ms / bar_ms) * bar_ms
    } else {
        ((time_ms - (bar_ms - 1)) / bar_ms) * bar_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    fn t(time_ms: i64, price: Decimal, qty: Decimal, side: Side) -> Trade {
        Trade { time_ms, price, qty, side }
    }

    fn approx(a: f64, b: f64, tol: f64) {
        assert!((a - b).abs() <= tol, "expected {b} got {a}");
    }

    #[test]
    fn vwap_matches_manual_calculation() {
        // Three trades, VWAP = Σ p*v / Σ v.
        let mut e = CandleEngine::default();
        e.record(&t(0, dec!(100), dec!(2), Side::Buy));
        e.record(&t(1000, dec!(102), dec!(3), Side::Buy));
        e.record(&t(2000, dec!(101), dec!(1), Side::Sell));
        // (100*2 + 102*3 + 101*1) / 6 = (200 + 306 + 101) / 6 = 607/6 ≈ 101.166...
        approx(e.session_vwap().unwrap(), 607.0 / 6.0, 1e-9);
    }

    #[test]
    fn cumulative_delta_runs_across_bars() {
        let mut e = CandleEngine::default();
        e.record(&t(0, dec!(100), dec!(2), Side::Buy));
        e.record(&t(70_000, dec!(101), dec!(1), Side::Sell));
        e.record(&t(140_000, dec!(100), dec!(0.5), Side::Buy));
        approx(e.cum_delta(), 2.0 - 1.0 + 0.5, 1e-9);
    }

    #[test]
    fn one_minute_bar_seals_on_minute_boundary() {
        let mut e = CandleEngine::default();
        e.record(&t(0, dec!(100), dec!(1), Side::Buy));
        e.record(&t(60_000, dec!(101), dec!(1), Side::Sell));
        assert_eq!(e.completed().len(), 1);
        let prev = e.completed().front().unwrap();
        assert_eq!(prev.start_ms, 0);
        assert_eq!(prev.end_ms, 60_000);
        approx(prev.open, 100.0, 0.0);
        approx(prev.close, 100.0, 0.0);
        let cur = e.forming().unwrap();
        assert_eq!(cur.start_ms, 60_000);
        approx(cur.open, 101.0, 0.0);
    }

    #[test]
    fn aggregate_to_5_minutes_groups_correctly() {
        let mut e = CandleEngine::default();
        // 6 minutes of one-trade-per-minute (different prices).
        for i in 0..6 {
            e.record(&t(i * 60_000, dec!(100) + Decimal::from(i), dec!(1), Side::Buy));
        }
        // 6 base bars: 5 completed + 1 forming.
        assert_eq!(e.completed().len(), 5);
        assert!(e.forming().is_some());

        let agg = e.aggregate(5 * 60_000);
        // Completed bars 0..5 form the first 5m bucket → 1 completed.
        // Forming base bar at minute 5 → forming 5m bucket starting at 5*60_000.
        assert_eq!(agg.completed.len(), 1);
        let c = &agg.completed[0];
        assert_eq!(c.start_ms, 0);
        assert_eq!(c.end_ms, 5 * 60_000);
        approx(c.open, 100.0, 0.0);
        approx(c.high, 104.0, 0.0);
        approx(c.low, 100.0, 0.0);
        approx(c.close, 104.0, 0.0);
        approx(c.buy_volume, 5.0, 0.0);
    }

    #[test]
    fn aggregate_total_volume_equals_base_total_volume() {
        let mut e = CandleEngine::default();
        let trades: Vec<(i64, Decimal, Decimal, Side)> = vec![
            (0, dec!(100), dec!(1.5), Side::Buy),
            (30_000, dec!(101), dec!(0.5), Side::Sell),
            (90_000, dec!(102), dec!(2.0), Side::Buy),
            (180_000, dec!(99), dec!(1.0), Side::Sell),
        ];
        for (ts, p, q, s) in &trades {
            e.record(&t(*ts, *p, *q, *s));
        }
        let base_total: f64 = e.completed().iter().map(|b| b.total_volume()).sum::<f64>()
            + e.forming().map(|b| b.total_volume()).unwrap_or(0.0);
        let agg = e.aggregate(15 * 60_000);
        let agg_total: f64 = agg.iter().map(|b| b.total_volume()).sum();
        approx(agg_total, base_total, 1e-9);
        approx(agg_total, 5.0, 1e-9);
    }

    #[test]
    fn session_vwap_unchanged_by_timeframe() {
        let mut e = CandleEngine::default();
        e.record(&t(0, dec!(100), dec!(2), Side::Buy));
        e.record(&t(60_000, dec!(102), dec!(2), Side::Sell));
        e.record(&t(120_000, dec!(101), dec!(2), Side::Buy));
        let vwap = e.session_vwap().unwrap();
        // Aggregation is a *view* — VWAP is a property of the engine and
        // must not change when re-aggregating.
        let _ = e.aggregate(60_000);
        let _ = e.aggregate(180_000);
        let _ = e.aggregate(900_000);
        approx(e.session_vwap().unwrap(), vwap, 1e-12);
    }

    #[test]
    fn vwap_stddev_zero_for_constant_price() {
        let mut e = CandleEngine::default();
        for i in 0..10 {
            e.record(&t(i * 1000, dec!(100), dec!(1), Side::Buy));
        }
        approx(e.session_vwap_stddev().unwrap(), 0.0, 1e-9);
    }

    #[test]
    fn heikin_ashi_first_bar_seeds_correctly() {
        let bars = vec![
            CandleBar { start_ms: 0, end_ms: 60_000, open: 100.0, high: 105.0, low: 99.0, close: 104.0, buy_volume: 1.0, sell_volume: 0.0, price_volume: 0.0 },
        ];
        let ha = heikin_ashi_series(&bars);
        // HA-Open seed = (O+C)/2 = (100+104)/2 = 102.0
        // HA-Close = (100+105+99+104)/4 = 102.0
        approx(ha[0].open, 102.0, 1e-12);
        approx(ha[0].close, 102.0, 1e-12);
        approx(ha[0].high, 105.0, 1e-12);
        approx(ha[0].low, 99.0, 1e-12);
    }

    #[test]
    fn heikin_ashi_second_bar_uses_prior_open_close() {
        let bars = vec![
            CandleBar { start_ms: 0, end_ms: 60_000, open: 100.0, high: 105.0, low: 99.0, close: 104.0, buy_volume: 0.0, sell_volume: 0.0, price_volume: 0.0 },
            CandleBar { start_ms: 60_000, end_ms: 120_000, open: 104.0, high: 110.0, low: 103.0, close: 108.0, buy_volume: 0.0, sell_volume: 0.0, price_volume: 0.0 },
        ];
        let ha = heikin_ashi_series(&bars);
        // ha[0].open = 102, ha[0].close = 102 (from previous test).
        // ha[1].open = (102 + 102)/2 = 102.0
        // ha[1].close = (104 + 110 + 103 + 108)/4 = 425/4 = 106.25
        approx(ha[1].open, 102.0, 1e-12);
        approx(ha[1].close, 106.25, 1e-12);
        approx(ha[1].high, 110.0_f64.max(102.0).max(106.25), 1e-12); // 110
        approx(ha[1].low, 103.0_f64.min(102.0).min(106.25), 1e-12);  // 102
    }

    #[test]
    fn out_of_order_older_trade_is_dropped() {
        let mut e = CandleEngine::default();
        e.record(&t(120_000, dec!(100), dec!(1), Side::Buy));
        // Older trade arrives after newer one — must NOT contaminate the
        // current bar or rewrite history.
        e.record(&t(30_000, dec!(99), dec!(5), Side::Sell));
        let cur = e.forming().unwrap();
        assert_eq!(cur.start_ms, 120_000);
        approx(cur.buy_volume, 1.0, 0.0);
        approx(cur.sell_volume, 0.0, 0.0);
        assert!(e.completed().is_empty());
    }

    #[test]
    fn cum_delta_trail_aligns_with_completed_bars() {
        let mut e = CandleEngine::default();
        e.record(&t(0, dec!(100), dec!(2), Side::Buy));
        e.record(&t(60_000, dec!(100), dec!(1), Side::Sell));
        e.record(&t(120_000, dec!(100), dec!(3), Side::Buy));
        // Two completed bars (minute 0 and minute 1).
        // After minute 0 closes: cum_delta = +2.
        // After minute 1 closes: cum_delta = +2 - 1 = +1.
        assert_eq!(e.completed().len(), 2);
        let trail: Vec<f64> = e.cum_delta_trail().iter().copied().collect();
        approx(trail[0], 2.0, 1e-9);
        approx(trail[1], 1.0, 1e-9);
    }

    #[test]
    fn reset_all_clears_engine_state() {
        let mut e = CandleEngine::default();
        e.record(&t(0, dec!(100), dec!(2), Side::Buy));
        e.reset_all();
        assert!(e.completed().is_empty());
        assert!(e.forming().is_none());
        assert!(e.session_vwap().is_none());
        approx(e.cum_delta(), 0.0, 0.0);
    }
}
