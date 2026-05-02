//! All engines that operate on a single symbol's market-data stream.
//!
//! A [`SymbolEngines`] is fully self-contained: it owns the order book,
//! delta tracker, footprint engine, candle engine, signals engine, tape
//! buffer, and the connection-status bookkeeping for one symbol. Multiple
//! instances run side-by-side in [`AppState`], one per watched symbol.

use std::collections::VecDeque;

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::candle::CandleEngine;
use crate::delta::{self, DeltaTracker};
use crate::feed::{FeedEvent, Trade};
use crate::footprint::FootprintEngine;
use crate::orderbook::{ApplyOutcome, OrderBook};
use crate::signals::SignalsEngine;

/// Hard cap on the visible tape per symbol.
pub const TAPE_CAP: usize = 500;
/// Sliding window for large-trade baseline.
pub const SIZE_WINDOW: usize = 300;
/// A trade is "large" when its qty exceeds this multiple of the rolling
/// mean qty.
pub const LARGE_MULT: Decimal = dec!(5);

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

pub struct SymbolEngines {
    pub symbol: String,
    pub book: OrderBook,
    pub delta: DeltaTracker,
    pub footprint: FootprintEngine,
    pub candles: CandleEngine,
    pub signals: SignalsEngine,
    pub tape: VecDeque<TapeEntry>,
    pub conn: ConnState,
    pub event_count: u64,
    pub has_snapshot: bool,
    qty_window: VecDeque<Decimal>,
    qty_window_sum: Decimal,
    /// Last close price observed (for alert-cross detection).
    pub last_seen_price: Option<f64>,
}

impl SymbolEngines {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            book: OrderBook::new(),
            delta: DeltaTracker::new(),
            footprint: FootprintEngine::default(),
            candles: CandleEngine::default(),
            signals: SignalsEngine::default(),
            tape: VecDeque::with_capacity(TAPE_CAP),
            conn: ConnState::Disconnected,
            event_count: 0,
            has_snapshot: false,
            qty_window: VecDeque::with_capacity(SIZE_WINDOW),
            qty_window_sum: Decimal::ZERO,
            last_seen_price: None,
        }
    }

    /// Last known price (forming candle close, then completed close).
    pub fn last_price(&self) -> Option<f64> {
        if let Some(b) = self.candles.forming() {
            return Some(b.close);
        }
        self.candles.completed().back().map(|b| b.close)
    }

    /// Apply a feed event to this symbol's engines.
    pub fn handle(&mut self, ev: FeedEvent) {
        self.event_count = self.event_count.saturating_add(1);
        match ev {
            FeedEvent::Connected => {
                self.conn = ConnState::Connected;
            }
            FeedEvent::Disconnected => {
                self.conn = ConnState::Disconnected;
                self.book.mark_stale();
            }
            FeedEvent::Snapshot(snap) => {
                if let Err(e) = self.book.apply_snapshot(&snap) {
                    tracing::warn!(error = ?e, "snapshot decode failed");
                    self.book.mark_stale();
                } else {
                    self.has_snapshot = true;
                    self.signals.observe_book(&self.book);
                }
            }
            FeedEvent::Diff(d) => match self.book.apply_diff(&d) {
                ApplyOutcome::Applied => {
                    self.signals.observe_book(&self.book);
                }
                ApplyOutcome::Dropped => {}
                ApplyOutcome::OutOfSync => {
                    tracing::warn!(
                        last = ?self.book.last_update_id,
                        first = d.first_update_id,
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

        // Detect bar closes on this trade. We use the engine's monotonic
        // sealed_count so the detection still works after the rolling
        // window cap evicts old bars (where `completed.len()` is constant).
        let prev_sealed = self.candles.sealed_count();
        self.candles.record(&trade);
        let sealed_now = self.candles.sealed_count();
        if sealed_now > prev_sealed {
            // The just-sealed bar is the *last* element of completed.
            // Iterate from the back, taking the (sealed_now - prev_sealed)
            // most recent bars.
            let new_n = (sealed_now - prev_sealed) as usize;
            let total = self.candles.completed().len();
            let skip = total.saturating_sub(new_n);
            for bar in self.candles.completed().iter().skip(skip) {
                let close = Decimal::from_f64_retain(bar.close).unwrap_or_default();
                let d = Decimal::from_f64_retain(bar.delta()).unwrap_or_default();
                self.signals.observe_bar_close(bar.start_ms, close, d);
            }
        }

        self.signals.observe_trade(&trade, &self.book);

        let price_f: f64 = trade.price.try_into().unwrap_or(0.0);
        self.last_seen_price = Some(price_f);

        // Tape + large-trade detection.
        // CRITICAL ordering: classify large BEFORE adding this trade's qty
        // to the rolling baseline. Otherwise the new trade biases its own
        // threshold (a true 5× outlier shifts the mean and would need ~6×
        // to register, suppressing detection).
        let large = self.is_large(trade.qty);
        self.update_size_window(trade.qty);
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

    pub fn aggressor_label(side: delta::Side) -> &'static str {
        match side {
            delta::Side::Buy => "BUY",
            delta::Side::Sell => "SELL",
        }
    }

    pub fn reset_session(&mut self) {
        self.delta.reset();
        self.footprint.reset();
        self.candles.reset_all();
        self.signals.reset();
    }
}
