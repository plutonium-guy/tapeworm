use std::collections::HashMap;

use rust_decimal::Decimal;
use time::OffsetDateTime;

use crate::analytics::{AnalyticsEngine, EntryContext};
use crate::chart::ChartView;
use crate::delta;
use crate::engines::{ConnState, SymbolEngines};
use crate::feed::FeedEvent;
use crate::multi::SymbolSummary;
use crate::paper::{BracketRole, Order, OrderStatus, OrderType, PaperEngine};

/// Wall-clock now, in Unix milliseconds.
pub fn now_ms() -> i64 {
    let t = OffsetDateTime::now_utc();
    t.unix_timestamp() * 1000 + (t.nanosecond() as i64) / 1_000_000
}

/// What happens when the user clicks a region of the UI.
#[derive(Debug, Clone)]
pub enum ClickAction {
    Quit,
    Reset,
    ToggleFootprint,
    ToggleChart,
    ToggleSignalsLog,
    TogglePaper,
    ToggleAnalytics,
    ToggleWatchlist,
    ToggleGraphs,
    ToggleCrosshair,
    CycleGraphsPage,
    CycleCandleType,
    CycleVolumeMode,
    CycleIndicator,
    CycleSignalsFilter,
    SetTimeframe(u8),
    SwitchActive(String),
    SelectSignalAt(usize),
    JumpToSelectedSignal,
    PaperBuy,
    PaperSell,
    PaperFlatten,
    PaperCancelAll,
    PaperQtyInc,
    PaperQtyDec,
    ToggleEma(u8),
    ToggleVwap,
    ToggleVolumeProfile,
    ToggleCumDelta,
    ToggleIndicatorPanel,
}

/// A click-dispatch hotspot: a rectangle on the terminal grid plus the
/// action to fire when clicked.
#[derive(Debug, Clone)]
pub struct Hotspot {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub action: ClickAction,
}

impl Hotspot {
    pub fn contains(&self, col: u16, row: u16) -> bool {
        col >= self.x && col < self.x + self.w && row >= self.y && row < self.y + self.h
    }
}

pub struct AppState {
    /// Active symbol's engines — drives UI panels.
    pub active: SymbolEngines,
    /// Other watched symbols' engines, running concurrently.
    pub others: Vec<SymbolEngines>,
    /// Lightweight summaries (subset of `others`) for the watchlist row.
    pub watchlist: Vec<SymbolSummary>,

    pub chart: ChartView,
    pub paper: PaperEngine,
    pub analytics: AnalyticsEngine,
    paper_seen_filled: HashMap<u64, Decimal>,
    pub paper_qty: Decimal,

    pub show_paper: bool,
    pub show_analytics: bool,
    pub show_watchlist: bool,
    pub show_footprint: bool,
    pub show_chart: bool,
    pub show_signals_log: bool,
    pub show_graphs: bool,
    /// Selected graphs page (cycles through pages with `]`).
    pub graphs_page: u8,
    /// Rolling spread samples (price units). Capped to ~120.
    pub spread_history: std::collections::VecDeque<f64>,
    /// Rolling order-book imbalance samples in [-1, 1] (top-10 bids vs asks).
    pub imbalance_history: std::collections::VecDeque<f64>,
    /// Rolling trades-per-second samples.
    pub tps_history: std::collections::VecDeque<f64>,
    /// Rolling mid-price samples for the active symbol.
    pub mid_history: std::collections::VecDeque<f64>,
    /// Rolling working-order count samples (taken on every event).
    pub working_orders_history: std::collections::VecDeque<f64>,
    /// Hotspots captured by the most recent draw — main loop dispatches
    /// mouse clicks against this list.
    pub hit_map: Vec<Hotspot>,

    pub alert_flash_until_ms: Option<i64>,
    pub signals_filter: Option<crate::signals::SignalKind>,
    pub signals_sel_idx: usize,
}

