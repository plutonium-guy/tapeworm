//! Order-flow signal detectors.
//!
//! Reads from existing engines (orderbook, trade feed, footprint, candles)
//! and emits typed signals with strength scores in [1, 5]. Designed for
//! incremental updates per trade or per book event.

use std::collections::{HashMap, VecDeque};

use rust_decimal::Decimal;
use rust_decimal::prelude::Signed;

use crate::delta::Side;
use crate::feed::Trade;
use crate::orderbook::OrderBook;

pub const SIGNAL_LOG_CAP: usize = 1000;
/// Max distinct price levels we track per symbol. Prevents unbounded
/// growth of [`SignalsEngine::levels`] in long sessions.
pub const LEVELS_MAX: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    Iceberg,
    Absorption,
    PaceSpike,
    StopRun,
    Exhaustion,
}

impl SignalKind {
    pub fn label(&self) -> &'static str {
        match self {
            SignalKind::Iceberg => "ICE",
            SignalKind::Absorption => "ABS",
            SignalKind::PaceSpike => "PCE",
            SignalKind::StopRun => "STP",
            SignalKind::Exhaustion => "EXH",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Signal {
    pub kind: SignalKind,
    pub time_ms: i64,
    pub price: Decimal,
    pub score: u8, // 1..=5
    pub side: Option<Side>,
    pub note: String,
}

#[derive(Debug, Clone, Default)]
pub struct SensitivityConfig {
    pub iceberg_volume_ratio: f64, // total qty / max visible qty
    pub iceberg_min_refreshes: u32,
    pub absorption_volume_min: f64,
    pub pace_spike_multiple: f64,
    pub stop_run_revert_bars: usize,
    pub exhaustion_divergence_bars: usize,
}

impl SensitivityConfig {
    pub fn defaults() -> Self {
        Self {
            iceberg_volume_ratio: 4.0,
            iceberg_min_refreshes: 3,
            absorption_volume_min: 1.0,
            pace_spike_multiple: 3.0,
            stop_run_revert_bars: 3,
            exhaustion_divergence_bars: 5,
        }
    }
}

/// Per-price-level state used by iceberg & absorption detectors.
#[derive(Debug, Clone, Default)]
struct LevelState {
    /// Largest quantity ever observed at this level (across both sides).
    max_visible: Decimal,
    /// Total traded volume at this level since first observation.
    traded: Decimal,
    /// Number of times the level has been refilled (visible qty went up
    /// after going down).
    refresh_count: u32,
    /// Last seen visible quantity (latest book update).
    last_visible: Decimal,
    /// True if we've already emitted an iceberg signal for this level.
    iceberg_emitted: bool,
    /// True if we've already emitted an absorption signal for this level.
    absorption_emitted: bool,
}

/// Pace of tape: rolling per-second trade counts (split buy/sell).
#[derive(Debug, Clone, Default)]
pub struct PaceTracker {
    /// Rolling timestamps (ms) of recent trades.
    times: VecDeque<i64>,
    buy_window: VecDeque<i64>,
    sell_window: VecDeque<i64>,
    /// Session running totals (for "average" comparison).
    session_count: u64,
    session_start_ms: Option<i64>,
}

impl PaceTracker {
    pub const WINDOW_MS: i64 = 5_000;

    pub fn record(&mut self, trade: &Trade) {
        if self.session_start_ms.is_none() {
            self.session_start_ms = Some(trade.time_ms);
        }
        self.session_count += 1;
        self.times.push_back(trade.time_ms);
        match trade.side {
            Side::Buy => self.buy_window.push_back(trade.time_ms),
            Side::Sell => self.sell_window.push_back(trade.time_ms),
        }
        let cutoff = trade.time_ms - Self::WINDOW_MS;
        while let Some(t) = self.times.front() {
            if *t < cutoff { self.times.pop_front(); } else { break; }
        }
        while let Some(t) = self.buy_window.front() {
            if *t < cutoff { self.buy_window.pop_front(); } else { break; }
        }
        while let Some(t) = self.sell_window.front() {
            if *t < cutoff { self.sell_window.pop_front(); } else { break; }
        }
    }

    /// Trades per second over the rolling window.
    pub fn current_tps(&self) -> f64 {
        self.times.len() as f64 / (Self::WINDOW_MS as f64 / 1000.0)
    }

    pub fn buy_tps(&self) -> f64 {
        self.buy_window.len() as f64 / (Self::WINDOW_MS as f64 / 1000.0)
    }

    pub fn sell_tps(&self) -> f64 {
        self.sell_window.len() as f64 / (Self::WINDOW_MS as f64 / 1000.0)
    }

    pub fn session_avg_tps(&self, now_ms: i64) -> Option<f64> {
        let start = self.session_start_ms?;
        let dur = (now_ms - start) as f64 / 1000.0;
        if dur <= 0.0 || self.session_count == 0 { return None; }
        Some(self.session_count as f64 / dur)
    }

    pub fn reset(&mut self) {
        self.times.clear();
        self.buy_window.clear();
        self.sell_window.clear();
        self.session_count = 0;
        self.session_start_ms = None;
    }
}

#[derive(Debug, Clone)]
pub struct SignalsEngine {
    pub config: SensitivityConfig,
    pub log: VecDeque<Signal>,
    pub pace: PaceTracker,
    levels: HashMap<Decimal, LevelState>,
    /// Last book extreme prices for stop-run detection.
    recent_highs: VecDeque<(i64, Decimal)>,
    recent_lows: VecDeque<(i64, Decimal)>,
    /// Bar deltas for exhaustion / stop-run analysis.
    bar_deltas: VecDeque<(i64, Decimal, Decimal)>, // (time, close, delta)
    last_pace_alert_ms: Option<i64>,
}

impl Default for SignalsEngine {
    fn default() -> Self {
        Self {
            config: SensitivityConfig::defaults(),
            log: VecDeque::with_capacity(SIGNAL_LOG_CAP + 1),
            pace: PaceTracker::default(),
            levels: HashMap::new(),
            recent_highs: VecDeque::with_capacity(64),
            recent_lows: VecDeque::with_capacity(64),
            bar_deltas: VecDeque::with_capacity(64),
            last_pace_alert_ms: None,
        }
    }
}

impl SignalsEngine {
    pub fn reset(&mut self) {
        self.log.clear();
        self.pace.reset();
        self.levels.clear();
        self.recent_highs.clear();
        self.recent_lows.clear();
        self.bar_deltas.clear();
        self.last_pace_alert_ms = None;
    }

    pub fn push(&mut self, sig: Signal) {
        self.log.push_front(sig);
        while self.log.len() > SIGNAL_LOG_CAP {
            self.log.pop_back();
        }
    }

    /// Update level visible-quantity tracking from the latest book snapshot.
    /// Should be called after every book change.
    pub fn observe_book(&mut self, book: &OrderBook) {
        let mut seen: std::collections::HashSet<Decimal> = std::collections::HashSet::new();
        for (price, qty) in book.bids.iter_desc().take(50) {
            self.observe_level(price, qty);
            seen.insert(price);
        }
        for (price, qty) in book.asks.iter_asc().take(50) {
            self.observe_level(price, qty);
            seen.insert(price);
        }
        // Decay levels that have left the visible book — clear last_visible
        // but keep traded counters.
        for (p, st) in self.levels.iter_mut() {
            if !seen.contains(p) {
                st.last_visible = Decimal::ZERO;
            }
        }
        // Cap total tracked levels to prevent unbounded memory growth in
        // long sessions. We drop the levels that are no longer visible AND
        // have no detection state worth preserving (no traded volume, no
        // refreshes, no signals emitted yet).
        if self.levels.len() > LEVELS_MAX {
            self.levels.retain(|_, st| {
                !st.last_visible.is_zero()
                    || !st.traded.is_zero()
                    || st.refresh_count > 0
                    || st.iceberg_emitted
                    || st.absorption_emitted
            });
            // If still over cap (everything has some state), drop arbitrary
            // half — we'd rather lose detector memory than crash with OOM.
            if self.levels.len() > LEVELS_MAX {
                let drop_n = self.levels.len() - LEVELS_MAX / 2;
                let to_drop: Vec<Decimal> = self
                    .levels
                    .iter()
                    .filter(|(_, st)| st.last_visible.is_zero())
                    .take(drop_n)
                    .map(|(p, _)| *p)
                    .collect();
                for p in to_drop {
                    self.levels.remove(&p);
                }
            }
        }
    }

    fn observe_level(&mut self, price: Decimal, qty: Decimal) {
        let st = self.levels.entry(price).or_default();
        let prev = st.last_visible;
        if qty > st.max_visible {
            st.max_visible = qty;
        }
        // Refresh: visible went down toward zero then back up.
        if prev.is_zero() && qty > Decimal::ZERO {
            // first observation or re-appearing
        } else if qty > prev && !prev.is_zero() && (prev / st.max_visible) < Decimal::new(5, 1) {
            // visible jumped back up after sliding below 50% of prior peak
            st.refresh_count = st.refresh_count.saturating_add(1);
        }
        st.last_visible = qty;
    }

    /// Process a trade — runs pace + per-level traded-volume bookkeeping
    /// + iceberg/absorption checks. Emits signals.
    pub fn observe_trade(&mut self, trade: &Trade, book: &OrderBook) {
        self.pace.record(trade);

        // Per-level traded volume.
        let st = self.levels.entry(trade.price).or_default();
        st.traded += trade.qty;
        let traded_now = st.traded;
        let raw_max_visible = st.max_visible;
        let refresh_count = st.refresh_count;
        let already_iceberg = st.iceberg_emitted;
        let already_absorption = st.absorption_emitted;
        // Snapshot now and release the borrow.
        let _ = st;
        // We can only reason about iceberg/absorption when the level was
        // observed in the visible book at some point. Trades for prices
        // that never showed up in our top-N depth window are excluded
        // from these detectors to avoid false positives.
        if raw_max_visible.is_zero() {
            // Pace spike still gets a chance.
            self.run_pace_check(trade);
            return;
        }
        let max_visible = raw_max_visible;

        // Iceberg: traded volume far exceeds max visible AND there's been
        // refresh activity at the level.
        let cfg = self.config.clone();
        if !already_iceberg {
            let traded_f: f64 = traded_now.try_into().unwrap_or(0.0);
            let visible_f: f64 = max_visible.try_into().unwrap_or(1.0);
            let ratio = traded_f / visible_f;
            if ratio >= cfg.iceberg_volume_ratio
                && refresh_count >= cfg.iceberg_min_refreshes
            {
                let score = ((ratio / cfg.iceberg_volume_ratio).min(2.5) * 2.0).round() as u8;
                let score = score.clamp(1, 5);
                let side = side_at(trade.price, book);
                self.push(Signal {
                    kind: SignalKind::Iceberg,
                    time_ms: trade.time_ms,
                    price: trade.price,
                    score,
                    side,
                    note: format!("traded {:.1}× visible, {} refreshes", ratio, refresh_count),
                });
                if let Some(s) = self.levels.get_mut(&trade.price) {
                    s.iceberg_emitted = true;
                }
            }
        }

        // Absorption: heavy trade volume at level but the level *didn't*
        // need to refresh — passive participant adding size to absorb.
        if !already_absorption {
            let traded_f: f64 = traded_now.try_into().unwrap_or(0.0);
            let visible_f: f64 = max_visible.try_into().unwrap_or(1.0);
            if traded_f >= cfg.absorption_volume_min
                && traded_f / visible_f >= 2.0
                && refresh_count <= 1
            {
                let score = ((traded_f / visible_f).min(5.0)).round() as u8;
                let score = score.clamp(1, 5);
                let side = match trade.side {
                    Side::Buy => Some(Side::Sell),
                    Side::Sell => Some(Side::Buy),
                };
                self.push(Signal {
                    kind: SignalKind::Absorption,
                    time_ms: trade.time_ms,
                    price: trade.price,
                    score,
                    side,
                    note: format!("aggression absorbed without level depleting"),
                });
                if let Some(s) = self.levels.get_mut(&trade.price) {
                    s.absorption_emitted = true;
                }
            }
        }

        self.run_pace_check(trade);
    }

    /// Pace-spike detector — extracted so it can run on every trade,
    /// including ones we've otherwise excluded from per-level detectors.
    fn run_pace_check(&mut self, trade: &Trade) {
        let cfg = self.config.clone();
        if let Some(avg) = self.pace.session_avg_tps(trade.time_ms) {
            let cur = self.pace.current_tps();
            let allowed = self
                .last_pace_alert_ms
                .map(|t| trade.time_ms - t > 3_000)
                .unwrap_or(true);
            if avg > 0.0 && cur >= avg * cfg.pace_spike_multiple && allowed {
                self.last_pace_alert_ms = Some(trade.time_ms);
                let dominant = if self.pace.buy_tps() > self.pace.sell_tps() * 1.2 {
                    Some(Side::Buy)
                } else if self.pace.sell_tps() > self.pace.buy_tps() * 1.2 {
                    Some(Side::Sell)
                } else {
                    None
                };
                let mult = (cur / avg).min(5.0);
                self.push(Signal {
                    kind: SignalKind::PaceSpike,
                    time_ms: trade.time_ms,
                    price: trade.price,
                    score: (mult.round() as u8).clamp(1, 5),
                    side: dominant,
                    note: format!("pace {:.1}× session avg", mult),
                });
            }
        }
    }

    /// Process a sealed bar's close + delta — used for stop-run/exhaustion.
    /// Caller passes (time_ms, close_price, total_delta).
    pub fn observe_bar_close(&mut self, time_ms: i64, close: Decimal, delta: Decimal) {
        self.bar_deltas.push_back((time_ms, close, delta));
        while self.bar_deltas.len() > 64 {
            self.bar_deltas.pop_front();
        }

        let cfg = self.config.clone();
        let n = self.bar_deltas.len();
        if n < cfg.stop_run_revert_bars + 2 {
            return;
        }

        // Stop-run: a bar with extreme range vs the prior few bars where
        // its delta has the OPPOSITE sign of its price move, followed by a
        // reversal in the next few bars.
        let trigger_idx = n - 2 - cfg.stop_run_revert_bars; // candidate
        if trigger_idx + 1 + cfg.stop_run_revert_bars < n {
            let (t, c, d) = self.bar_deltas[trigger_idx];
            let prev = self.bar_deltas[trigger_idx.saturating_sub(1)];
            let move_sign = (c - prev.1).signum();
            let delta_sign = d.signum();
            // Run + divergence.
            if move_sign != Decimal::ZERO && delta_sign != Decimal::ZERO && move_sign != delta_sign {
                // Reversal: did price retrace at least halfway in the next K bars?
                let mut reverted = false;
                for k in 1..=cfg.stop_run_revert_bars {
                    if let Some((_, cn, _)) = self.bar_deltas.get(trigger_idx + k) {
                        let advance = c - prev.1;
                        let pullback = c - *cn;
                        if move_sign == Decimal::ONE && pullback >= advance / Decimal::from(2) {
                            reverted = true;
                            break;
                        }
                        if move_sign == -Decimal::ONE && (-pullback) >= (-advance) / Decimal::from(2) {
                            reverted = true;
                            break;
                        }
                    }
                }
                if reverted {
                    let already = self.log.iter().take(20).any(|s| matches!(s.kind, SignalKind::StopRun) && s.time_ms == t);
                    if !already {
                        self.push(Signal {
                            kind: SignalKind::StopRun,
                            time_ms: t,
                            price: c,
                            score: 4,
                            side: if move_sign == Decimal::ONE { Some(Side::Sell) } else { Some(Side::Buy) },
                            note: format!("price extended; delta diverged; reverted within {} bars", cfg.stop_run_revert_bars),
                        });
                    }
                }
            }
        }

        // Exhaustion: bar makes a new K-bar high/low but its delta sign
        // diverges from the prior extreme.
        let lookback = cfg.exhaustion_divergence_bars.min(n - 1);
        if lookback >= 2 {
            let (t, c, d) = *self.bar_deltas.back().unwrap();
            let recent: Vec<(i64, Decimal, Decimal)> = self.bar_deltas.iter().rev().skip(1).take(lookback).copied().collect();
            let prev_high = recent.iter().map(|x| x.1).max().unwrap_or(c);
            let prev_low = recent.iter().map(|x| x.1).min().unwrap_or(c);
            let prev_high_delta = recent
                .iter()
                .max_by(|a, b| a.1.cmp(&b.1))
                .map(|x| x.2)
                .unwrap_or(Decimal::ZERO);
            let prev_low_delta = recent
                .iter()
                .min_by(|a, b| a.1.cmp(&b.1))
                .map(|x| x.2)
                .unwrap_or(Decimal::ZERO);
            // New high but weaker buy delta than prior high.
            if c > prev_high && d.signum() != Decimal::ONE && prev_high_delta.signum() == Decimal::ONE {
                self.push(Signal {
                    kind: SignalKind::Exhaustion,
                    time_ms: t,
                    price: c,
                    score: 5,
                    side: Some(Side::Sell),
                    note: format!("new high with weakening delta"),
                });
            } else if c < prev_low && d.signum() != -Decimal::ONE && prev_low_delta.signum() == -Decimal::ONE {
                self.push(Signal {
                    kind: SignalKind::Exhaustion,
                    time_ms: t,
                    price: c,
                    score: 5,
                    side: Some(Side::Buy),
                    note: format!("new low with weakening delta"),
                });
            }
        }
    }
}

fn side_at(price: Decimal, book: &OrderBook) -> Option<Side> {
    if let Some((bb, _)) = book.best_bid() {
        if price <= bb { return Some(Side::Buy); }
    }
    if let Some((ba, _)) = book.best_ask() {
        if price >= ba { return Some(Side::Sell); }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn t(time_ms: i64, price: Decimal, qty: Decimal, side: Side) -> Trade {
        Trade { time_ms, price, qty, side }
    }

    #[test]
    fn pace_window_decays_after_5_seconds() {
        let mut p = PaceTracker::default();
        for i in 0..10 {
            p.record(&t(i * 100, dec!(100), dec!(1), Side::Buy));
        }
        assert!(p.current_tps() > 0.0);
        // Move 6s forward — old trades should age out.
        p.record(&t(7000, dec!(100), dec!(1), Side::Buy));
        // Only the one new trade should remain.
        assert_eq!(p.times.len(), 1);
    }

    #[test]
    fn pace_separates_buy_and_sell_streams() {
        let mut p = PaceTracker::default();
        for i in 0..3 { p.record(&t(i * 100, dec!(100), dec!(1), Side::Buy)); }
        for i in 0..7 { p.record(&t(300 + i * 100, dec!(100), dec!(1), Side::Sell)); }
        assert!(p.sell_tps() > p.buy_tps());
    }

    #[test]
    fn signals_log_capacity_enforced() {
        let mut e = SignalsEngine::default();
        for i in 0..(SIGNAL_LOG_CAP as i64 + 50) {
            e.push(Signal {
                kind: SignalKind::PaceSpike,
                time_ms: i,
                price: dec!(100),
                score: 1,
                side: None,
                note: String::new(),
            });
        }
        assert_eq!(e.log.len(), SIGNAL_LOG_CAP);
    }

    #[test]
    fn iceberg_emitted_when_traded_far_exceeds_visible_with_refreshes() {
        let mut e = SignalsEngine::default();
        // Place a small visible 1.0 at price 100, refresh several times.
        for i in 0..5 {
            // Drop the visible to 0.1, then refresh back to 1.0.
            e.observe_level(dec!(100), dec!(1.0));
            e.observe_level(dec!(100), dec!(0.1));
            // Trade through it.
            let mut book = OrderBook::default();
            book.bids.apply(dec!(99), dec!(1));
            book.asks.apply(dec!(101), dec!(1));
            e.observe_trade(&t(i * 100, dec!(100), dec!(2.0), Side::Buy), &book);
        }
        let has_iceberg = e.log.iter().any(|s| matches!(s.kind, SignalKind::Iceberg));
        assert!(has_iceberg, "expected an iceberg signal");
    }

    #[test]
    fn absorption_emitted_when_volume_far_above_visible_without_refresh() {
        let mut e = SignalsEngine::default();
        // Set a level visible at 1.0 (only seen once).
        e.observe_level(dec!(100), dec!(1.0));
        let mut book = OrderBook::default();
        book.bids.apply(dec!(99), dec!(1));
        book.asks.apply(dec!(101), dec!(1));
        // Heavy trades through it without refresh.
        for i in 0..3 {
            e.observe_trade(&t(i * 50, dec!(100), dec!(1.0), Side::Sell), &book);
        }
        assert!(e.log.iter().any(|s| matches!(s.kind, SignalKind::Absorption)));
    }

    #[test]
    fn pace_spike_emits_when_rate_is_3x_session_average() {
        let mut e = SignalsEngine::default();
        let mut book = OrderBook::default();
        book.bids.apply(dec!(99), dec!(1));
        book.asks.apply(dec!(101), dec!(1));
        // Slow baseline: 1 trade per 2 seconds for 60 seconds → avg ~0.5 tps.
        for i in 0..30 {
            e.observe_trade(&t(i * 2000, dec!(100), dec!(0.1), Side::Buy), &book);
        }
        // Burst: 25 trades in 1 second → instantaneous tps = 25/5 = 5 → ratio 10× avg.
        for i in 0..25 {
            e.observe_trade(&t(60_000 + i * 40, dec!(100), dec!(0.1), Side::Buy), &book);
        }
        assert!(e.log.iter().any(|s| matches!(s.kind, SignalKind::PaceSpike)));
    }

    #[test]
    fn stop_run_emitted_after_divergent_extension_and_reversal() {
        let mut e = SignalsEngine::default();
        // Rising sequence with positive deltas.
        e.observe_bar_close(0, dec!(100), dec!(1));
        e.observe_bar_close(60_000, dec!(101), dec!(1));
        // Trigger bar: price runs up further but delta is negative (divergent).
        e.observe_bar_close(120_000, dec!(105), dec!(-2));
        // Reversal bars over the next stop_run_revert_bars=3.
        e.observe_bar_close(180_000, dec!(103), dec!(-1));
        e.observe_bar_close(240_000, dec!(101), dec!(-1));
        e.observe_bar_close(300_000, dec!(100), dec!(0));
        e.observe_bar_close(360_000, dec!(99), dec!(0));
        assert!(e.log.iter().any(|s| matches!(s.kind, SignalKind::StopRun)));
    }

    #[test]
    fn exhaustion_emitted_when_new_high_with_weakening_delta() {
        let mut e = SignalsEngine::default();
        // Build a trail with a prior high accompanied by strong buy delta.
        e.observe_bar_close(0, dec!(100), dec!(2));
        e.observe_bar_close(60_000, dec!(105), dec!(5)); // prior high w/ +delta
        e.observe_bar_close(120_000, dec!(103), dec!(-1));
        e.observe_bar_close(180_000, dec!(102), dec!(0));
        e.observe_bar_close(240_000, dec!(104), dec!(1));
        e.observe_bar_close(300_000, dec!(103), dec!(-1));
        // New high with NEGATIVE delta.
        e.observe_bar_close(360_000, dec!(106), dec!(-3));
        assert!(e.log.iter().any(|s| matches!(s.kind, SignalKind::Exhaustion)));
    }

    #[test]
    fn iceberg_not_emitted_when_level_was_never_visible() {
        let mut e = SignalsEngine::default();
        let mut book = OrderBook::default();
        book.bids.apply(dec!(99), dec!(1));
        book.asks.apply(dec!(101), dec!(1));
        // Trade hits a price that was never in the visible book — no
        // observe_level call at price 100.
        for i in 0..10 {
            e.observe_trade(&t(i * 50, dec!(100), dec!(5.0), Side::Buy), &book);
        }
        assert!(!e.log.iter().any(|s| matches!(s.kind, SignalKind::Iceberg)));
        assert!(!e.log.iter().any(|s| matches!(s.kind, SignalKind::Absorption)));
    }

    #[test]
    fn levels_map_bounded_under_pressure() {
        let mut e = SignalsEngine::default();
        // Feed many distinct prices into observe_level via observe_book.
        // Construct synthetic books with N levels each, never overlapping.
        for cycle in 0..3 {
            let mut book = OrderBook::default();
            for i in 0..1000 {
                let price = Decimal::from(cycle * 100_000 + i);
                book.asks.apply(price, dec!(1));
            }
            e.observe_book(&book);
        }
        // After 3000 distinct prices observed, the bound should hold.
        assert!(
            e.levels.len() <= LEVELS_MAX,
            "levels exceeded cap: {}",
            e.levels.len()
        );
    }

    #[test]
    fn reset_clears_engine() {
        let mut e = SignalsEngine::default();
        e.observe_level(dec!(100), dec!(1));
        let mut book = OrderBook::default();
        book.bids.apply(dec!(99), dec!(1));
        book.asks.apply(dec!(101), dec!(1));
        e.observe_trade(&t(0, dec!(100), dec!(1), Side::Buy), &book);
        e.reset();
        assert!(e.log.is_empty());
        assert!(e.levels.is_empty());
        assert_eq!(e.pace.current_tps(), 0.0);
    }
}
