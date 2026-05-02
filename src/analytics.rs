//! Session analytics. Listens to paper-engine fills and reconstructs
//! round-trip trades, then computes performance, timing, behavioural,
//! and market-context metrics on demand.
//!
//! Round-trip definition: a sequence of fills that takes the position
//! from zero back to zero (or that *closes* a portion). When the
//! position closes (partially or fully), a TradeRecord is emitted for
//! the closed quantity at the matching average entry price.

use std::collections::VecDeque;

use rust_decimal::Decimal;

use crate::delta::Side;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Long,
    Short,
}

#[derive(Debug, Clone)]
pub struct EntryContext {
    pub above_vwap: Option<bool>,
    pub aligned_with_delta: Option<bool>,
    pub signals_at_entry: u32,
}

#[derive(Debug, Clone)]
pub struct TradeRecord {
    pub direction: Direction,
    pub entry_time_ms: i64,
    pub exit_time_ms: i64,
    pub qty: Decimal,
    pub entry_price: Decimal,
    pub exit_price: Decimal,
    pub pnl: Decimal,
    pub hold_ms: i64,
    pub entry_ctx: EntryContext,
    /// Original intended stop in price units (for risk-to-reward).
    pub planned_stop: Option<Decimal>,
    /// Original intended target.
    pub planned_target: Option<Decimal>,
    /// Number of times the stop was widened after entry (behavioural metric).
    pub stop_widened: u32,
    /// Number of times the target was tightened (early exit indicator).
    pub target_tightened: u32,
}

#[derive(Debug, Clone)]
struct OpenLot {
    /// Used by callers to know which side the lot represents — also kept
    /// for parity with TradeRecord even though emit_close_from is told
    /// the direction explicitly.
    #[allow(dead_code)]
    direction: Direction,
    entry_time_ms: i64,
    qty: Decimal,
    entry_price: Decimal,
    entry_ctx: EntryContext,
    planned_stop: Option<Decimal>,
    planned_target: Option<Decimal>,
}

