//! Multi-symbol watchlist support.
//!
//! For each watched symbol the application keeps a lightweight
//! [`SymbolSummary`] populated by a side-channel WebSocket connection
//! that consumes only the trade stream (much cheaper than the full depth
//! feed). The user's "active" symbol still drives the heavy engines and
//! UI panels in [`AppState`]; non-active symbols only contribute to the
//! watchlist row.

use std::collections::VecDeque;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::delta;

/// Maximum number of watched symbols (spec cap).
pub const MAX_WATCHED: usize = 8;
/// Sparkline history depth (1-minute close samples).
pub const SPARK_LEN: usize = 30;

#[derive(Debug, Clone)]
pub struct SymbolSummary {
    pub symbol: String,
    pub last_price: f64,
    pub session_open: Option<f64>,
    pub session_buy_volume: f64,
    pub session_sell_volume: f64,
    pub session_high: f64,
    pub session_low: f64,
    pub spread: Option<f64>,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub trade_count: u64,
    pub last_trade_ms: i64,
    /// Rolling 1-minute close samples for inline sparkline.
    pub sparkline: VecDeque<f64>,
    /// Count of signal events emitted while watching (placeholder; light
    /// feeds don't currently run signal detection).
    pub signal_count: u32,
    pub connected: bool,
    /// Active bar boundary (used to fold trades into 1m close samples).
    bar_start_ms: i64,
}

impl SymbolSummary {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            last_price: 0.0,
            session_open: None,
            session_buy_volume: 0.0,
            session_sell_volume: 0.0,
            session_high: f64::NEG_INFINITY,
            session_low: f64::INFINITY,
            spread: None,
            bid: None,
            ask: None,
            trade_count: 0,
            last_trade_ms: 0,
            sparkline: VecDeque::with_capacity(SPARK_LEN + 1),
            signal_count: 0,
            connected: false,
            bar_start_ms: 0,
        }
    }

    pub fn delta(&self) -> f64 {
        self.session_buy_volume - self.session_sell_volume
    }

    pub fn total_volume(&self) -> f64 {
        self.session_buy_volume + self.session_sell_volume
    }

    pub fn change_from_open_pct(&self) -> Option<f64> {
        let open = self.session_open?;
        if open.abs() < 1e-12 { return None; }
        Some((self.last_price - open) / open * 100.0)
    }

    fn observe_trade(&mut self, time_ms: i64, price: f64, qty: f64, buyer_is_maker: bool) {
        if self.session_open.is_none() {
            self.session_open = Some(price);
            self.bar_start_ms = (time_ms / 60_000) * 60_000;
        }
        self.last_trade_ms = time_ms;
        self.last_price = price;
        if price > self.session_high { self.session_high = price; }
        if price < self.session_low { self.session_low = price; }
        match delta::aggressor_from_buyer_maker(buyer_is_maker) {
            delta::Side::Buy => self.session_buy_volume += qty,
            delta::Side::Sell => self.session_sell_volume += qty,
        }
        self.trade_count += 1;

        // Bar transition: append this bar's last price to sparkline.
        let new_bar_start = (time_ms / 60_000) * 60_000;
        if new_bar_start > self.bar_start_ms {
            self.sparkline.push_back(price);
            while self.sparkline.len() > SPARK_LEN {
                self.sparkline.pop_front();
            }
            self.bar_start_ms = new_bar_start;
        }
    }
}

#[derive(Debug)]
pub enum WatchEvent {
    Connected(String),
    Disconnected(String),
    Trade {
        symbol: String,
        time_ms: i64,
        price: f64,
        qty: f64,
        buyer_is_maker: bool,
    },
    BookTop {
        symbol: String,
        bid: Option<f64>,
        ask: Option<f64>,
    },
}

#[derive(Debug, Deserialize)]
struct Frame<'a> {
    stream: &'a str,
    data: &'a serde_json::value::RawValue,
}

#[derive(Debug, Deserialize)]
struct WireTrade {
    #[serde(rename = "T")]
    time_ms: i64,
    #[serde(rename = "p")]
    price: String,
    #[serde(rename = "q")]
    qty: String,
    #[serde(rename = "m")]
    buyer_is_maker: bool,
}

#[derive(Debug, Deserialize)]
struct WireBookTicker {
    #[serde(rename = "b")]
    bid: String,
    #[serde(rename = "a")]
    ask: String,
}

