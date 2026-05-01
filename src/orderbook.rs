use std::collections::BTreeMap;

use rust_decimal::Decimal;
use serde::Deserialize;

/// One side of the book, keyed by price (Decimal -> exact equality).
/// Bids: iterate in reverse for descending order. Asks: iterate forward.
#[derive(Debug, Clone, Default)]
pub struct Side {
    levels: BTreeMap<Decimal, Decimal>,
}

impl Side {
    pub fn new() -> Self {
        Self { levels: BTreeMap::new() }
    }

    pub fn clear(&mut self) {
        self.levels.clear();
    }

    /// Apply a single (price, qty) update. qty == 0 removes the level.
    pub fn apply(&mut self, price: Decimal, qty: Decimal) {
        if qty.is_zero() {
            self.levels.remove(&price);
        } else {
            self.levels.insert(price, qty);
        }
    }

    pub fn len(&self) -> usize {
        self.levels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.levels.is_empty()
    }

    pub fn get(&self, price: &Decimal) -> Option<Decimal> {
        self.levels.get(price).copied()
    }

    /// Bids: iterate descending (highest price first).
    pub fn iter_desc(&self) -> impl Iterator<Item = (Decimal, Decimal)> + '_ {
        self.levels.iter().rev().map(|(p, q)| (*p, *q))
    }

    /// Asks: iterate ascending (lowest price first).
    pub fn iter_asc(&self) -> impl Iterator<Item = (Decimal, Decimal)> + '_ {
        self.levels.iter().map(|(p, q)| (*p, *q))
    }

    pub fn best_desc(&self) -> Option<(Decimal, Decimal)> {
        self.levels.iter().next_back().map(|(p, q)| (*p, *q))
    }

    pub fn best_asc(&self) -> Option<(Decimal, Decimal)> {
        self.levels.iter().next().map(|(p, q)| (*p, *q))
    }
}

#[derive(Debug, Clone, Default)]
pub struct OrderBook {
    pub bids: Side,
    pub asks: Side,
    /// Last applied update id from feed. None = no snapshot yet.
    pub last_update_id: Option<u64>,
    /// True when sequence broke and book may be wrong. UI must show staleness.
    pub stale: bool,
}

/// Raw price/qty pair as Binance sends them — strings, parsed to Decimal.
#[derive(Debug, Deserialize, Clone)]
pub struct PriceLevelStr(pub String, pub String);

/// REST depth snapshot.
#[derive(Debug, Deserialize)]
pub struct DepthSnapshot {
    #[serde(rename = "lastUpdateId")]
    pub last_update_id: u64,
    pub bids: Vec<PriceLevelStr>,
    pub asks: Vec<PriceLevelStr>,
}

/// Diff depth event from `<symbol>@depth` stream.
#[derive(Debug, Deserialize)]
pub struct DiffDepthEvent {
    #[serde(rename = "U")]
    pub first_update_id: u64,
    #[serde(rename = "u")]
    pub final_update_id: u64,
    #[serde(rename = "b")]
    pub bids: Vec<PriceLevelStr>,
    #[serde(rename = "a")]
    pub asks: Vec<PriceLevelStr>,
}

/// Outcome of applying a diff event.
#[derive(Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Event applied successfully.
    Applied,
    /// Event predates snapshot — drop silently.
    Dropped,
    /// Sequence gap detected. Book is now stale; caller must resync.
    OutOfSync,
    /// Snapshot not yet loaded.
    NoSnapshot,
}

