use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::delta;
use crate::orderbook::{DepthSnapshot, DiffDepthEvent};

/// One classified trade.
#[derive(Debug, Clone)]
pub struct Trade {
    pub time_ms: i64,
    pub price: Decimal,
    pub qty: Decimal,
    pub side: delta::Side,
}

/// Events emitted by the feed task to the app.
#[derive(Debug)]
pub enum FeedEvent {
    Connected,
    Disconnected,
    /// REST snapshot — apply, then expect Diffs to follow.
    Snapshot(DepthSnapshot),
    /// Diff update — apply on top of book.
    Diff(DiffDepthEvent),
    Trade(Trade),
}

/// Combined stream wrapper: `{ "stream": "...", "data": {...} }`.
#[derive(Debug, Deserialize)]
struct CombinedFrame<'a> {
    stream: &'a str,
    data: &'a serde_json::value::RawValue,
}

#[derive(Debug, Deserialize)]
struct RawTrade {
    #[serde(rename = "T")]
    time_ms: i64,
    #[serde(rename = "p")]
    price: String,
    #[serde(rename = "q")]
    qty: String,
    #[serde(rename = "m")]
    buyer_is_maker: bool,
}

/// Spawn the feed loop. Sends events to `tx`. Reconnects forever with backoff.
pub fn spawn(symbol: String, tx: mpsc::Sender<FeedEvent>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut backoff_ms: u64 = 500;
        loop {
            let started = std::time::Instant::now();
            match run_once(&symbol, &tx).await {
                Ok(_) => {
                    warn!("feed loop returned cleanly; reconnecting");
                }
                Err(e) => {
                    warn!(error = ?e, "feed error; reconnecting");
                }
            }
            let _ = tx.send(FeedEvent::Disconnected).await;
            // Reset backoff if we had a long, healthy session.
            if started.elapsed() > Duration::from_secs(30) {
                backoff_ms = 500;
            }
            sleep(Duration::from_millis(backoff_ms)).await;
            backoff_ms = (backoff_ms * 2).min(10_000);
        }
    })
}

async fn run_once(symbol: &str, tx: &mpsc::Sender<FeedEvent>) -> Result<()> {
    let lower = symbol.to_lowercase();
    let url = format!(
        "wss://stream.binance.com:9443/stream?streams={s}@depth@100ms/{s}@trade",
        s = lower
    );
    info!(url = %url, "connecting to Binance WS");

    let (ws_stream, _resp) = connect_async(&url)
        .await
        .with_context(|| format!("connect_async {url}"))?;

    let (mut write, mut read) = ws_stream.split();
    tx.send(FeedEvent::Connected).await.ok();

    // Buffer diff events while fetching the REST snapshot.
    let mut buffered: Vec<DiffDepthEvent> = Vec::new();
    let mut snapshot_loaded = false;
    // Last `u` we forwarded — used to validate `U == last_u + 1` on the next.
    let mut last_emitted_u: Option<u64> = None;

    let snapshot_fut = fetch_snapshot(symbol);
    tokio::pin!(snapshot_fut);

    loop {
        tokio::select! {
            biased;

            snap_result = &mut snapshot_fut, if !snapshot_loaded => {
                let snap = snap_result?;
                let last = snap.last_update_id;
                debug!(last_update_id = last, buffered = buffered.len(), "snapshot fetched");

                // Find first buffered diff that covers last+1.
                let drain = std::mem::take(&mut buffered);
                let mut to_emit: Vec<DiffDepthEvent> = Vec::with_capacity(drain.len());
                let mut found_anchor = false;
                for ev in drain {
                    if ev.final_update_id <= last { continue; }
                    if !found_anchor {
                        if ev.first_update_id <= last + 1 && last + 1 <= ev.final_update_id {
                            found_anchor = true;
                            last_emitted_u = Some(ev.final_update_id);
                            to_emit.push(ev);
                        } else if ev.first_update_id > last + 1 {
                            // Sync hole — this snapshot is too old, restart cleanly.
                            return Err(anyhow!("buffered diffs jumped past snapshot; resync"));
                        }
                        // else: skip stale diff
                    } else {
                        last_emitted_u = Some(ev.final_update_id);
                        to_emit.push(ev);
                    }
                }
                tx.send(FeedEvent::Snapshot(snap)).await.ok();
                for ev in to_emit {
                    tx.send(FeedEvent::Diff(ev)).await.ok();
                }
                snapshot_loaded = true;
                if last_emitted_u.is_none() {
                    last_emitted_u = Some(last); // next live event must be U == last+1
                }
            }

            msg = read.next() => {
                let Some(msg) = msg else { return Ok(()) };
                let msg = msg.context("ws recv")?;
                match msg {
                    Message::Text(text) => handle_text(&text, tx, &mut buffered, snapshot_loaded, &mut last_emitted_u).await?,
                    Message::Binary(b) => {
                        if let Ok(s) = std::str::from_utf8(&b) {
                            handle_text(s, tx, &mut buffered, snapshot_loaded, &mut last_emitted_u).await?;
                        }
                    }
                    Message::Ping(p) => { write.send(Message::Pong(p)).await.ok(); }
                    Message::Close(_) => return Ok(()),
                    _ => {}
                }
            }
        }
    }
}

async fn handle_text(
    text: &str,
    tx: &mpsc::Sender<FeedEvent>,
    buffered: &mut Vec<DiffDepthEvent>,
    snapshot_loaded: bool,
    last_emitted_u: &mut Option<u64>,
) -> Result<()> {
    let frame: CombinedFrame = serde_json::from_str(text).context("parse combined frame")?;
    if frame.stream.ends_with("@trade") {
        let raw: RawTrade = serde_json::from_str(frame.data.get())?;
        let price: Decimal = raw.price.parse().context("trade price decimal")?;
        let qty: Decimal = raw.qty.parse().context("trade qty decimal")?;
        let side = delta::aggressor_from_buyer_maker(raw.buyer_is_maker);
        tx.send(FeedEvent::Trade(Trade {
            time_ms: raw.time_ms,
            price,
            qty,
            side,
        }))
        .await
        .ok();
    } else if frame.stream.contains("@depth") {
        let ev: DiffDepthEvent = serde_json::from_str(frame.data.get())?;
        if !snapshot_loaded {
            buffered.push(ev);
        } else if let Some(prev_u) = *last_emitted_u {
            if ev.final_update_id <= prev_u {
                // already covered, ignore
            } else if ev.first_update_id == prev_u + 1
                || (ev.first_update_id <= prev_u + 1 && prev_u + 1 <= ev.final_update_id)
            {
                *last_emitted_u = Some(ev.final_update_id);
                tx.send(FeedEvent::Diff(ev)).await.ok();
            } else {
                return Err(anyhow!(
                    "depth sequence gap: prev_u={prev_u} got U={} u={}",
                    ev.first_update_id,
                    ev.final_update_id
                ));
            }
        }
    }
    Ok(())
}

async fn fetch_snapshot(symbol: &str) -> Result<DepthSnapshot> {
    let url = format!(
        "https://api.binance.com/api/v3/depth?symbol={}&limit=1000",
        symbol.to_uppercase()
    );
    let snap = reqwest::get(&url)
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()?
        .json::<DepthSnapshot>()
        .await
        .context("snapshot json")?;
    Ok(snap)
}