/// Spawn a watchlist feed for a symbol. Subscribes to `<sym>@trade` and
/// `<sym>@bookTicker` (top-of-book updates) on a single WebSocket. Sends
/// events to `tx`. Reconnects forever with backoff.
pub fn spawn_watchlist_feed(symbol: String, tx: mpsc::Sender<WatchEvent>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut backoff_ms: u64 = 500;
        loop {
            let started = std::time::Instant::now();
            match run_once(&symbol, &tx).await {
                Ok(_) => {}
                Err(_) => {}
            }
            let _ = tx.send(WatchEvent::Disconnected(symbol.clone())).await;
            if started.elapsed() > Duration::from_secs(30) {
                backoff_ms = 500;
            }
            sleep(Duration::from_millis(backoff_ms)).await;
            backoff_ms = (backoff_ms * 2).min(10_000);
        }
    })
}

async fn run_once(symbol: &str, tx: &mpsc::Sender<WatchEvent>) -> Result<()> {
    let lower = symbol.to_lowercase();
    let url = format!(
        "wss://stream.binance.com:9443/stream?streams={s}@trade/{s}@bookTicker",
        s = lower
    );
    let (ws, _) = connect_async(&url)
        .await
        .with_context(|| format!("connect_async {url}"))?;
    let (mut write, mut read) = ws.split();
    tx.send(WatchEvent::Connected(symbol.to_string())).await.ok();

    while let Some(msg) = read.next().await {
        let msg = msg.context("watchlist ws recv")?;
        match msg {
            Message::Text(text) => handle_frame(symbol, &text, tx).await?,
            Message::Binary(b) => {
                if let Ok(s) = std::str::from_utf8(&b) {
                    handle_frame(symbol, s, tx).await?;
                }
            }
            Message::Ping(p) => { write.send(Message::Pong(p)).await.ok(); }
            Message::Close(_) => return Ok(()),
            _ => {}
        }
    }
    Ok(())
}

async fn handle_frame(symbol: &str, text: &str, tx: &mpsc::Sender<WatchEvent>) -> Result<()> {
    let frame: Frame = match serde_json::from_str(text) {
        Ok(f) => f,
        Err(_) => return Ok(()),
    };
    if frame.stream.contains("@trade") {
        if let Ok(t) = serde_json::from_str::<WireTrade>(frame.data.get()) {
            let price: f64 = t.price.parse().unwrap_or(0.0);
            let qty: f64 = t.qty.parse().unwrap_or(0.0);
            tx.send(WatchEvent::Trade {
                symbol: symbol.to_string(),
                time_ms: t.time_ms,
                price,
                qty,
                buyer_is_maker: t.buyer_is_maker,
            }).await.ok();
        }
    } else if frame.stream.contains("@bookTicker") {
        if let Ok(b) = serde_json::from_str::<WireBookTicker>(frame.data.get()) {
            let bid: Option<f64> = b.bid.parse().ok();
            let ask: Option<f64> = b.ask.parse().ok();
            tx.send(WatchEvent::BookTop {
                symbol: symbol.to_string(),
                bid,
                ask,
            }).await.ok();
        }
    }
    Ok(())
}

/// Process a [`WatchEvent`] into the matching summary. Returns true if the
/// summary was found and updated.
pub fn apply_event(summaries: &mut [SymbolSummary], ev: &WatchEvent) -> bool {
    let target = match ev {
        WatchEvent::Connected(s) | WatchEvent::Disconnected(s) => s,
        WatchEvent::Trade { symbol, .. } | WatchEvent::BookTop { symbol, .. } => symbol,
    };
    let Some(sum) = summaries.iter_mut().find(|s| s.symbol.eq_ignore_ascii_case(target)) else {
        return false;
    };
    match ev {
        WatchEvent::Connected(_) => sum.connected = true,
        WatchEvent::Disconnected(_) => sum.connected = false,
        WatchEvent::Trade { time_ms, price, qty, buyer_is_maker, .. } => {
            sum.observe_trade(*time_ms, *price, *qty, *buyer_is_maker);
        }
        WatchEvent::BookTop { bid, ask, .. } => {
            sum.bid = *bid;
            sum.ask = *ask;
            sum.spread = match (*bid, *ask) {
                (Some(b), Some(a)) => Some(a - b),
                _ => None,
            };
        }
    }
    true
}

