//! Paper-trading engine. Owns the simulated order book of resting working
//! orders, the position, and the account. Fills are driven by:
//!   - the live order-book (for market orders walking the visible depth)
//!   - the live trade tape (for resting limit orders crossing through)
//!
//! No real broker is used. PnL is computed at the trade level.

use std::collections::VecDeque;

use rust_decimal::Decimal;

use crate::delta::Side;
use crate::feed::Trade;
use crate::orderbook::OrderBook;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderType {
    Market,
    Limit,
    Stop,
    StopLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStatus {
    Pending,
    Working,
    PartialFill,
    Filled,
    Cancelled,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BracketRole {
    /// Standalone (default).
    None,
    /// The protective stop attached to a parent entry order.
    StopOf(u64),
    /// The take-profit attached to a parent entry order.
    TargetOf(u64),
}

#[derive(Debug, Clone)]
pub struct Order {
    pub id: u64,
    pub kind: OrderType,
    pub side: Side,
    pub qty: Decimal,
    pub price: Option<Decimal>,
    pub stop_price: Option<Decimal>,
    pub status: OrderStatus,
    pub filled_qty: Decimal,
    pub avg_fill_price: Decimal,
    pub trail_ticks: Option<Decimal>,
    pub trail_extreme: Option<Decimal>,
    pub bracket: BracketRole,
    pub created_ms: i64,
    pub last_event_ms: i64,
    pub reject_reason: Option<String>,
    /// Slippage accumulated by this order (intended − filled, weighted by qty).
    pub slippage: Decimal,
    /// Snapshot of the contra-best at submission time — the reference price
    /// for slippage on a market order. None when nothing was known.
    pub intended_price: Option<Decimal>,
}

impl Order {
    pub fn remaining(&self) -> Decimal {
        self.qty - self.filled_qty
    }
}

#[derive(Debug, Clone, Default)]
pub struct Position {
    /// Signed quantity. Positive = long, negative = short.
    pub qty: Decimal,
    pub avg_price: Decimal,
    pub realized_pnl: Decimal,
}

impl Position {
    pub fn unrealized(&self, mark: Decimal) -> Decimal {
        if self.qty.is_zero() {
            return Decimal::ZERO;
        }
        (mark - self.avg_price) * self.qty
    }

    /// Apply an executed fill at `price` for `signed_qty` (positive=buy, negative=sell).
    /// Updates avg_price + realized_pnl.
    fn apply_fill(&mut self, price: Decimal, signed_qty: Decimal) {
        if signed_qty.is_zero() {
            return;
        }
        if self.qty.is_zero() {
            self.qty = signed_qty;
            self.avg_price = price;
            return;
        }
        let same_side = self.qty.is_sign_positive() == signed_qty.is_sign_positive();
        if same_side {
            // Average up.
            let new_qty = self.qty + signed_qty;
            self.avg_price = (self.avg_price * self.qty + price * signed_qty) / new_qty;
            self.qty = new_qty;
        } else {
            // Closing or flipping.
            let close_qty = if signed_qty.abs() > self.qty.abs() {
                self.qty
            } else {
                -signed_qty
            };
            // Realized: (close_price - avg_price) * close_qty (signed).
            self.realized_pnl += (price - self.avg_price) * close_qty;
            let new_qty = self.qty + signed_qty;
            if new_qty.is_zero() {
                self.qty = Decimal::ZERO;
                self.avg_price = Decimal::ZERO;
            } else if new_qty.is_sign_positive() != self.qty.is_sign_positive() {
                // Flipped through zero.
                self.qty = new_qty;
                self.avg_price = price;
            } else {
                self.qty = new_qty;
                // avg unchanged when partially closing
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct AccountConfig {
    pub starting_balance: Decimal,
    pub max_position_qty: Decimal,
    pub max_daily_loss: Decimal,
}

impl Default for AccountConfig {
    fn default() -> Self {
        Self {
            starting_balance: rust_decimal_macros::dec!(10_000),
            max_position_qty: rust_decimal_macros::dec!(5),
            max_daily_loss: rust_decimal_macros::dec!(500),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PaperEngine {
    pub config: AccountConfig,
    next_id: u64,
    pub orders: VecDeque<Order>,
    pub position: Position,
    pub trading_mode: bool,
    /// Snapshot of realized at session start for daily-loss accounting.
    session_start_realized: Decimal,
    pub last_mark: Option<Decimal>,
    /// Top-of-book snapshot used for sizing slippage references on market
    /// orders. `(best_bid, best_ask)` from the most recent book update.
    pub last_best_bid: Option<Decimal>,
    pub last_best_ask: Option<Decimal>,
    pub session_slippage: Decimal,
}

impl Default for PaperEngine {
    fn default() -> Self {
        Self::new(AccountConfig::default())
    }
}

impl PaperEngine {
    pub fn new(config: AccountConfig) -> Self {
        Self {
            config,
            next_id: 1,
            orders: VecDeque::new(),
            position: Position::default(),
            trading_mode: true,
            session_start_realized: Decimal::ZERO,
            last_mark: None,
            last_best_bid: None,
            last_best_ask: None,
            session_slippage: Decimal::ZERO,
        }
    }

    pub fn balance(&self) -> Decimal {
        self.config.starting_balance + self.position.realized_pnl
    }

    pub fn unrealized_pnl(&self) -> Decimal {
        self.last_mark
            .map(|m| self.position.unrealized(m))
            .unwrap_or(Decimal::ZERO)
    }

    pub fn daily_loss(&self) -> Decimal {
        // Negative if we're losing; positive if we've made money.
        self.position.realized_pnl - self.session_start_realized
    }

    pub fn daily_loss_buffer(&self) -> Decimal {
        // Remaining loss capacity before the daily limit kicks in.
        self.config.max_daily_loss + self.daily_loss()
    }

    pub fn trading_locked(&self) -> bool {
        // Locked when daily loss has met or exceeded the configured limit.
        self.daily_loss() <= -self.config.max_daily_loss
    }

    fn issue_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn submit(&mut self, mut order: Order) -> u64 {
        order.id = self.issue_id();
        order.created_ms = order.created_ms.max(0);
        // Capture an intended-price snapshot for slippage attribution.
        // For market orders we use the best contra-side price at submission;
        // for resting limit/stop orders we use their own configured price
        // since the user explicitly chose where they wanted to fill.
        if order.intended_price.is_none() {
            order.intended_price = match order.kind {
                // For a market order, slippage reference is the contra-side
                // best at submission — the price we would theoretically take.
                OrderType::Market => match order.side {
                    Side::Buy => self.last_best_ask.or(self.last_mark),
                    Side::Sell => self.last_best_bid.or(self.last_mark),
                },
                OrderType::Limit | OrderType::StopLimit => order.price,
                OrderType::Stop => order.stop_price,
            };
        }
        if !self.trading_mode {
            order.status = OrderStatus::Rejected;
            order.reject_reason = Some("trading mode disabled".into());
            self.orders.push_back(order.clone());
            return order.id;
        }
        if self.trading_locked() {
            order.status = OrderStatus::Rejected;
            order.reject_reason = Some("daily loss limit hit".into());
            self.orders.push_back(order.clone());
            return order.id;
        }
        // Position-limit pre-check: would the order, if fully filled, push
        // |position| over the configured maximum?
        let signed = match order.side {
            Side::Buy => order.qty,
            Side::Sell => -order.qty,
        };
        let projected = self.position.qty + signed;
        if projected.abs() > self.config.max_position_qty {
            order.status = OrderStatus::Rejected;
            order.reject_reason = Some("position limit".into());
            self.orders.push_back(order.clone());
            return order.id;
        }
        order.status = OrderStatus::Working;
        let id = order.id;
        self.orders.push_back(order);
        id
    }

    pub fn cancel(&mut self, id: u64) -> bool {
        for o in self.orders.iter_mut() {
            if o.id == id && matches!(o.status, OrderStatus::Working | OrderStatus::PartialFill | OrderStatus::Pending) {
                o.status = OrderStatus::Cancelled;
                return true;
            }
        }
        false
    }

    pub fn cancel_all(&mut self) {
        for o in self.orders.iter_mut() {
            if matches!(o.status, OrderStatus::Working | OrderStatus::PartialFill | OrderStatus::Pending) {
                o.status = OrderStatus::Cancelled;
            }
        }
    }

    /// Send a flatten — cancels working orders and submits a market order
    /// to close the current position.
    pub fn flatten(&mut self, time_ms: i64) -> Option<u64> {
        self.cancel_all();
        if self.position.qty.is_zero() {
            return None;
        }
        let side = if self.position.qty.is_sign_positive() {
            Side::Sell
        } else {
            Side::Buy
        };
        let qty = self.position.qty.abs();
        Some(self.submit(Order {
            id: 0,
            kind: OrderType::Market,
            side,
            qty,
            price: None,
            stop_price: None,
            status: OrderStatus::Pending,
            filled_qty: Decimal::ZERO,
            avg_fill_price: Decimal::ZERO,
            trail_ticks: None,
            trail_extreme: None,
            bracket: BracketRole::None,
            created_ms: time_ms,
            last_event_ms: time_ms,
            reject_reason: None,
            slippage: Decimal::ZERO,
            intended_price: None,
        }))
    }

    /// On every book update: try to fill any pending market orders by
    /// walking the current visible depth.
    pub fn on_book_update(&mut self, book: &OrderBook, time_ms: i64) {
        // Collect the snapshot of best levels BEFORE iterating orders so we
        // don't borrow `book` mutably (it isn't, but stay consistent).
        let asks: Vec<(Decimal, Decimal)> = book.asks.iter_asc().take(50).collect();
        let bids: Vec<(Decimal, Decimal)> = book.bids.iter_desc().take(50).collect();
        for o in self.orders.iter_mut() {
            if !matches!(o.status, OrderStatus::Working | OrderStatus::PartialFill) {
                continue;
            }
            if !matches!(o.kind, OrderType::Market) {
                continue;
            }
            let levels = if matches!(o.side, Side::Buy) { &asks } else { &bids };
            let mut to_fill = o.remaining();
            let mut filled_value = Decimal::ZERO;
            let mut filled_qty = Decimal::ZERO;
            for (px, qty) in levels {
                if to_fill.is_zero() { break; }
                let take = (*qty).min(to_fill);
                if take.is_zero() { continue; }
                filled_value += *px * take;
                filled_qty += take;
                to_fill -= take;
            }
            if filled_qty.is_zero() {
                continue;
            }
            let avg = filled_value / filled_qty;
            let prior_filled = o.filled_qty;
            let prior_avg = o.avg_fill_price;
            let new_total_filled = prior_filled + filled_qty;
            o.avg_fill_price = (prior_avg * prior_filled + avg * filled_qty) / new_total_filled;
            o.filled_qty = new_total_filled;
            o.last_event_ms = time_ms;
            // Slippage relative to first visible top-of-book at time of
            // submission isn't tracked precisely here; we use difference
            // from best price at fill time as a proxy.
            // Slippage = signed (avg_fill − intended_at_submission). Positive
            // value means we paid worse than the reference price; negative
            // means price improvement.
            let intended = o.intended_price.unwrap_or(avg);
            let slip = (avg - intended) * filled_qty
                * if matches!(o.side, Side::Buy) { Decimal::ONE } else { -Decimal::ONE };
            o.slippage += slip;
            self.session_slippage += slip;
            // Apply to position.
            let signed = match o.side {
                Side::Buy => filled_qty,
                Side::Sell => -filled_qty,
            };
            self.position.apply_fill(avg, signed);
            // CRITICAL: a market order is single-shot against the live book.
            // Once we have made any pass over the depth we MUST mark it
            // Filled even if the visible book was thinner than the order
            // size — otherwise subsequent on_book_update calls walk the
            // same untouched depth and double-count the same liquidity.
            // The unfilled remainder is treated as lost capacity.
            o.status = OrderStatus::Filled;
        }
        self.handle_oco();
        // Record top-of-book for slippage reference on future market orders.
        self.last_best_bid = book.best_bid().map(|x| x.0);
        self.last_best_ask = book.best_ask().map(|x| x.0);
        // Mark = midpoint when both sides exist, else whichever side we have.
        self.last_mark = match (self.last_best_bid, self.last_best_ask) {
            (Some(b), Some(a)) => Some((a + b) / Decimal::from(2)),
            (Some(b), None) => Some(b),
            (None, Some(a)) => Some(a),
            (None, None) => self.last_mark,
        };
    }

    /// On every trade: fill resting limit orders, trigger stop orders,
    /// adjust trailing stops, mark to market.
    pub fn on_trade(&mut self, trade: &Trade) {
        let price = trade.price;
        // Limit fills first. CRITICAL: a single trade represents one
        // executed match in the real market. Multiple resting paper
        // orders must SHARE that tape quantity rather than each taking
        // the full amount independently. Track remaining trade qty
        // across the iteration.
        let mut remaining_trade = trade.qty;
        for o in self.orders.iter_mut() {
            if remaining_trade.is_zero() { break; }
            if !matches!(o.status, OrderStatus::Working | OrderStatus::PartialFill) {
                continue;
            }
            match o.kind {
                // NOTE: do not include StopLimit here — an untriggered
                // StopLimit must NOT fill as a Limit. After its stop
                // triggers, the stop-promotion code rewrites its kind to
                // OrderType::Limit, at which point this branch picks it up.
                OrderType::Limit => {
                    let limit = match o.price {
                        Some(p) => p,
                        None => continue,
                    };
                    let crossed = match o.side {
                        Side::Buy => price <= limit,
                        Side::Sell => price >= limit,
                    };
                    if !crossed { continue; }
                    let take = remaining_trade.min(o.remaining());
                    if take.is_zero() { continue; }
                    let prior_filled = o.filled_qty;
                    let prior_avg = o.avg_fill_price;
                    let new_total = prior_filled + take;
                    o.avg_fill_price = (prior_avg * prior_filled + limit * take) / new_total;
                    o.filled_qty = new_total;
                    o.last_event_ms = trade.time_ms;
                    let signed = match o.side { Side::Buy => take, Side::Sell => -take };
                    self.position.apply_fill(limit, signed);
                    remaining_trade -= take;
                    if o.remaining() <= Decimal::ZERO {
                        o.status = OrderStatus::Filled;
                    } else {
                        o.status = OrderStatus::PartialFill;
                    }
                }
                _ => {}
            }
        }

        // Stop & stop-limit triggers.
        let mut promotions: Vec<u64> = Vec::new();
        for o in self.orders.iter_mut() {
            if !matches!(o.status, OrderStatus::Working) { continue; }
            let stop = match o.stop_price { Some(p) => p, None => continue };
            let triggered = match (o.kind, o.side) {
                (OrderType::Stop, Side::Buy) => price >= stop,
                (OrderType::Stop, Side::Sell) => price <= stop,
                (OrderType::StopLimit, Side::Buy) => price >= stop,
                (OrderType::StopLimit, Side::Sell) => price <= stop,
                _ => false,
            };
            if triggered {
                promotions.push(o.id);
            }
        }
        for id in promotions {
            if let Some(o) = self.orders.iter_mut().find(|x| x.id == id) {
                match o.kind {
                    // A plain Stop becomes a Market order — slippage
                    // should reference the (possibly-ratcheted) stop_price
                    // at trigger time, not the original submission stop.
                    OrderType::Stop => {
                        o.intended_price = o.stop_price.or(o.intended_price);
                        o.kind = OrderType::Market;
                    }
                    // A StopLimit becomes a Limit order. Its slippage
                    // reference stays at the user-chosen limit price
                    // (already set as intended_price at submit) —
                    // `o.stop_price` was just the trigger threshold, not
                    // a price commitment.
                    OrderType::StopLimit => {
                        o.kind = OrderType::Limit;
                    }
                    _ => {}
                }
                o.last_event_ms = trade.time_ms;
            }
        }

        // Trailing-stop adjustment for orders with trail_ticks set.
        for o in self.orders.iter_mut() {
            if !matches!(o.status, OrderStatus::Working) { continue; }
            let Some(trail) = o.trail_ticks else { continue; };
            let extreme = o.trail_extreme.get_or_insert(price);
            match o.side {
                // For a SELL stop attached to a long, follow new highs upward.
                Side::Sell => {
                    if price > *extreme {
                        *extreme = price;
                        o.stop_price = Some(*extreme - trail);
                    }
                }
                Side::Buy => {
                    if price < *extreme {
                        *extreme = price;
                        o.stop_price = Some(*extreme + trail);
                    }
                }
            }
        }

        self.handle_oco();
        self.last_mark = Some(price);
    }

    fn handle_oco(&mut self) {
        // For each filled bracket child, cancel the other child of the same parent.
        let mut to_cancel: Vec<u64> = Vec::new();
        for o in self.orders.iter() {
            if matches!(o.status, OrderStatus::Filled) {
                let parent_id = match o.bracket {
                    BracketRole::StopOf(p) | BracketRole::TargetOf(p) => Some(p),
                    BracketRole::None => None,
                };
                if let Some(parent) = parent_id {
                    for sib in self.orders.iter() {
                        if sib.id == o.id { continue; }
                        let sib_parent = match sib.bracket {
                            BracketRole::StopOf(p) | BracketRole::TargetOf(p) => Some(p),
                            BracketRole::None => None,
                        };
                        if sib_parent == Some(parent)
                            && matches!(sib.status, OrderStatus::Working | OrderStatus::PartialFill)
                        {
                            to_cancel.push(sib.id);
                        }
                    }
                }
            }
        }
        for id in to_cancel {
            self.cancel(id);
        }
    }

    /// Submit an entry market order with attached stop-loss and take-profit
    /// (in tick offsets from the entry's expected price). Returns the
    /// entry-order id.
    pub fn submit_bracket(
        &mut self,
        entry_side: Side,
        qty: Decimal,
        expected_entry: Decimal,
        stop_ticks: Decimal,
        target_ticks: Decimal,
        time_ms: i64,
    ) -> u64 {
        let entry_id = self.submit(Order {
            id: 0,
            kind: OrderType::Market,
            side: entry_side,
            qty,
            price: None,
            stop_price: None,
            status: OrderStatus::Pending,
            filled_qty: Decimal::ZERO,
            avg_fill_price: Decimal::ZERO,
            trail_ticks: None,
            trail_extreme: None,
            bracket: BracketRole::None,
            created_ms: time_ms,
            last_event_ms: time_ms,
            reject_reason: None,
            slippage: Decimal::ZERO,
            intended_price: None,
        });

        let (stop_side, stop_price, target_price) = match entry_side {
            Side::Buy => (Side::Sell, expected_entry - stop_ticks, expected_entry + target_ticks),
            Side::Sell => (Side::Buy, expected_entry + stop_ticks, expected_entry - target_ticks),
        };
        self.submit(Order {
            id: 0,
            kind: OrderType::Stop,
            side: stop_side,
            qty,
            price: None,
            stop_price: Some(stop_price),
            status: OrderStatus::Pending,
            filled_qty: Decimal::ZERO,
            avg_fill_price: Decimal::ZERO,
            trail_ticks: None,
            trail_extreme: None,
            bracket: BracketRole::StopOf(entry_id),
            created_ms: time_ms,
            last_event_ms: time_ms,
            reject_reason: None,
            slippage: Decimal::ZERO,
            intended_price: None,
        });
        self.submit(Order {
            id: 0,
            kind: OrderType::Limit,
            side: stop_side,
            qty,
            price: Some(target_price),
            stop_price: None,
            status: OrderStatus::Pending,
            filled_qty: Decimal::ZERO,
            avg_fill_price: Decimal::ZERO,
            trail_ticks: None,
            trail_extreme: None,
            bracket: BracketRole::TargetOf(entry_id),
            created_ms: time_ms,
            last_event_ms: time_ms,
            reject_reason: None,
            slippage: Decimal::ZERO,
            intended_price: None,
        });
        entry_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn book(asks: &[(&str, &str)], bids: &[(&str, &str)]) -> OrderBook {
        let mut b = OrderBook::default();
        for (p, q) in asks {
            b.asks.apply(p.parse().unwrap(), q.parse().unwrap());
        }
        for (p, q) in bids {
            b.bids.apply(p.parse().unwrap(), q.parse().unwrap());
        }
        b
    }

    fn mkt(side: Side, qty: Decimal) -> Order {
        Order {
            id: 0,
            kind: OrderType::Market,
            side,
            qty,
            price: None,
            stop_price: None,
            status: OrderStatus::Pending,
            filled_qty: Decimal::ZERO,
            avg_fill_price: Decimal::ZERO,
            trail_ticks: None,
            trail_extreme: None,
            bracket: BracketRole::None,
            created_ms: 0,
            last_event_ms: 0,
            reject_reason: None,
            slippage: Decimal::ZERO,
            intended_price: None,
        }
    }

    fn lmt(side: Side, qty: Decimal, price: Decimal) -> Order {
        Order { kind: OrderType::Limit, price: Some(price), ..mkt(side, qty) }
    }

    fn stp(side: Side, qty: Decimal, stop_price: Decimal) -> Order {
        Order { kind: OrderType::Stop, stop_price: Some(stop_price), ..mkt(side, qty) }
    }

    fn t(time_ms: i64, price: Decimal, qty: Decimal, side: Side) -> Trade {
        Trade { time_ms, price, qty, side }
    }

    #[test]
    fn market_buy_walks_book_with_slippage() {
        let mut e = PaperEngine::default();
        let id = e.submit(mkt(Side::Buy, dec!(2)));
        let bk = book(&[("100", "1.0"), ("101", "2.0")], &[("99", "5.0")]);
        e.on_book_update(&bk, 0);
        let o = e.orders.iter().find(|o| o.id == id).unwrap();
        assert!(matches!(o.status, OrderStatus::Filled));
        // Avg: (100*1 + 101*1) / 2 = 100.5
        assert_eq!(o.avg_fill_price, dec!(100.5));
        assert_eq!(e.position.qty, dec!(2));
        assert_eq!(e.position.avg_price, dec!(100.5));
    }

    #[test]
    fn limit_fills_when_tape_crosses_through() {
        let mut e = PaperEngine::default();
        let id = e.submit(lmt(Side::Buy, dec!(1.5), dec!(99.50)));
        // Tape trades through @ 99.40 — buy limit @ 99.50 should fill.
        e.on_trade(&t(0, dec!(99.40), dec!(2.0), Side::Sell));
        let o = e.orders.iter().find(|o| o.id == id).unwrap();
        assert!(matches!(o.status, OrderStatus::Filled));
        assert_eq!(o.avg_fill_price, dec!(99.50));
    }

    #[test]
    fn stop_triggers_into_market() {
        let mut e = PaperEngine::default();
        let id = e.submit(stp(Side::Buy, dec!(1), dec!(101)));
        // No trigger at lower prices.
        e.on_trade(&t(0, dec!(100.50), dec!(1), Side::Buy));
        let o = e.orders.iter().find(|o| o.id == id).unwrap();
        assert!(matches!(o.kind, OrderType::Stop));
        // Cross stop.
        e.on_trade(&t(1, dec!(101.20), dec!(0.1), Side::Buy));
        let o = e.orders.iter().find(|o| o.id == id).unwrap();
        assert!(matches!(o.kind, OrderType::Market));
        // Now book update fills it.
        let bk = book(&[("101.50", "5")], &[("100", "5")]);
        e.on_book_update(&bk, 2);
        let o = e.orders.iter().find(|o| o.id == id).unwrap();
        assert!(matches!(o.status, OrderStatus::Filled));
    }

    #[test]
    fn position_avg_price_across_partial_fills() {
        let mut p = Position::default();
        p.apply_fill(dec!(100), dec!(1));
        p.apply_fill(dec!(102), dec!(1));
        assert_eq!(p.qty, dec!(2));
        assert_eq!(p.avg_price, dec!(101));
    }

    #[test]
    fn realized_pnl_on_close() {
        let mut p = Position::default();
        p.apply_fill(dec!(100), dec!(2));
        p.apply_fill(dec!(110), dec!(-2));
        assert_eq!(p.qty, Decimal::ZERO);
        assert_eq!(p.realized_pnl, dec!(20));
    }

    #[test]
    fn position_flips_through_zero() {
        let mut p = Position::default();
        p.apply_fill(dec!(100), dec!(1));
        // Sell 3: closes the long, then opens a -2 short at 110.
        p.apply_fill(dec!(110), dec!(-3));
        assert_eq!(p.qty, dec!(-2));
        assert_eq!(p.avg_price, dec!(110));
        assert_eq!(p.realized_pnl, dec!(10));
    }

    #[test]
    fn bracket_oco_cancels_sibling_when_one_fills() {
        let mut e = PaperEngine::default();
        let entry = e.submit_bracket(Side::Buy, dec!(1), dec!(100), dec!(2), dec!(4), 0);
        // Fill the entry market order first.
        let bk = book(&[("100", "1")], &[("99", "1")]);
        e.on_book_update(&bk, 1);
        // Tape moves up to target 104 → take-profit fills, stop cancels.
        e.on_trade(&t(2, dec!(104), dec!(1), Side::Buy));
        let stop = e.orders.iter().find(|o| matches!(o.bracket, BracketRole::StopOf(p) if p == entry)).unwrap();
        let target = e.orders.iter().find(|o| matches!(o.bracket, BracketRole::TargetOf(p) if p == entry)).unwrap();
        assert!(matches!(target.status, OrderStatus::Filled));
        assert!(matches!(stop.status, OrderStatus::Cancelled));
    }

    #[test]
    fn trailing_stop_follows_favourable_price_only() {
        let mut e = PaperEngine::default();
        let mut o = stp(Side::Sell, dec!(1), dec!(95));
        o.trail_ticks = Some(dec!(5));
        let id = e.submit(o);
        // Long position trailing 5 ticks: extreme starts at first observed price.
        e.on_trade(&t(0, dec!(100), dec!(1), Side::Buy));
        // Price climbs to 110 → stop should ratchet to 105.
        e.on_trade(&t(1, dec!(110), dec!(1), Side::Buy));
        let o = e.orders.iter().find(|o| o.id == id).unwrap();
        assert_eq!(o.stop_price, Some(dec!(105)));
        // Pullback to 107 → stop must NOT move backward.
        e.on_trade(&t(2, dec!(107), dec!(1), Side::Sell));
        let o = e.orders.iter().find(|o| o.id == id).unwrap();
        assert_eq!(o.stop_price, Some(dec!(105)));
    }

    #[test]
    fn position_limit_rejects_oversized_orders() {
        let mut e = PaperEngine::new(AccountConfig {
            max_position_qty: dec!(2),
            ..AccountConfig::default()
        });
        let id = e.submit(mkt(Side::Buy, dec!(5)));
        let o = e.orders.iter().find(|o| o.id == id).unwrap();
        assert!(matches!(o.status, OrderStatus::Rejected));
    }

    #[test]
    fn daily_loss_limit_blocks_new_orders() {
        let mut e = PaperEngine::default();
        // Manually set a loss exceeding the limit.
        e.position.realized_pnl = dec!(-1000);
        assert!(e.trading_locked());
        let id = e.submit(mkt(Side::Buy, dec!(1)));
        let o = e.orders.iter().find(|o| o.id == id).unwrap();
        assert!(matches!(o.status, OrderStatus::Rejected));
    }

    #[test]
    fn flatten_closes_position_and_cancels_working_orders() {
        let mut e = PaperEngine::default();
        // Open long 2.
        e.submit(mkt(Side::Buy, dec!(2)));
        let bk = book(&[("100", "5")], &[("99", "5")]);
        e.on_book_update(&bk, 1);
        // Place a working limit and a working stop.
        e.submit(lmt(Side::Sell, dec!(1), dec!(110)));
        e.submit(stp(Side::Sell, dec!(1), dec!(95)));
        let _ = e.flatten(2);
        let bk = book(&[("100", "5")], &[("99", "5")]);
        e.on_book_update(&bk, 3);
        assert_eq!(e.position.qty, Decimal::ZERO);
        // The two prior working orders must now be cancelled.
        assert!(e.orders.iter().filter(|o| matches!(o.status, OrderStatus::Cancelled)).count() >= 2);
    }

    #[test]
    fn market_order_does_not_refill_from_same_depth_across_book_updates() {
        // Buy 3 against a thin book of only 2 units total. The first walk
        // partial-fills 2. Subsequent on_book_update calls must NOT keep
        // taking the same depth.
        let mut e = PaperEngine::default();
        let bk = book(&[("100", "1.0"), ("101", "1.0")], &[("99", "5.0")]);
        let id = e.submit(mkt(Side::Buy, dec!(3)));
        e.on_book_update(&bk, 1);
        let snap1 = e.orders.iter().find(|o| o.id == id).unwrap().clone();
        // Same book again — must not change anything.
        e.on_book_update(&bk, 2);
        e.on_book_update(&bk, 3);
        let snap2 = e.orders.iter().find(|o| o.id == id).unwrap().clone();
        assert!(matches!(snap1.status, OrderStatus::Filled));
        assert!(matches!(snap2.status, OrderStatus::Filled));
        assert_eq!(snap1.filled_qty, snap2.filled_qty);
        assert_eq!(snap1.filled_qty, dec!(2));
        // Position should reflect only the 2 actually taken on first walk.
        assert_eq!(e.position.qty, dec!(2));
    }

    #[test]
    fn market_buy_slippage_referenced_to_submission_best_ask() {
        let mut e = PaperEngine::default();
        // First, give PaperEngine a top-of-book snapshot via on_book_update
        // so submit can record the right intended_price.
        let bk0 = book(&[("100", "0.5"), ("101", "5.0")], &[("99", "5.0")]);
        e.on_book_update(&bk0, 0);
        // Submit a market buy of 2: walks 100 (0.5) + 101 (1.5) → avg 100.75.
        e.submit(mkt(Side::Buy, dec!(2)));
        e.on_book_update(&bk0, 1);
        let o = &e.orders[0];
        assert!(matches!(o.status, OrderStatus::Filled));
        assert_eq!(o.avg_fill_price, dec!(100.75));
        // Intended price should be best_ask at submission = 100.
        assert_eq!(o.intended_price, Some(dec!(100)));
        // Slippage = (avg − intended) × qty × +1 (buy) = (100.75 − 100) × 2 = 1.5.
        assert_eq!(o.slippage, dec!(1.5));
    }

    #[test]
    fn stop_limit_does_not_fill_until_stop_triggers() {
        // A buy StopLimit at stop=105 with limit=106. While the tape stays
        // below 105, the order is a passive StopLimit and must NOT fill
        // even when price (briefly) crosses the limit price 106 logically
        // — that situation can't happen ahead of stop, but the bug we're
        // guarding is the previous code matching StopLimit in the
        // limit-fill arm.
        let mut e = PaperEngine::default();
        let mut o = lmt(Side::Buy, dec!(1), dec!(106));
        o.kind = OrderType::StopLimit;
        o.stop_price = Some(dec!(105));
        let id = e.submit(o);
        // A trade at 100 (below limit). A buggy implementation would treat
        // the StopLimit like a regular buy-limit at 106 and fill it (since
        // 100 ≤ 106). The fix means it stays Working.
        e.on_trade(&t(0, dec!(100), dec!(1), Side::Sell));
        let o = e.orders.iter().find(|o| o.id == id).unwrap();
        assert!(matches!(o.status, OrderStatus::Working));
        assert!(matches!(o.kind, OrderType::StopLimit));
        // Now move price up through the stop. It should promote to Limit
        // and remain working until the price crosses the limit.
        e.on_trade(&t(1, dec!(105.5), dec!(0.1), Side::Buy));
        let o = e.orders.iter().find(|o| o.id == id).unwrap();
        assert!(matches!(o.kind, OrderType::Limit));
        // Now a trade at 106 (≤ limit for buy) should fill it.
        e.on_trade(&t(2, dec!(106), dec!(1), Side::Sell));
        let o = e.orders.iter().find(|o| o.id == id).unwrap();
        assert!(matches!(o.status, OrderStatus::Filled));
    }

    #[test]
    fn stop_limit_promotion_preserves_limit_intended_price() {
        let mut e = PaperEngine::default();
        let bk = book(&[("110", "5")], &[("100", "5")]);
        e.on_book_update(&bk, 0);
        let mut o = lmt(Side::Buy, dec!(1), dec!(106));
        o.kind = OrderType::StopLimit;
        o.stop_price = Some(dec!(105));
        let id = e.submit(o);
        // Trade crosses the stop — promotes to Limit.
        e.on_trade(&t(0, dec!(105.5), dec!(0.1), Side::Buy));
        let snap = e.orders.iter().find(|o| o.id == id).unwrap().clone();
        assert!(matches!(snap.kind, OrderType::Limit));
        // Slippage reference stays at the LIMIT price (106), not the stop (105).
        assert_eq!(snap.intended_price, Some(dec!(106)));
    }

    #[test]
    fn multiple_limits_at_same_price_share_trade_qty() {
        let mut e = PaperEngine::default();
        // Two buy limits at 100, qty=2 each.
        let id1 = e.submit(lmt(Side::Buy, dec!(2), dec!(100)));
        let id2 = e.submit(lmt(Side::Buy, dec!(2), dec!(100)));
        // A single tape trade of 3 sells at 100.
        e.on_trade(&t(0, dec!(100), dec!(3), Side::Sell));
        // First order filled (took 2), second order partial (took 1).
        let o1 = e.orders.iter().find(|o| o.id == id1).unwrap();
        let o2 = e.orders.iter().find(|o| o.id == id2).unwrap();
        assert_eq!(o1.filled_qty, dec!(2));
        assert!(matches!(o1.status, OrderStatus::Filled));
        assert_eq!(o2.filled_qty, dec!(1));
        assert!(matches!(o2.status, OrderStatus::PartialFill));
        // Position reflects only the 3 actually traded, not 4.
        assert_eq!(e.position.qty, dec!(3));
    }

    #[test]
    fn trailing_stop_slippage_referenced_to_ratcheted_stop() {
        let mut e = PaperEngine::default();
        // Set top-of-book so submit captures intended_price for the stop.
        let bk = book(&[("110", "5")], &[("100", "5")]);
        e.on_book_update(&bk, 0);
        let mut o = stp(Side::Sell, dec!(1), dec!(95));
        o.trail_ticks = Some(dec!(5));
        let id = e.submit(o);
        // Price climbs — trailing stop ratchets from 95 up.
        e.on_trade(&t(0, dec!(100), dec!(1), Side::Buy));
        e.on_trade(&t(1, dec!(110), dec!(1), Side::Buy));
        let snap = e.orders.iter().find(|o| o.id == id).unwrap().clone();
        assert_eq!(snap.stop_price, Some(dec!(105))); // ratcheted to 110−5
        // Price falls and crosses the ratcheted stop — promotes to Market.
        e.on_trade(&t(2, dec!(105), dec!(0.1), Side::Sell));
        let snap = e.orders.iter().find(|o| o.id == id).unwrap().clone();
        assert!(matches!(snap.kind, OrderType::Market));
        // Slippage reference must be the RATCHETED stop (105), not the
        // original submission stop (95).
        assert_eq!(snap.intended_price, Some(dec!(105)));
    }

    #[test]
    fn cancel_individual_order() {
        let mut e = PaperEngine::default();
        let id = e.submit(lmt(Side::Buy, dec!(1), dec!(99)));
        assert!(e.cancel(id));
        let o = e.orders.iter().find(|o| o.id == id).unwrap();
        assert!(matches!(o.status, OrderStatus::Cancelled));
    }
}