impl AppState {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            active: SymbolEngines::new(symbol),
            others: Vec::new(),
            watchlist: Vec::new(),
            chart: ChartView::default(),
            paper: PaperEngine::default(),
            analytics: AnalyticsEngine::default(),
            paper_seen_filled: HashMap::new(),
            paper_qty: rust_decimal_macros::dec!(0.01),
            show_paper: false,
            show_analytics: false,
            show_watchlist: false,
            show_footprint: true,
            show_chart: false,
            show_signals_log: false,
            show_graphs: false,
            graphs_page: 0,
            spread_history: std::collections::VecDeque::with_capacity(128),
            imbalance_history: std::collections::VecDeque::with_capacity(128),
            tps_history: std::collections::VecDeque::with_capacity(128),
            mid_history: std::collections::VecDeque::with_capacity(240),
            working_orders_history: std::collections::VecDeque::with_capacity(120),
            hit_map: Vec::new(),
            alert_flash_until_ms: None,
            signals_filter: None,
            signals_sel_idx: 0,
        }
    }

    /// Apply a feed event for the active symbol.
    pub fn handle(&mut self, ev: FeedEvent) {
        let prev_price = self.active.last_seen_price;
        // Pull out the trade ahead of time so paper trading can see it
        // without us holding two references to the event.
        let trade_for_paper: Option<crate::feed::Trade> = match &ev {
            FeedEvent::Trade(t) => Some(t.clone()),
            _ => None,
        };

        self.active.handle(ev);

        // Use the real trade timestamp where possible, else wall-clock now.
        let event_time_ms = trade_for_paper
            .as_ref()
            .map(|t| t.time_ms)
            .unwrap_or_else(now_ms);
        if let Some(trade) = &trade_for_paper {
            // Alert-cross detection (active symbol only).
            let cur: f64 = trade.price.try_into().unwrap_or(0.0);
            if let Some(prev) = prev_price {
                for &alert in &self.chart.alerts {
                    if (prev < alert && cur >= alert) || (prev > alert && cur <= alert) {
                        self.alert_flash_until_ms = Some(trade.time_ms + 1500);
                        break;
                    }
                }
            }
            self.paper.on_trade(trade);
            self.record_tps_sample();
        }
        self.paper.on_book_update(&self.active.book, event_time_ms);
        self.record_spread_sample();
        self.record_working_orders_sample();
        self.sync_analytics(event_time_ms);
    }

    /// Apply a feed event for a non-active symbol.
    pub fn handle_other(&mut self, idx: usize, ev: FeedEvent) {
        if let Some(eng) = self.others.get_mut(idx) {
            eng.handle(ev);
        }
    }

    /// Add a symbol to the watched set (non-active).
    pub fn add_symbol(&mut self, symbol: impl Into<String>) -> usize {
        let sym: String = symbol.into();
        if self.active.symbol.eq_ignore_ascii_case(&sym)
            || self.others.iter().any(|s| s.symbol.eq_ignore_ascii_case(&sym))
        {
            return usize::MAX;
        }
        self.others.push(SymbolEngines::new(sym));
        self.others.len() - 1
    }

    /// Switch the active symbol to `target`. Swaps the engines so existing
    /// state is preserved on both sides. Paper trading is tied to the
    /// active symbol — any working orders are cancelled on switch so they
    /// don't accidentally fill against the new symbol's book.
    pub fn switch_active(&mut self, target: &str) {
        if self.active.symbol.eq_ignore_ascii_case(target) {
            return;
        }
        if let Some(idx) = self
            .others
            .iter()
            .position(|s| s.symbol.eq_ignore_ascii_case(target))
        {
            // Cancel working paper orders BEFORE swapping. The paper engine
            // is account-level and has no notion of which symbol an order
            // belongs to; we treat outstanding orders as belonging to the
            // currently-active symbol and cancel them to avoid cross-symbol
            // fills.
            self.paper.cancel_all();
            std::mem::swap(&mut self.active, &mut self.others[idx]);
            self.paper_seen_filled.clear();
        }
    }

    /// Reset cumulative session counters across all symbol engines.
    pub fn reset_session(&mut self) {
        self.active.reset_session();
        for o in &mut self.others {
            o.reset_session();
        }
        self.analytics.reset();
        self.paper_seen_filled.clear();
    }

    pub fn toggle_footprint(&mut self) {
        self.show_footprint = !self.show_footprint;
        if self.show_footprint {
            self.show_chart = false;
        }
    }

    pub fn toggle_chart(&mut self) {
        self.show_chart = !self.show_chart;
        if self.show_chart {
            self.show_footprint = false;
        }
    }

    pub fn toggle_signals_log(&mut self) {
        self.show_signals_log = !self.show_signals_log;
    }

    pub fn signals_select(&mut self, delta: i32) {
        let n = self.filtered_signals().count();
        if n == 0 {
            self.signals_sel_idx = 0;
            return;
        }
        let cur = self.signals_sel_idx as i64;
        let max = (n - 1) as i64;
        let nxt = (cur + delta as i64).clamp(0, max);
        self.signals_sel_idx = nxt as usize;
    }

    pub fn filtered_signals(&self) -> impl Iterator<Item = &crate::signals::Signal> + '_ {
        let filter = self.signals_filter;
        self.active.signals.log.iter().filter(move |s| match filter {
            None => true,
            Some(k) => s.kind == k,
        })
    }

    pub fn jump_chart_to_selected_signal(&mut self) {
        let Some(sig) = self.filtered_signals().nth(self.signals_sel_idx) else {
            return;
        };
        let target_time = sig.time_ms;
        let mut total_count = self.active.candles.completed().len();
        if self.active.candles.forming().is_some() {
            total_count += 1;
        }
        if total_count == 0 {
            return;
        }
        let mut scroll = 0usize;
        for (i, bar) in self
            .active
            .candles
            .completed()
            .iter()
            .chain(self.active.candles.forming().into_iter())
            .enumerate()
        {
            if bar.start_ms <= target_time && target_time < bar.end_ms {
                scroll = total_count.saturating_sub(1).saturating_sub(i);
                break;
            }
        }
        self.chart.scroll_back = scroll;
    }

    pub fn paper_market_buy(&mut self, _time_ms: i64) {
        let qty = self.paper_qty;
        let t = now_ms();
        // For a market BUY, slippage is measured against the best ask
        // we'd theoretically have taken at submission.
        let intended = self.active.book.best_ask().map(|x| x.0);
        self.paper.submit(Order {
            id: 0,
            kind: OrderType::Market,
            side: delta::Side::Buy,
            qty,
            price: None,
            stop_price: None,
            status: OrderStatus::Pending,
            filled_qty: Decimal::ZERO,
            avg_fill_price: Decimal::ZERO,
            trail_ticks: None,
            trail_extreme: None,
            bracket: BracketRole::None,
            created_ms: t,
            last_event_ms: t,
            reject_reason: None,
            slippage: Decimal::ZERO,
            intended_price: intended,
        });
    }

    pub fn paper_market_sell(&mut self, _time_ms: i64) {
        let qty = self.paper_qty;
        let t = now_ms();
        // For a market SELL, slippage reference is best_bid at submission.
        let intended = self.active.book.best_bid().map(|x| x.0);
        self.paper.submit(Order {
            id: 0,
            kind: OrderType::Market,
            side: delta::Side::Sell,
            qty,
            price: None,
            stop_price: None,
            status: OrderStatus::Pending,
            filled_qty: Decimal::ZERO,
            avg_fill_price: Decimal::ZERO,
            trail_ticks: None,
            trail_extreme: None,
            bracket: BracketRole::None,
            created_ms: t,
            last_event_ms: t,
            reject_reason: None,
            slippage: Decimal::ZERO,
            intended_price: intended,
        });
    }

    pub fn paper_flatten(&mut self, _time_ms: i64) {
        self.paper.flatten(now_ms());
    }

    pub fn paper_cancel_all(&mut self) {
        self.paper.cancel_all();
    }

    pub fn paper_qty_inc(&mut self) {
        self.paper_qty += rust_decimal_macros::dec!(0.01);
    }

    pub fn paper_qty_dec(&mut self) {
        let next = self.paper_qty - rust_decimal_macros::dec!(0.01);
        if next > Decimal::ZERO {
            self.paper_qty = next;
        }
    }

    pub fn toggle_paper(&mut self) {
        self.show_paper = !self.show_paper;
    }

    pub fn toggle_analytics(&mut self) {
        self.show_analytics = !self.show_analytics;
    }

    pub fn toggle_watchlist(&mut self) {
        self.show_watchlist = !self.show_watchlist;
    }

    pub fn toggle_graphs(&mut self) {
        self.show_graphs = !self.show_graphs;
    }

    pub fn cycle_graphs_page(&mut self) {
        self.graphs_page = (self.graphs_page + 1) % 6;
    }

    /// Resolve a click against the hit map and dispatch each matching action.
    /// Returns `true` if a `Quit` action was triggered.
    pub fn handle_click(&mut self, col: u16, row: u16) -> bool {
        let actions: Vec<ClickAction> = self
            .hit_map
            .iter()
            .filter(|h| h.contains(col, row))
            .map(|h| h.action.clone())
            .collect();
        let mut quit = false;
        for action in actions {
            if matches!(action, ClickAction::Quit) {
                quit = true;
            }
            self.apply_action(action);
        }
        quit
    }

    fn apply_action(&mut self, a: ClickAction) {
        match a {
            ClickAction::Quit => {}
            ClickAction::Reset => self.reset_session(),
            ClickAction::ToggleFootprint => self.toggle_footprint(),
            ClickAction::ToggleChart => self.toggle_chart(),
            ClickAction::ToggleSignalsLog => self.toggle_signals_log(),
            ClickAction::TogglePaper => self.toggle_paper(),
            ClickAction::ToggleAnalytics => self.toggle_analytics(),
            ClickAction::ToggleWatchlist => self.toggle_watchlist(),
            ClickAction::ToggleGraphs => self.toggle_graphs(),
            ClickAction::ToggleCrosshair => self.chart.toggle_crosshair(),
            ClickAction::CycleGraphsPage => self.cycle_graphs_page(),
            ClickAction::CycleCandleType => self.chart.cycle_candle_type(),
            ClickAction::CycleVolumeMode => self.chart.cycle_volume_mode(),
            ClickAction::CycleIndicator => self.chart.cycle_indicator(),
            ClickAction::CycleSignalsFilter => self.cycle_signals_filter(),
            ClickAction::SetTimeframe(idx) => self.chart.set_timeframe_index(idx as usize),
            ClickAction::SwitchActive(sym) => self.switch_active(&sym),
            ClickAction::SelectSignalAt(idx) => {
                self.signals_sel_idx = idx.min(self.filtered_signals().count().saturating_sub(1));
            }
            ClickAction::JumpToSelectedSignal => self.jump_chart_to_selected_signal(),
            ClickAction::PaperBuy => self.paper_market_buy(0),
            ClickAction::PaperSell => self.paper_market_sell(0),
            ClickAction::PaperFlatten => self.paper_flatten(0),
            ClickAction::PaperCancelAll => self.paper_cancel_all(),
            ClickAction::PaperQtyInc => self.paper_qty_inc(),
            ClickAction::PaperQtyDec => self.paper_qty_dec(),
            ClickAction::ToggleEma(i) => self.chart.toggle_ema(i as usize),
            ClickAction::ToggleVwap => self.chart.toggle_vwap(),
            ClickAction::ToggleVolumeProfile => self.chart.toggle_volume_profile(),
            ClickAction::ToggleCumDelta => self.chart.toggle_cum_delta(),
            ClickAction::ToggleIndicatorPanel => self.chart.toggle_indicator_panel(),
        }
    }

    pub fn record_working_orders_sample(&mut self) {
        let n = self
            .paper
            .orders
            .iter()
            .filter(|o| {
                matches!(
                    o.status,
                    crate::paper::OrderStatus::Working | crate::paper::OrderStatus::PartialFill
                )
            })
            .count() as f64;
        self.working_orders_history.push_back(n);
        while self.working_orders_history.len() > 120 {
            self.working_orders_history.pop_front();
        }
    }

    /// Sample the active book's spread + top-N imbalance for graphs.
    pub fn record_spread_sample(&mut self) {
        if let Some(spread) = self.active.book.spread() {
            let s: f64 = spread.try_into().unwrap_or(0.0);
            self.spread_history.push_back(s);
            while self.spread_history.len() > 120 {
                self.spread_history.pop_front();
            }
        }
        if let Some(mid) = self.active.book.mid() {
            let m: f64 = mid.try_into().unwrap_or(0.0);
            self.mid_history.push_back(m);
            while self.mid_history.len() > 240 {
                self.mid_history.pop_front();
            }
        }
        // Top-10 imbalance: (Σ bid qty − Σ ask qty) / (sum). In [-1, +1].
        let bids: rust_decimal::Decimal = self
            .active
            .book
            .bids
            .iter_desc()
            .take(10)
            .map(|(_, q)| q)
            .sum();
        let asks: rust_decimal::Decimal = self
            .active
            .book
            .asks
            .iter_asc()
            .take(10)
            .map(|(_, q)| q)
            .sum();
        let total = bids + asks;
        if !total.is_zero() {
            let imb: f64 = ((bids - asks) / total).try_into().unwrap_or(0.0);
            self.imbalance_history.push_back(imb);
            while self.imbalance_history.len() > 120 {
                self.imbalance_history.pop_front();
            }
        }
    }

    /// Sample the rolling trades-per-second pace; called on every trade.
    pub fn record_tps_sample(&mut self) {
        let cur = self.active.signals.pace.current_tps();
        self.tps_history.push_back(cur);
        while self.tps_history.len() > 120 {
            self.tps_history.pop_front();
        }
    }

    pub fn add_watchlist(&mut self, sym: impl Into<String>) {
        let sym = sym.into();
        if self.watchlist.len() >= crate::multi::MAX_WATCHED {
            return;
        }
        if self
            .watchlist
            .iter()
            .any(|s| s.symbol.eq_ignore_ascii_case(&sym))
        {
            return;
        }
        self.watchlist.push(SymbolSummary::new(sym));
    }

    /// Detect any new paper fills since the last call and forward each
    /// to the analytics engine.
    fn sync_analytics(&mut self, time_ms: i64) {
        let signals_at_entry = self.active.signals.log.len() as u32;
        let above_vwap = self
            .active
            .candles
            .session_vwap()
            .and_then(|v| self.active.last_price().map(|p| p > v));
        let aligned_with_delta: Option<bool> = None;
        let ctx = EntryContext {
            above_vwap,
            aligned_with_delta,
            signals_at_entry,
        };
        let mut new_fills: Vec<(u64, delta::Side, Decimal, Decimal, i64)> = Vec::new();
        for o in self.paper.orders.iter() {
            let prev = self
                .paper_seen_filled
                .get(&o.id)
                .copied()
                .unwrap_or(Decimal::ZERO);
            if o.filled_qty > prev {
                new_fills.push((
                    o.id,
                    o.side,
                    o.filled_qty - prev,
                    o.avg_fill_price,
                    o.last_event_ms.max(time_ms),
                ));
            }
        }
        for (id, side, q, px, t) in new_fills {
            self.analytics.record_fill(side, q, px, t, ctx.clone(), None, None);
            self.paper_seen_filled.insert(
                id,
                self.paper
                    .orders
                    .iter()
                    .find(|o| o.id == id)
                    .map(|o| o.filled_qty)
                    .unwrap_or(Decimal::ZERO),
            );
        }
    }

    pub fn toggle_trading_mode(&mut self) {
        self.paper.trading_mode = !self.paper.trading_mode;
    }

    pub fn cycle_signals_filter(&mut self) {
        use crate::signals::SignalKind::*;
        self.signals_filter = match self.signals_filter {
            None => Some(Iceberg),
            Some(Iceberg) => Some(Absorption),
            Some(Absorption) => Some(PaceSpike),
            Some(PaceSpike) => Some(StopRun),
            Some(StopRun) => Some(Exhaustion),
            Some(Exhaustion) => None,
        };
    }

    pub fn last_chart_price(&self) -> Option<f64> {
        self.active.last_price()
    }

    pub fn aggressor_label(&self, side: delta::Side) -> &'static str {
        match side {
            delta::Side::Buy => "BUY",
            delta::Side::Sell => "SELL",
        }
    }

    /// Iterate every running symbol-engine: active + others.
    pub fn all_engines(&self) -> impl Iterator<Item = &SymbolEngines> + '_ {
        std::iter::once(&self.active).chain(self.others.iter())
    }

    pub fn engines_count(&self) -> usize {
        1 + self.others.len()
    }

    pub fn conn(&self) -> ConnState {
        self.active.conn
    }

    pub fn event_count(&self) -> u64 {
        self.active.event_count
    }

    pub fn symbol(&self) -> &str {
        &self.active.symbol
    }

    pub fn has_snapshot(&self) -> bool {
        self.active.has_snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::Side;
    use crate::engines::TAPE_CAP;
    use crate::feed::Trade;
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
        assert_eq!(app.active.tape.len(), TAPE_CAP);
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
        assert_eq!(app.active.tape.front().unwrap().trade.time_ms, 2);
    }

    #[test]
    fn large_trade_flag_triggers_above_threshold() {
        let mut app = AppState::new("BTCUSDT");
        for _ in 0..30 {
            app.handle(FeedEvent::Trade(t(Side::Buy, dec!(1))));
        }
        app.handle(FeedEvent::Trade(t(Side::Sell, dec!(10))));
        assert!(app.active.tape.front().unwrap().large);
        app.handle(FeedEvent::Trade(t(Side::Sell, dec!(1))));
        assert!(!app.active.tape.front().unwrap().large);
    }

    #[test]
    fn disconnect_marks_book_stale() {
        let mut app = AppState::new("BTCUSDT");
        app.handle(FeedEvent::Connected);
        app.handle(FeedEvent::Disconnected);
        assert!(app.active.book.is_stale());
        assert_eq!(app.active.conn, ConnState::Disconnected);
    }

    #[test]
    fn reset_session_zeros_delta_and_resets_all_symbols() {
        let mut app = AppState::new("BTCUSDT");
        app.add_symbol("ETHUSDT");
        app.handle(FeedEvent::Trade(t(Side::Buy, dec!(2))));
        app.handle(FeedEvent::Trade(t(Side::Sell, dec!(1))));
        app.handle_other(0, FeedEvent::Trade(t(Side::Buy, dec!(3))));
        assert_eq!(app.active.delta.delta(), dec!(1));
        assert_eq!(app.others[0].delta.delta(), dec!(3));
        app.reset_session();
        assert_eq!(app.active.delta.delta(), Decimal::ZERO);
        assert_eq!(app.others[0].delta.delta(), Decimal::ZERO);
    }

    #[test]
    fn footprint_totals_match_delta_totals_within_one_bar() {
        let mut app = AppState::new("BTCUSDT");
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
        let bar = app.active.footprint.forming().unwrap();
        assert_eq!(bar.total_buy, app.active.delta.buy_volume());
        assert_eq!(bar.total_sell, app.active.delta.sell_volume());
        assert_eq!(bar.delta(), app.active.delta.delta());
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

    #[test]
    fn switch_active_swaps_engines_in_place() {
        let mut app = AppState::new("BTCUSDT");
        app.add_symbol("ETHUSDT");
        // Drive each side a bit.
        app.handle(FeedEvent::Trade(t(Side::Buy, dec!(1))));
        app.handle_other(
            0,
            FeedEvent::Trade(Trade {
                time_ms: 0,
                price: dec!(2000),
                qty: dec!(0.5),
                side: Side::Sell,
            }),
        );

        assert_eq!(app.active.symbol, "BTCUSDT");
        app.switch_active("ETHUSDT");
        assert_eq!(app.active.symbol, "ETHUSDT");
        assert_eq!(app.active.delta.sell_volume(), dec!(0.5));
        // The previous BTCUSDT engine is preserved in others.
        let btc = app.others.iter().find(|s| s.symbol == "BTCUSDT").unwrap();
        assert_eq!(btc.delta.buy_volume(), dec!(1));
    }
}