/// Pearson correlation between two equal-length samples; returns `None`
/// when the inputs are degenerate (all-zero variance).
pub fn correlation(a: &[f64], b: &[f64]) -> Option<f64> {
    if a.len() != b.len() || a.len() < 2 {
        return None;
    }
    let n = a.len() as f64;
    let mean_a = a.iter().copied().sum::<f64>() / n;
    let mean_b = b.iter().copied().sum::<f64>() / n;
    let mut num = 0.0;
    let mut den_a = 0.0;
    let mut den_b = 0.0;
    for i in 0..a.len() {
        let da = a[i] - mean_a;
        let db = b[i] - mean_b;
        num += da * db;
        den_a += da * da;
        den_b += db * db;
    }
    if den_a == 0.0 || den_b == 0.0 {
        return None;
    }
    Some(num / (den_a.sqrt() * den_b.sqrt()))
}

/// Lead-lag: returns the lag in samples (signed) at which `b`'s
/// cross-correlation with `a` is maximal in magnitude. Positive means
/// `b` lags `a` (i.e. `a` leads), negative means `b` leads.
pub fn lead_lag(a: &[f64], b: &[f64], max_lag: usize) -> Option<i32> {
    if a.len() != b.len() || a.len() < max_lag + 2 {
        return None;
    }
    let mut best_lag: i32 = 0;
    let mut best_abs = 0.0;
    for lag in -(max_lag as i32)..=(max_lag as i32) {
        let (sa, sb): (&[f64], &[f64]) = if lag >= 0 {
            let l = lag as usize;
            (&a[..a.len() - l], &b[l..])
        } else {
            let l = (-lag) as usize;
            (&a[l..], &b[..b.len() - l])
        };
        if let Some(c) = correlation(sa, sb) {
            let abs = c.abs();
            if abs > best_abs {
                best_abs = abs;
                best_lag = lag;
            }
        }
    }
    Some(best_lag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_trade_updates_session_state() {
        let mut s = SymbolSummary::new("BTCUSDT");
        s.observe_trade(0, 100.0, 1.0, false); // buy
        s.observe_trade(1000, 102.0, 2.0, true); // sell
        assert_eq!(s.last_price, 102.0);
        assert_eq!(s.session_open, Some(100.0));
        assert_eq!(s.session_high, 102.0);
        assert_eq!(s.session_low, 100.0);
        assert!((s.delta() - (1.0 - 2.0)).abs() < 1e-9);
        assert_eq!(s.trade_count, 2);
    }

    #[test]
    fn change_from_open_pct() {
        let mut s = SymbolSummary::new("BTCUSDT");
        s.observe_trade(0, 100.0, 1.0, false);
        s.observe_trade(1000, 110.0, 1.0, true);
        let pct = s.change_from_open_pct().unwrap();
        assert!((pct - 10.0).abs() < 1e-9);
    }

    #[test]
    fn sparkline_grows_per_minute() {
        let mut s = SymbolSummary::new("BTCUSDT");
        s.observe_trade(0, 100.0, 1.0, false);
        s.observe_trade(60_001, 101.0, 1.0, false); // new minute
        s.observe_trade(120_001, 102.0, 1.0, false);
        s.observe_trade(180_001, 103.0, 1.0, false);
        // Sparkline appended on each bar transition.
        assert!(s.sparkline.len() >= 3);
    }

    #[test]
    fn correlation_perfect_positive() {
        let a = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let b: Vec<f64> = a.iter().map(|x| x * 2.0 + 1.0).collect();
        let c = correlation(&a, &b).unwrap();
        assert!((c - 1.0).abs() < 1e-9);
    }

    #[test]
    fn correlation_perfect_negative() {
        let a = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let b: Vec<f64> = a.iter().map(|x| -x).collect();
        let c = correlation(&a, &b).unwrap();
        assert!((c + 1.0).abs() < 1e-9);
    }

    #[test]
    fn lead_lag_detects_b_lagging() {
        // b is a shifted by 1 sample → b lags by 1.
        let a: Vec<f64> = (0..30).map(|i| (i as f64).sin()).collect();
        let mut b = vec![0.0_f64; 30];
        for i in 1..30 { b[i] = a[i - 1]; }
        let lag = lead_lag(&a, &b, 5).unwrap();
        assert_eq!(lag, 1);
    }

    #[test]
    fn apply_event_routes_by_symbol() {
        let mut sums = vec![SymbolSummary::new("BTCUSDT"), SymbolSummary::new("ETHUSDT")];
        let ev = WatchEvent::Trade {
            symbol: "ETHUSDT".into(),
            time_ms: 0,
            price: 2000.0,
            qty: 1.0,
            buyer_is_maker: false,
        };
        assert!(apply_event(&mut sums, &ev));
        assert_eq!(sums[1].last_price, 2000.0);
        assert_eq!(sums[0].trade_count, 0);
    }
}
