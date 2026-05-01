use rust_decimal::Decimal;

/// Aggressor side of a trade: did a buyer hit the ask, or a seller hit the bid?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

/// Binance trade stream marks `m=true` when the buyer is the maker — i.e.
/// the taker (aggressor) is the seller. So `m=false` => Buy aggressor.
pub fn aggressor_from_buyer_maker(buyer_is_maker: bool) -> Side {
    if buyer_is_maker { Side::Sell } else { Side::Buy }
}

#[derive(Debug, Default, Clone)]
pub struct DeltaTracker {
    buy_volume: Decimal,
    sell_volume: Decimal,
    trade_count: u64,
}

impl DeltaTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, side: Side, qty: Decimal) {
        if qty.is_sign_negative() || qty.is_zero() {
            return;
        }
        match side {
            Side::Buy => self.buy_volume += qty,
            Side::Sell => self.sell_volume += qty,
        }
        self.trade_count += 1;
    }

    pub fn reset(&mut self) {
        self.buy_volume = Decimal::ZERO;
        self.sell_volume = Decimal::ZERO;
        self.trade_count = 0;
    }

    pub fn buy_volume(&self) -> Decimal {
        self.buy_volume
    }

    pub fn sell_volume(&self) -> Decimal {
        self.sell_volume
    }

    pub fn total_volume(&self) -> Decimal {
        self.buy_volume + self.sell_volume
    }

    pub fn delta(&self) -> Decimal {
        self.buy_volume - self.sell_volume
    }

    pub fn trade_count(&self) -> u64 {
        self.trade_count
    }

    /// Buy share of total volume, in [0, 1]. None when no trades.
    pub fn buy_ratio(&self) -> Option<f64> {
        let total = self.total_volume();
        if total.is_zero() {
            return None;
        }
        let buy: f64 = self.buy_volume.try_into().unwrap_or(0.0);
        let tot: f64 = total.try_into().unwrap_or(0.0);
        if tot == 0.0 { None } else { Some(buy / tot) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn aggressor_classification() {
        assert_eq!(aggressor_from_buyer_maker(true), Side::Sell);
        assert_eq!(aggressor_from_buyer_maker(false), Side::Buy);
    }

    #[test]
    fn empty_tracker_is_zero() {
        let d = DeltaTracker::new();
        assert_eq!(d.buy_volume(), Decimal::ZERO);
        assert_eq!(d.sell_volume(), Decimal::ZERO);
        assert_eq!(d.delta(), Decimal::ZERO);
        assert_eq!(d.total_volume(), Decimal::ZERO);
        assert_eq!(d.buy_ratio(), None);
        assert_eq!(d.trade_count(), 0);
    }

    #[test]
    fn buys_increase_positive_delta() {
        let mut d = DeltaTracker::new();
        d.record(Side::Buy, dec!(1.5));
        d.record(Side::Buy, dec!(2.5));
        assert_eq!(d.buy_volume(), dec!(4.0));
        assert_eq!(d.delta(), dec!(4.0));
        assert_eq!(d.trade_count(), 2);
    }

    #[test]
    fn sells_make_delta_negative() {
        let mut d = DeltaTracker::new();
        d.record(Side::Buy, dec!(1));
        d.record(Side::Sell, dec!(3));
        assert_eq!(d.delta(), dec!(-2));
        assert_eq!(d.total_volume(), dec!(4));
    }

    #[test]
    fn buy_ratio_reflects_proportion() {
        let mut d = DeltaTracker::new();
        d.record(Side::Buy, dec!(3));
        d.record(Side::Sell, dec!(1));
        let r = d.buy_ratio().unwrap();
        assert!((r - 0.75).abs() < 1e-9);
    }

    #[test]
    fn reset_zeroes_everything() {
        let mut d = DeltaTracker::new();
        d.record(Side::Buy, dec!(10));
        d.record(Side::Sell, dec!(5));
        d.reset();
        assert_eq!(d.buy_volume(), Decimal::ZERO);
        assert_eq!(d.sell_volume(), Decimal::ZERO);
        assert_eq!(d.delta(), Decimal::ZERO);
        assert_eq!(d.trade_count(), 0);
    }

    #[test]
    fn zero_and_negative_qty_ignored() {
        let mut d = DeltaTracker::new();
        d.record(Side::Buy, dec!(0));
        d.record(Side::Sell, dec!(-1));
        assert_eq!(d.trade_count(), 0);
        assert_eq!(d.total_volume(), Decimal::ZERO);
    }

    #[test]
    fn precision_holds_across_many_small_trades() {
        // 1000 trades of 0.001 each — Decimal must sum exactly to 1.000.
        let mut d = DeltaTracker::new();
        for _ in 0..1000 {
            d.record(Side::Buy, dec!(0.001));
        }
        assert_eq!(d.buy_volume(), dec!(1.000));
    }
}
