//! Live smoke test against Binance public stream.
//!
//! Skipped by default (network-dependent). Run with:
//!   cargo test --test feed_smoke -- --ignored --nocapture

use std::time::Duration;

use tapeworm::app::AppState;
use tapeworm::feed::{self, FeedEvent};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep, timeout};

#[tokio::test]
#[ignore]
async fn live_feed_populates_book_and_tape() {
    let (tx, mut rx) = mpsc::channel::<FeedEvent>(2048);
    let handle = feed::spawn("BTCUSDT".to_string(), tx);
    let mut app = AppState::new("BTCUSDT");

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut got_snapshot = false;
    let mut got_diff = false;
    let mut got_trade = false;

    while Instant::now() < deadline {
        let remaining = deadline - Instant::now();
        match timeout(remaining, rx.recv()).await {
            Err(_) => break,
            Ok(None) => break,
            Ok(Some(ev)) => {
                match &ev {
                    FeedEvent::Snapshot(_) => got_snapshot = true,
                    FeedEvent::Diff(_) => got_diff = true,
                    FeedEvent::Trade(_) => got_trade = true,
                    _ => {}
                }
                app.handle(ev);
                if got_snapshot && got_diff && got_trade && app.tape.len() >= 5 {
                    break;
                }
            }
        }
    }

    handle.abort();
    sleep(Duration::from_millis(50)).await;

    assert!(got_snapshot, "expected a Snapshot event from the feed");
    assert!(got_diff, "expected at least one Diff event");
    assert!(got_trade, "expected at least one Trade event");
    assert!(app.has_snapshot, "app state should record snapshot");
    assert!(!app.book.is_stale(), "book must not be stale");
    assert!(app.book.best_bid().is_some(), "best bid present");
    assert!(app.book.best_ask().is_some(), "best ask present");
    let (bb, _) = app.book.best_bid().unwrap();
    let (ba, _) = app.book.best_ask().unwrap();
    assert!(bb < ba, "bid {bb} must be < ask {ba}");
    assert!(app.tape.len() >= 5, "tape grew, got {}", app.tape.len());
}
