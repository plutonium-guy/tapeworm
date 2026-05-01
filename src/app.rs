use std::collections::VecDeque;

use rust_decimal::Decimal;

use crate::delta::{self, DeltaTracker};
use crate::feed::{FeedEvent, Trade};
use crate::footprint::FootprintEngine;
use crate::orderbook::{ApplyOutcome, OrderBook};

/// Hard cap on the visible tape. Older trades are dropped.
pub const TAPE_CAP: usize = 500;
/// Sliding window for large-trade baseline.
pub const SIZE_WINDOW: usize = 300;
/// A trade is "large" when its qty exceeds this multiple of the rolling
/// mean qty AND clears the absolute minimum.
pub const LARGE_MULT: Decimal = rust_decimal_macros::dec!(5);

#[derive(Debug, Clone)]
pub struct TapeEntry {
    pub trade: Trade,
    pub large: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Disconnected,
    Connected,
}

pub struct AppState {
    pub symbol: String,
    pub book: OrderBook,
    pub delta: DeltaTracker,
    pub footprint: FootprintEngine,
    pub tape: VecDeque<TapeEntry>,
    pub conn: ConnState,
    pub event_count: u64,
    /// Rolling sum of recent trade qtys (for large detection).
    qty_window: VecDeque<Decimal>,
    qty_window_sum: Decimal,
    /// Track if any snapshot has ever been received (to suppress "stale"
    /// before first sync).
    pub has_snapshot: bool,
    /// Whether the footprint panel is shown.
    pub show_footprint: bool,
}