impl OpenLot {
    fn new(direction: Direction) -> Self {
        Self {
            direction,
            entry_time_ms: 0,
            qty: Decimal::ZERO,
            entry_price: Decimal::ZERO,
            entry_ctx: EntryContext { above_vwap: None, aligned_with_delta: None, signals_at_entry: 0 },
            planned_stop: None,
            planned_target: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AnalyticsEngine {
    pub trades: VecDeque<TradeRecord>,
    open_long: Option<OpenLot>,
    open_short: Option<OpenLot>,
}

impl Default for AnalyticsEngine {
    fn default() -> Self {
        Self {
            trades: VecDeque::new(),
            open_long: None,
            open_short: None,
        }
    }
}

impl AnalyticsEngine {
    pub fn reset(&mut self) {
        self.trades.clear();
        self.open_long = None;
        self.open_short = None;
    }

    /// Record a fill. Caller passes the side, qty, and price along with
    /// optional entry-context info (used only when the fill opens a new
    /// directional lot — i.e. the position grows away from zero).
    pub fn record_fill(
        &mut self,
        side: Side,
        qty: Decimal,
        price: Decimal,
        time_ms: i64,
        ctx: EntryContext,
        planned_stop: Option<Decimal>,
        planned_target: Option<Decimal>,
    ) {
        if qty.is_zero() {
            return;
        }
        match side {
            Side::Buy => self.apply_buy(qty, price, time_ms, ctx, planned_stop, planned_target),
            Side::Sell => self.apply_sell(qty, price, time_ms, ctx, planned_stop, planned_target),
        }
    }

    fn apply_buy(
        &mut self,
        mut qty: Decimal,
        price: Decimal,
        time_ms: i64,
        ctx: EntryContext,
        planned_stop: Option<Decimal>,
        planned_target: Option<Decimal>,
    ) {
        // Close shorts first — split into snapshot, mutate qty, emit close,
        // then commit lot updates. This keeps the borrow checker happy.
        let close_qty = self.open_short.as_ref().map(|l| l.qty.min(qty)).unwrap_or(Decimal::ZERO);
        if !close_qty.is_zero() {
            let lot_snapshot = self.open_short.as_ref().cloned().unwrap();
            self.emit_close_from(Direction::Short, &lot_snapshot, close_qty, price, time_ms);
            if let Some(s) = self.open_short.as_mut() {
                s.qty -= close_qty;
                if s.qty.is_zero() {
                    self.open_short = None;
                }
            }
            qty -= close_qty;
        }
        if !qty.is_zero() {
            // Open or extend long.
            let lot = self.open_long.get_or_insert_with(|| {
                let mut l = OpenLot::new(Direction::Long);
                l.entry_time_ms = time_ms;
                l.entry_ctx = ctx.clone();
                l.planned_stop = planned_stop;
                l.planned_target = planned_target;
                l
            });
            let new_qty = lot.qty + qty;
            lot.entry_price = if new_qty.is_zero() { price } else { (lot.entry_price * lot.qty + price * qty) / new_qty };
            lot.qty = new_qty;
        }
    }

    fn apply_sell(
        &mut self,
        mut qty: Decimal,
        price: Decimal,
        time_ms: i64,
        ctx: EntryContext,
        planned_stop: Option<Decimal>,
        planned_target: Option<Decimal>,
    ) {
        let close_qty = self.open_long.as_ref().map(|l| l.qty.min(qty)).unwrap_or(Decimal::ZERO);
        if !close_qty.is_zero() {
            let lot_snapshot = self.open_long.as_ref().cloned().unwrap();
            self.emit_close_from(Direction::Long, &lot_snapshot, close_qty, price, time_ms);
            if let Some(l) = self.open_long.as_mut() {
                l.qty -= close_qty;
                if l.qty.is_zero() {
                    self.open_long = None;
                }
            }
            qty -= close_qty;
        }
        if !qty.is_zero() {
            let lot = self.open_short.get_or_insert_with(|| {
                let mut l = OpenLot::new(Direction::Short);
                l.entry_time_ms = time_ms;
                l.entry_ctx = ctx.clone();
                l.planned_stop = planned_stop;
                l.planned_target = planned_target;
                l
            });
            let new_qty = lot.qty + qty;
            lot.entry_price = if new_qty.is_zero() { price } else { (lot.entry_price * lot.qty + price * qty) / new_qty };
            lot.qty = new_qty;
        }
    }

    fn emit_close_from(&mut self, dir: Direction, lot: &OpenLot, qty: Decimal, exit_price: Decimal, exit_time_ms: i64) {
        let pnl = match dir {
            Direction::Long => (exit_price - lot.entry_price) * qty,
            Direction::Short => (lot.entry_price - exit_price) * qty,
        };
        self.trades.push_back(TradeRecord {
            direction: dir,
            entry_time_ms: lot.entry_time_ms,
            exit_time_ms,
            qty,
            entry_price: lot.entry_price,
            exit_price,
            pnl,
            hold_ms: exit_time_ms - lot.entry_time_ms,
            entry_ctx: lot.entry_ctx.clone(),
            planned_stop: lot.planned_stop,
            planned_target: lot.planned_target,
            stop_widened: 0,
            target_tightened: 0,
        });
    }

    pub fn count(&self) -> usize { self.trades.len() }

    pub fn winners(&self) -> usize {
        self.trades.iter().filter(|t| t.pnl > Decimal::ZERO).count()
    }

    pub fn losers(&self) -> usize {
        self.trades.iter().filter(|t| t.pnl < Decimal::ZERO).count()
    }

    pub fn win_rate(&self) -> Option<f64> {
        let n = self.count();
        if n == 0 { None } else { Some(self.winners() as f64 / n as f64) }
    }

    pub fn total_pnl(&self) -> Decimal {
        self.trades.iter().map(|t| t.pnl).sum()
    }

    pub fn gross_profit(&self) -> Decimal {
        self.trades.iter().filter(|t| t.pnl > Decimal::ZERO).map(|t| t.pnl).sum()
    }

    pub fn gross_loss(&self) -> Decimal {
        // Returned as a positive number (sum of |loss|).
        let s: Decimal = self.trades.iter().filter(|t| t.pnl < Decimal::ZERO).map(|t| t.pnl).sum();
        -s
    }

    pub fn profit_factor(&self) -> Option<f64> {
        let gl = self.gross_loss();
        if gl.is_zero() { return None; }
        let gp: f64 = self.gross_profit().try_into().unwrap_or(0.0);
        let glf: f64 = gl.try_into().unwrap_or(0.0);
        if glf == 0.0 { None } else { Some(gp / glf) }
    }

    pub fn avg_winner(&self) -> Option<Decimal> {
        let w: Vec<Decimal> = self.trades.iter().filter(|t| t.pnl > Decimal::ZERO).map(|t| t.pnl).collect();
        if w.is_empty() { None } else { Some(w.iter().copied().sum::<Decimal>() / Decimal::from(w.len() as u64)) }
    }

    pub fn avg_loser(&self) -> Option<Decimal> {
        let l: Vec<Decimal> = self.trades.iter().filter(|t| t.pnl < Decimal::ZERO).map(|t| t.pnl).collect();
        if l.is_empty() { None } else { Some(l.iter().copied().sum::<Decimal>() / Decimal::from(l.len() as u64)) }
    }

    pub fn expectancy(&self) -> Option<Decimal> {
        let n = self.count();
        if n == 0 { return None; }
        Some(self.total_pnl() / Decimal::from(n as u64))
    }

    pub fn max_drawdown(&self) -> Decimal {
        let mut peak = Decimal::ZERO;
        let mut cum = Decimal::ZERO;
        let mut dd = Decimal::ZERO;
        for t in &self.trades {
            cum += t.pnl;
            if cum > peak { peak = cum; }
            let cur_dd = peak - cum;
            if cur_dd > dd { dd = cur_dd; }
        }
        dd
    }

    pub fn longest_streak(&self, winning: bool) -> usize {
        let mut best = 0;
        let mut cur = 0;
        for t in &self.trades {
            let hit = if winning { t.pnl > Decimal::ZERO } else { t.pnl < Decimal::ZERO };
            if hit {
                cur += 1;
                if cur > best { best = cur; }
            } else {
                cur = 0;
            }
        }
        best
    }

    pub fn cumulative_pnl(&self) -> Vec<Decimal> {
        let mut out = Vec::with_capacity(self.trades.len());
        let mut acc = Decimal::ZERO;
        for t in &self.trades {
            acc += t.pnl;
            out.push(acc);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn ctx() -> EntryContext {
        EntryContext { above_vwap: None, aligned_with_delta: None, signals_at_entry: 0 }
    }

    #[test]
    fn long_round_trip_records_one_trade_with_correct_pnl() {
        let mut a = AnalyticsEngine::default();
        a.record_fill(Side::Buy, dec!(1), dec!(100), 0, ctx(), None, None);
        a.record_fill(Side::Sell, dec!(1), dec!(110), 60_000, ctx(), None, None);
        assert_eq!(a.count(), 1);
        let t = a.trades.front().unwrap();
        assert_eq!(t.direction, Direction::Long);
        assert_eq!(t.pnl, dec!(10));
        assert_eq!(t.hold_ms, 60_000);
    }

    #[test]
    fn short_round_trip() {
        let mut a = AnalyticsEngine::default();
        a.record_fill(Side::Sell, dec!(2), dec!(100), 0, ctx(), None, None);
        a.record_fill(Side::Buy, dec!(2), dec!(90), 30_000, ctx(), None, None);
        assert_eq!(a.count(), 1);
        let t = a.trades.front().unwrap();
        assert_eq!(t.direction, Direction::Short);
        assert_eq!(t.pnl, dec!(20));
    }

    #[test]
    fn partial_close_then_full_close_records_two_trades() {
        let mut a = AnalyticsEngine::default();
        a.record_fill(Side::Buy, dec!(2), dec!(100), 0, ctx(), None, None);
        a.record_fill(Side::Sell, dec!(1), dec!(105), 30_000, ctx(), None, None);
        a.record_fill(Side::Sell, dec!(1), dec!(108), 60_000, ctx(), None, None);
        assert_eq!(a.count(), 2);
        let total = a.total_pnl();
        assert_eq!(total, dec!(13));
    }

    #[test]
    fn flip_through_zero_records_close_then_opens_short() {
        let mut a = AnalyticsEngine::default();
        a.record_fill(Side::Buy, dec!(1), dec!(100), 0, ctx(), None, None);
        a.record_fill(Side::Sell, dec!(3), dec!(105), 30_000, ctx(), None, None);
        // Closes long, opens short of size 2.
        assert_eq!(a.count(), 1);
        // Now cover the short.
        a.record_fill(Side::Buy, dec!(2), dec!(100), 60_000, ctx(), None, None);
        assert_eq!(a.count(), 2);
        // Long: +5; Short: +10. Total: +15.
        assert_eq!(a.total_pnl(), dec!(15));
    }

    #[test]
    fn metrics_basics() {
        let mut a = AnalyticsEngine::default();
        // Two winners, one loser.
        a.record_fill(Side::Buy, dec!(1), dec!(100), 0, ctx(), None, None);
        a.record_fill(Side::Sell, dec!(1), dec!(110), 1, ctx(), None, None); // +10
        a.record_fill(Side::Buy, dec!(1), dec!(100), 2, ctx(), None, None);
        a.record_fill(Side::Sell, dec!(1), dec!(95), 3, ctx(), None, None);  // -5
        a.record_fill(Side::Buy, dec!(1), dec!(100), 4, ctx(), None, None);
        a.record_fill(Side::Sell, dec!(1), dec!(105), 5, ctx(), None, None); // +5

        assert_eq!(a.count(), 3);
        assert_eq!(a.winners(), 2);
        assert_eq!(a.losers(), 1);
        let wr = a.win_rate().unwrap();
        assert!((wr - 2.0/3.0).abs() < 1e-9);
        assert_eq!(a.total_pnl(), dec!(10));
        assert_eq!(a.gross_profit(), dec!(15));
        assert_eq!(a.gross_loss(), dec!(5));
        let pf = a.profit_factor().unwrap();
        assert!((pf - 3.0).abs() < 1e-9);
        assert_eq!(a.avg_winner().unwrap(), dec!(7.5));
        assert_eq!(a.avg_loser().unwrap(), dec!(-5));
    }

    #[test]
    fn max_drawdown_tracks_peak_to_trough() {
        let mut a = AnalyticsEngine::default();
        for (entry, exit) in [(100, 110), (100, 105), (100, 80), (100, 90)] {
            a.record_fill(Side::Buy, dec!(1), Decimal::from(entry), 0, ctx(), None, None);
            a.record_fill(Side::Sell, dec!(1), Decimal::from(exit), 1, ctx(), None, None);
        }
        // Cum PnL: 10, 15, -5, -15. Peak 15, trough -15. DD=30.
        assert_eq!(a.max_drawdown(), dec!(30));
    }

    #[test]
    fn longest_streak_winning_and_losing() {
        let mut a = AnalyticsEngine::default();
        let outcomes = [110, 110, 110, 90, 90, 110, 90, 90, 90];
        for ex in outcomes {
            a.record_fill(Side::Buy, dec!(1), dec!(100), 0, ctx(), None, None);
            a.record_fill(Side::Sell, dec!(1), Decimal::from(ex), 1, ctx(), None, None);
        }
        assert_eq!(a.longest_streak(true), 3);
        assert_eq!(a.longest_streak(false), 3);
    }

    #[test]
    fn reset_clears_state() {
        let mut a = AnalyticsEngine::default();
        a.record_fill(Side::Buy, dec!(1), dec!(100), 0, ctx(), None, None);
        a.record_fill(Side::Sell, dec!(1), dec!(101), 1, ctx(), None, None);
        a.reset();
        assert_eq!(a.count(), 0);
    }
}
