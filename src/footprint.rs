use std::collections::{BTreeMap, VecDeque};

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::delta::Side;
use crate::feed::Trade;

/// One-minute bars by default.
pub const DEFAULT_BAR_MS: i64 = 60_000;
/// Rolling history depth for completed bars.
pub const ROLLING_BARS: usize = 20;
/// Imbalance threshold: one side ≥ N× the other.
pub const IMBALANCE_RATIO: i64 = 3;
/// Tick size for BTCUSDT spot.
pub const DEFAULT_TICK: Decimal = dec!(0.01);

/// One price-level cell within a bar: buy and sell volumes at this tick.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cell {
    pub buy: Decimal,
    pub sell: Decimal,
}

impl Cell {
    pub fn total(&self) -> Decimal {
        self.buy + self.sell
    }

    /// Imbalance class for this cell.
    pub fn imbalance(&self) -> Imbalance {
        let r = Decimal::from(IMBALANCE_RATIO);
        // Spec: "three times or more the sell volume" → use >= (inclusive).
        let buy_dominant = self.buy >= self.sell * r && self.buy > Decimal::ZERO;
        let sell_dominant = self.sell >= self.buy * r && self.sell > Decimal::ZERO;
        if buy_dominant {
            Imbalance::Buy
        } else if sell_dominant {
            Imbalance::Sell
        } else {
            Imbalance::Balanced
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Imbalance {
    Buy,
    Sell,
    Balanced,
}

/// One time bar.
#[derive(Debug, Clone)]
pub struct FootprintBar {
    pub start_ms: i64,
    pub end_ms: i64,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub total_buy: Decimal,
    pub total_sell: Decimal,
    /// Cells keyed by tick-rounded price. BTreeMap so iteration is by ascending price.
    pub levels: BTreeMap<Decimal, Cell>,
}

impl FootprintBar {
    pub fn new(start_ms: i64, end_ms: i64, first_price: Decimal) -> Self {
        Self {
            start_ms,
            end_ms,
            open: first_price,
            high: first_price,
            low: first_price,
            close: first_price,
            total_buy: Decimal::ZERO,
            total_sell: Decimal::ZERO,
            levels: BTreeMap::new(),
        }
    }

    pub fn delta(&self) -> Decimal {
        self.total_buy - self.total_sell
    }

    pub fn total_volume(&self) -> Decimal {
        self.total_buy + self.total_sell
    }

    /// The price level with the highest combined volume in this bar.
    /// On ties, returns the highest price (deterministic).
    pub fn point_of_control(&self) -> Option<Decimal> {
        self.levels
            .iter()
            .max_by(|a, b| {
                let ord = a.1.total().cmp(&b.1.total());
                if ord.is_eq() { a.0.cmp(b.0) } else { ord }
            })
            .map(|(p, _)| *p)
    }

    pub fn record_trade(&mut self, price: Decimal, qty: Decimal, side: Side, tick: Decimal) {
        if qty.is_sign_negative() || qty.is_zero() {
            return;
        }
        let level = round_to_tick(price, tick);
        let cell = self.levels.entry(level).or_default();
        match side {
            Side::Buy => {
                cell.buy += qty;
                self.total_buy += qty;
            }
            Side::Sell => {
                cell.sell += qty;
                self.total_sell += qty;
            }
        }
        self.close = price;
        if price > self.high {
            self.high = price;
        }
        if price < self.low {
            self.low = price;
        }
    }
}

fn round_to_tick(price: Decimal, tick: Decimal) -> Decimal {
    if tick.is_zero() || tick.is_sign_negative() {
        return price;
    }
    let n = price / tick;
    n.round() * tick
}

/// Aligns a timestamp to the start of its bar period.
fn bar_start_for(time_ms: i64, bar_ms: i64) -> i64 {
    // Floor toward minus-infinity so negative timestamps stay coherent.
    if time_ms >= 0 {
        (time_ms / bar_ms) * bar_ms
    } else {
        ((time_ms - (bar_ms - 1)) / bar_ms) * bar_ms
    }
}

#[derive(Debug, Clone)]
pub struct FootprintEngine {
    bar_ms: i64,
    tick: Decimal,
    completed: VecDeque<FootprintBar>,
    forming: Option<FootprintBar>,
}

impl Default for FootprintEngine {
    fn default() -> Self {
        Self::new(DEFAULT_BAR_MS, DEFAULT_TICK)
    }
}

impl FootprintEngine {
    pub fn new(bar_ms: i64, tick: Decimal) -> Self {
        assert!(bar_ms > 0, "bar period must be positive");
        Self {
            bar_ms,
            tick,
            completed: VecDeque::with_capacity(ROLLING_BARS + 1),
            forming: None,
        }
    }

    pub fn bar_ms(&self) -> i64 {
        self.bar_ms
    }

    pub fn tick(&self) -> Decimal {
        self.tick
    }

    pub fn record(&mut self, trade: &Trade) {
        let bar_start = bar_start_for(trade.time_ms, self.bar_ms);
        let bar_end = bar_start + self.bar_ms;

        match self.forming.as_mut() {
            None => {
                let mut b = FootprintBar::new(bar_start, bar_end, trade.price);
                b.record_trade(trade.price, trade.qty, trade.side, self.tick);
                self.forming = Some(b);
            }
            Some(b) if b.start_ms == bar_start => {
                b.record_trade(trade.price, trade.qty, trade.side, self.tick);
            }
            Some(b) if bar_start < b.start_ms => {
                // Out-of-order older trade: drop to avoid contaminating bars.
                return;
            }
            Some(_) => {
                // Newer bar: seal the forming bar and start fresh.
                let old = self.forming.take().expect("forming present");
                self.completed.push_back(old);
                while self.completed.len() > ROLLING_BARS {
                    self.completed.pop_front();
                }
                let mut b = FootprintBar::new(bar_start, bar_end, trade.price);
                b.record_trade(trade.price, trade.qty, trade.side, self.tick);
                self.forming = Some(b);
            }
        }
    }

    pub fn completed(&self) -> &VecDeque<FootprintBar> {
        &self.completed
    }

    pub fn forming(&self) -> Option<&FootprintBar> {
        self.forming.as_ref()
    }

    /// Iterator over visible bars: completed (oldest first) then forming.
    pub fn iter_visible(&self) -> impl Iterator<Item = &FootprintBar> {
        self.completed.iter().chain(self.forming.as_ref())
    }

    pub fn reset(&mut self) {
        self.completed.clear();
        self.forming = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trade(time_ms: i64, price: Decimal, qty: Decimal, side: Side) -> Trade {
        Trade { time_ms, price, qty, side }
    }

    #[test]
    fn bar_start_minute_aligned() {
        // 60_001 ms → bar starts at 60_000 (1 minute boundary).
        assert_eq!(bar_start_for(60_001, 60_000), 60_000);
        assert_eq!(bar_start_for(119_999, 60_000), 60_000);
        assert_eq!(bar_start_for(120_000, 60_000), 120_000);
        assert_eq!(bar_start_for(0, 60_000), 0);
    }

    #[test]
    fn round_to_tick_basic() {
        assert_eq!(round_to_tick(dec!(100.234), dec!(0.01)), dec!(100.23));
        assert_eq!(round_to_tick(dec!(100.236), dec!(0.01)), dec!(100.24));
    }

    #[test]
    fn trades_in_same_bar_aggregate() {
        let mut eng = FootprintEngine::new(60_000, dec!(0.01));
        eng.record(&trade(0, dec!(100), dec!(1), Side::Buy));
        eng.record(&trade(30_000, dec!(100.01), dec!(2), Side::Sell));
        eng.record(&trade(59_999, dec!(100), dec!(0.5), Side::Buy));

        let f = eng.forming().unwrap();
        assert_eq!(f.start_ms, 0);
        assert_eq!(f.end_ms, 60_000);
        assert_eq!(f.total_buy, dec!(1.5));
        assert_eq!(f.total_sell, dec!(2));
        assert_eq!(f.delta(), dec!(-0.5));
        assert_eq!(f.levels.len(), 2);
        assert!(eng.completed().is_empty());
    }

    #[test]
    fn next_minute_seals_previous_bar() {
        let mut eng = FootprintEngine::new(60_000, dec!(0.01));
        eng.record(&trade(0, dec!(100), dec!(1), Side::Buy));
        eng.record(&trade(60_000, dec!(101), dec!(2), Side::Sell));

        assert_eq!(eng.completed().len(), 1);
        let prev = eng.completed().front().unwrap();
        assert_eq!(prev.start_ms, 0);
        assert_eq!(prev.total_buy, dec!(1));
        assert_eq!(prev.total_sell, Decimal::ZERO);

        let now = eng.forming().unwrap();
        assert_eq!(now.start_ms, 60_000);
        assert_eq!(now.total_sell, dec!(2));
    }

    #[test]
    fn out_of_order_older_trade_dropped() {
        let mut eng = FootprintEngine::new(60_000, dec!(0.01));
        eng.record(&trade(120_000, dec!(100), dec!(1), Side::Buy));
        // A trade arriving "from the past" must not contaminate the new bar.
        eng.record(&trade(30_000, dec!(99), dec!(5), Side::Sell));

        let f = eng.forming().unwrap();
        assert_eq!(f.start_ms, 120_000);
        assert_eq!(f.total_buy, dec!(1));
        assert_eq!(f.total_sell, Decimal::ZERO);
        assert!(eng.completed().is_empty());
    }

    #[test]
    fn point_of_control_picks_highest_volume_level() {
        let mut eng = FootprintEngine::new(60_000, dec!(0.01));
        eng.record(&trade(0, dec!(100.00), dec!(1), Side::Buy));
        eng.record(&trade(0, dec!(100.05), dec!(5), Side::Sell));
        eng.record(&trade(0, dec!(100.10), dec!(2), Side::Buy));
        let poc = eng.forming().unwrap().point_of_control().unwrap();
        assert_eq!(poc, dec!(100.05));
    }

    #[test]
    fn imbalance_classification() {
        let mut eng = FootprintEngine::new(60_000, dec!(0.01));
        eng.record(&trade(0, dec!(100), dec!(10), Side::Buy));
        eng.record(&trade(0, dec!(100), dec!(2), Side::Sell));
        let cell = eng.forming().unwrap().levels[&dec!(100)];
        // 10 vs 2: 10 > 2*3 = 6 → buy-imbalanced.
        assert_eq!(cell.imbalance(), Imbalance::Buy);

        let mut eng2 = FootprintEngine::new(60_000, dec!(0.01));
        eng2.record(&trade(0, dec!(100), dec!(3), Side::Buy));
        eng2.record(&trade(0, dec!(100), dec!(3), Side::Sell));
        let cell2 = eng2.forming().unwrap().levels[&dec!(100)];
        assert_eq!(cell2.imbalance(), Imbalance::Balanced);
    }

    #[test]
    fn ohlc_tracked_on_raw_prices() {
        let mut eng = FootprintEngine::new(60_000, dec!(0.01));
        eng.record(&trade(0, dec!(100.00), dec!(1), Side::Buy));
        eng.record(&trade(1000, dec!(100.50), dec!(1), Side::Sell));
        eng.record(&trade(2000, dec!(99.80), dec!(1), Side::Buy));
        eng.record(&trade(3000, dec!(100.20), dec!(1), Side::Sell));
        let f = eng.forming().unwrap();
        assert_eq!(f.open, dec!(100.00));
        assert_eq!(f.high, dec!(100.50));
        assert_eq!(f.low, dec!(99.80));
        assert_eq!(f.close, dec!(100.20));
    }

    #[test]
    fn rolling_window_caps_at_twenty() {
        let mut eng = FootprintEngine::new(60_000, dec!(0.01));
        for i in 0..(ROLLING_BARS as i64 + 5) {
            eng.record(&trade(i * 60_000, dec!(100), dec!(1), Side::Buy));
        }
        // Final trade triggered a new forming bar AFTER pushing prior into completed.
        // 25 trades in distinct minutes → 24 sealed bars (rolling caps to 20) + 1 forming.
        assert_eq!(eng.completed().len(), ROLLING_BARS);
        assert!(eng.forming().is_some());
    }

    #[test]
    fn cell_volumes_sum_to_bar_totals() {
        let mut eng = FootprintEngine::new(60_000, dec!(0.01));
        eng.record(&trade(0, dec!(100.00), dec!(1.0), Side::Buy));
        eng.record(&trade(0, dec!(100.05), dec!(0.5), Side::Sell));
        eng.record(&trade(0, dec!(100.10), dec!(2.0), Side::Buy));
        eng.record(&trade(0, dec!(100.05), dec!(1.5), Side::Sell));

        let f = eng.forming().unwrap();
        let cell_buy: Decimal = f.levels.values().map(|c| c.buy).sum();
        let cell_sell: Decimal = f.levels.values().map(|c| c.sell).sum();
        assert_eq!(cell_buy, f.total_buy);
        assert_eq!(cell_sell, f.total_sell);
    }

    #[test]
    fn empty_minutes_do_not_create_phantom_bars() {
        let mut eng = FootprintEngine::new(60_000, dec!(0.01));
        eng.record(&trade(0, dec!(100), dec!(1), Side::Buy));
        // Skip ahead 5 minutes; no trades happened in between.
        eng.record(&trade(300_000, dec!(101), dec!(1), Side::Sell));
        assert_eq!(eng.completed().len(), 1);
        assert_eq!(eng.completed().front().unwrap().start_ms, 0);
        assert_eq!(eng.forming().unwrap().start_ms, 300_000);
    }

    #[test]
    fn reset_clears_engine() {
        let mut eng = FootprintEngine::new(60_000, dec!(0.01));
        eng.record(&trade(0, dec!(100), dec!(1), Side::Buy));
        eng.record(&trade(60_000, dec!(101), dec!(1), Side::Buy));
        eng.reset();
        assert!(eng.completed().is_empty());
        assert!(eng.forming().is_none());
    }
}