impl AppState {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            book: OrderBook::new(),
            delta: DeltaTracker::new(),
            footprint: FootprintEngine::default(),
            tape: VecDeque::with_capacity(TAPE_CAP),
            conn: ConnState::Disconnected,
            event_count: 0,
            qty_window: VecDeque::with_capacity(SIZE_WINDOW),
            qty_window_sum: Decimal::ZERO,
            has_snapshot: false,
            show_footprint: true,
        }
    }

    pub fn handle(&mut self, ev: FeedEvent) {
        self.event_count = self.event_count.saturating_add(1);
        match ev {
            FeedEvent::Connected => {
                self.conn = ConnState::Connected;
            }
            FeedEvent::Disconnected => {
                self.conn = ConnState::Disconnected;
                // After a reconnect we will re-sync from a fresh snapshot.
                self.book.mark_stale();
            }
            FeedEvent::Snapshot(snap) => {
                if let Err(e) = self.book.apply_snapshot(&snap) {
                    tracing::warn!(error = ?e, "snapshot decode failed");
                    self.book.mark_stale();
                } else {
                    self.has_snapshot = true;
                }
            }
            FeedEvent::Diff(ev) => match self.book.apply_diff(&ev) {
                ApplyOutcome::Applied | ApplyOutcome::Dropped => {}
                ApplyOutcome::OutOfSync => {
                    tracing::warn!(
                        last = ?self.book.last_update_id,
                        first = ev.first_update_id,
                        "diff out of sync; awaiting resync"
                    );
                }
                ApplyOutcome::NoSnapshot => {}
            },
            FeedEvent::Trade(trade) => self.record_trade(trade),
        }
    }

    fn record_trade(&mut self, trade: Trade) {
        self.delta.record(trade.side, trade.qty);
        self.footprint.record(&trade);
        self.update_size_window(trade.qty);
        let large = self.is_large(trade.qty);
        self.tape.push_front(TapeEntry { trade, large });
        while self.tape.len() > TAPE_CAP {
            self.tape.pop_back();
        }
    }

    fn update_size_window(&mut self, qty: Decimal) {
        self.qty_window.push_back(qty);
        self.qty_window_sum += qty;
        while self.qty_window.len() > SIZE_WINDOW {
            if let Some(old) = self.qty_window.pop_front() {
                self.qty_window_sum -= old;
            }
        }
    }

    fn is_large(&self, qty: Decimal) -> bool {
        let n = self.qty_window.len();
        if n < 20 {
            return false;
        }
        let mean = self.qty_window_sum / Decimal::from(n as u64);
        if mean.is_zero() {
            return false;
        }
        qty >= mean * LARGE_MULT
    }

    /// Reset cumulative session counters. Tape & book left as-is.
    pub fn reset_session(&mut self) {
        self.delta.reset();
        self.footprint.reset();
    }

    pub fn toggle_footprint(&mut self) {
        self.show_footprint = !self.show_footprint;
    }

    pub fn aggressor_label(&self, side: delta::Side) -> &'static str {
        match side {
            delta::Side::Buy => "BUY",
            delta::Side::Sell => "SELL",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::Side;
    use rust_decimal_macros::dec;

    fn t(side: Side, qty: Decimal) -> Trade {
        Trade {
            time_ms: 0,
            price: dec!(100),
            qty,
            side,
        }
    }

    #[test]
    fn tape_caps_at_limit() {
        let mut app = AppState::new("BTCUSDT");
        for _ in 0..(TAPE_CAP + 50) {
            app.handle(FeedEvent::Trade(t(Side::Buy, dec!(1))));
        }
        assert_eq!(app.tape.len(), TAPE_CAP);
    }

    #[test]
    fn newest_trade_is_at_front() {
        let mut app = AppState::new("BTCUSDT");
        app.handle(FeedEvent::Trade(Trade {
            time_ms: 1,
            price: dec!(100),
            qty: dec!(1),
            side: Side::Buy,
        }));
        app.handle(FeedEvent::Trade(Trade {
            time_ms: 2,
            price: dec!(101),
            qty: dec!(1),
            side: Side::Sell,
        }));
        assert_eq!(app.tape.front().unwrap().trade.time_ms, 2);
    }

    #[test]
    fn large_trade_flag_triggers_above_threshold() {
        let mut app = AppState::new("BTCUSDT");
        // Seed window with 30 small trades.
        for _ in 0..30 {
            app.handle(FeedEvent::Trade(t(Side::Buy, dec!(1))));
        }
        // Mean ~1; LARGE_MULT=5 → 5+ counts as large.
        app.handle(FeedEvent::Trade(t(Side::Sell, dec!(10))));
        assert!(app.tape.front().unwrap().large);
        // A normal one right after should not be flagged.
        app.handle(FeedEvent::Trade(t(Side::Sell, dec!(1))));
        assert!(!app.tape.front().unwrap().large);
    }

    #[test]
    fn disconnect_marks_book_stale() {
        let mut app = AppState::new("BTCUSDT");
        app.handle(FeedEvent::Connected);
        app.handle(FeedEvent::Disconnected);
        assert!(app.book.is_stale());
        assert_eq!(app.conn, ConnState::Disconnected);
    }

    #[test]
    fn reset_session_zeros_delta_only() {
        let mut app = AppState::new("BTCUSDT");
        app.handle(FeedEvent::Trade(t(Side::Buy, dec!(2))));
        app.handle(FeedEvent::Trade(t(Side::Sell, dec!(1))));
        assert_eq!(app.delta.delta(), dec!(1));
        app.reset_session();
        assert_eq!(app.delta.delta(), Decimal::ZERO);
        assert!(app.footprint.forming().is_none());
        assert_eq!(app.tape.len(), 2); // tape preserved
    }

    #[test]
    fn footprint_totals_match_delta_totals_within_one_bar() {
        let mut app = AppState::new("BTCUSDT");
        // All trades within the same minute bar.
        let trades: Vec<(Side, Decimal, Decimal)> = vec![
            (Side::Buy, dec!(100.00), dec!(1.5)),
            (Side::Sell, dec!(100.05), dec!(0.7)),
            (Side::Buy, dec!(100.10), dec!(2.0)),
            (Side::Sell, dec!(100.05), dec!(1.3)),
            (Side::Buy, dec!(100.00), dec!(0.5)),
        ];
        for (side, price, qty) in trades {
            app.handle(FeedEvent::Trade(Trade {
                time_ms: 30_000,
                price,
                qty,
                side,
            }));
        }
        let bar = app.footprint.forming().unwrap();
        assert_eq!(bar.total_buy, app.delta.buy_volume());
        assert_eq!(bar.total_sell, app.delta.sell_volume());
        assert_eq!(bar.delta(), app.delta.delta());
    }

    #[test]
    fn toggle_flips_footprint_visibility() {
        let mut app = AppState::new("BTCUSDT");
        assert!(app.show_footprint);
        app.toggle_footprint();
        assert!(!app.show_footprint);
        app.toggle_footprint();
        assert!(app.show_footprint);
    }
}