impl OrderBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.bids.clear();
        self.asks.clear();
        self.last_update_id = None;
        self.stale = false;
    }

    /// Replace book contents from a REST snapshot.
    pub fn apply_snapshot(&mut self, snap: &DepthSnapshot) -> Result<(), rust_decimal::Error> {
        self.bids.clear();
        self.asks.clear();
        for PriceLevelStr(p, q) in &snap.bids {
            let price: Decimal = p.parse()?;
            let qty: Decimal = q.parse()?;
            self.bids.apply(price, qty);
        }
        for PriceLevelStr(p, q) in &snap.asks {
            let price: Decimal = p.parse()?;
            let qty: Decimal = q.parse()?;
            self.asks.apply(price, qty);
        }
        self.last_update_id = Some(snap.last_update_id);
        self.stale = false;
        Ok(())
    }

    /// Apply a diff event using Binance's order-book maintenance protocol:
    /// drop events with `u` <= snapshot's lastUpdateId; the first applied
    /// event must satisfy `U <= lastUpdateId+1 <= u`; thereafter each
    /// event's `U` must equal previous `u + 1`. Any gap → stale.
    pub fn apply_diff(&mut self, ev: &DiffDepthEvent) -> ApplyOutcome {
        let Some(last) = self.last_update_id else {
            return ApplyOutcome::NoSnapshot;
        };

        if ev.final_update_id <= last {
            return ApplyOutcome::Dropped;
        }

        let expected_first = last + 1;
        // First event after snapshot OR continuing stream — both must cover expected_first.
        if !(ev.first_update_id <= expected_first && expected_first <= ev.final_update_id) {
            self.stale = true;
            return ApplyOutcome::OutOfSync;
        }

        for PriceLevelStr(p, q) in &ev.bids {
            let Ok(price) = p.parse::<Decimal>() else {
                self.stale = true;
                return ApplyOutcome::OutOfSync;
            };
            let Ok(qty) = q.parse::<Decimal>() else {
                self.stale = true;
                return ApplyOutcome::OutOfSync;
            };
            self.bids.apply(price, qty);
        }
        for PriceLevelStr(p, q) in &ev.asks {
            let Ok(price) = p.parse::<Decimal>() else {
                self.stale = true;
                return ApplyOutcome::OutOfSync;
            };
            let Ok(qty) = q.parse::<Decimal>() else {
                self.stale = true;
                return ApplyOutcome::OutOfSync;
            };
            self.asks.apply(price, qty);
        }

        self.last_update_id = Some(ev.final_update_id);
        ApplyOutcome::Applied
    }

    pub fn best_bid(&self) -> Option<(Decimal, Decimal)> {
        self.bids.best_desc()
    }

    pub fn best_ask(&self) -> Option<(Decimal, Decimal)> {
        self.asks.best_asc()
    }

    pub fn spread(&self) -> Option<Decimal> {
        Some(self.best_ask()?.0 - self.best_bid()?.0)
    }

    pub fn mid(&self) -> Option<Decimal> {
        let bb = self.best_bid()?.0;
        let ba = self.best_ask()?.0;
        Some((bb + ba) / Decimal::from(2))
    }

    pub fn mark_stale(&mut self) {
        self.stale = true;
    }

    pub fn is_stale(&self) -> bool {
        self.stale
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn lvl(p: &str, q: &str) -> PriceLevelStr {
        PriceLevelStr(p.to_string(), q.to_string())
    }

    fn snap(id: u64, bids: Vec<(&str, &str)>, asks: Vec<(&str, &str)>) -> DepthSnapshot {
        DepthSnapshot {
            last_update_id: id,
            bids: bids.into_iter().map(|(p, q)| lvl(p, q)).collect(),
            asks: asks.into_iter().map(|(p, q)| lvl(p, q)).collect(),
        }
    }

    fn diff(u_first: u64, u_final: u64, bids: Vec<(&str, &str)>, asks: Vec<(&str, &str)>) -> DiffDepthEvent {
        DiffDepthEvent {
            first_update_id: u_first,
            final_update_id: u_final,
            bids: bids.into_iter().map(|(p, q)| lvl(p, q)).collect(),
            asks: asks.into_iter().map(|(p, q)| lvl(p, q)).collect(),
        }
    }

    #[test]
    fn snapshot_loads_levels_and_sets_id() {
        let mut book = OrderBook::new();
        let s = snap(100, vec![("99", "1.0"), ("98", "2.0")], vec![("101", "1.5"), ("102", "0.5")]);
        book.apply_snapshot(&s).unwrap();
        assert_eq!(book.last_update_id, Some(100));
        assert_eq!(book.best_bid(), Some((dec!(99), dec!(1.0))));
        assert_eq!(book.best_ask(), Some((dec!(101), dec!(1.5))));
        assert_eq!(book.spread(), Some(dec!(2)));
    }

    #[test]
    fn bids_iterate_descending() {
        let mut book = OrderBook::new();
        let s = snap(1, vec![("99", "1"), ("98", "1"), ("100", "1")], vec![]);
        book.apply_snapshot(&s).unwrap();
        let prices: Vec<Decimal> = book.bids.iter_desc().map(|(p, _)| p).collect();
        assert_eq!(prices, vec![dec!(100), dec!(99), dec!(98)]);
    }

    #[test]
    fn asks_iterate_ascending() {
        let mut book = OrderBook::new();
        let s = snap(1, vec![], vec![("101", "1"), ("103", "1"), ("102", "1")]);
        book.apply_snapshot(&s).unwrap();
        let prices: Vec<Decimal> = book.asks.iter_asc().map(|(p, _)| p).collect();
        assert_eq!(prices, vec![dec!(101), dec!(102), dec!(103)]);
    }

    #[test]
    fn zero_qty_removes_level() {
        let mut book = OrderBook::new();
        book.apply_snapshot(&snap(10, vec![("99", "1.0")], vec![("101", "1.0")])).unwrap();
        let ev = diff(11, 12, vec![("99", "0")], vec![]);
        assert_eq!(book.apply_diff(&ev), ApplyOutcome::Applied);
        assert!(book.bids.is_empty());
        assert_eq!(book.last_update_id, Some(12));
    }

    #[test]
    fn diff_before_snapshot_is_dropped() {
        let mut book = OrderBook::new();
        book.apply_snapshot(&snap(100, vec![("99", "1")], vec![("101", "1")])).unwrap();
        let ev = diff(50, 99, vec![("99", "9")], vec![]);
        assert_eq!(book.apply_diff(&ev), ApplyOutcome::Dropped);
        assert_eq!(book.bids.get(&dec!(99)), Some(dec!(1))); // unchanged
        assert_eq!(book.last_update_id, Some(100));
    }

    #[test]
    fn first_diff_must_cover_snapshot_plus_one() {
        let mut book = OrderBook::new();
        book.apply_snapshot(&snap(100, vec![("99", "1")], vec![("101", "1")])).unwrap();
        // Valid: U=95, u=105 → covers 101
        let ev = diff(95, 105, vec![("99", "2")], vec![]);
        assert_eq!(book.apply_diff(&ev), ApplyOutcome::Applied);
        assert_eq!(book.bids.get(&dec!(99)), Some(dec!(2)));
    }

    #[test]
    fn gap_between_diffs_marks_stale() {
        let mut book = OrderBook::new();
        book.apply_snapshot(&snap(100, vec![("99", "1")], vec![("101", "1")])).unwrap();
        assert_eq!(book.apply_diff(&diff(101, 105, vec![("99", "2")], vec![])), ApplyOutcome::Applied);
        // Next event must have U == 106, simulate gap with U=110.
        let gap = diff(110, 115, vec![("99", "3")], vec![]);
        assert_eq!(book.apply_diff(&gap), ApplyOutcome::OutOfSync);
        assert!(book.is_stale());
    }

    #[test]
    fn no_snapshot_yields_no_snapshot() {
        let mut book = OrderBook::new();
        let ev = diff(1, 2, vec![("99", "1")], vec![]);
        assert_eq!(book.apply_diff(&ev), ApplyOutcome::NoSnapshot);
    }

    #[test]
    fn precise_decimal_prices_avoid_float_pitfalls() {
        // 0.1 + 0.2 in f64 != 0.3. Decimal must equate exactly.
        let mut book = OrderBook::new();
        book.apply_snapshot(&snap(1, vec![("0.30000000", "1.5")], vec![("0.40000000", "2.5")])).unwrap();
        assert_eq!(book.best_bid(), Some((dec!(0.3), dec!(1.5))));
        let ev = diff(2, 3, vec![("0.30000000", "0")], vec![]);
        assert_eq!(book.apply_diff(&ev), ApplyOutcome::Applied);
        assert!(book.bids.is_empty());
    }

    #[test]
    fn reset_clears_book() {
        let mut book = OrderBook::new();
        book.apply_snapshot(&snap(1, vec![("99", "1")], vec![("101", "1")])).unwrap();
        book.mark_stale();
        book.reset();
        assert!(book.bids.is_empty());
        assert!(book.asks.is_empty());
        assert_eq!(book.last_update_id, None);
        assert!(!book.is_stale());
    }
}
