use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use rust_decimal::Decimal;
use time::OffsetDateTime;
use time::format_description::FormatItem;
use time::macros::format_description;

use crate::app::{AppState, ConnState};
use crate::delta::Side;
use crate::footprint::{FootprintBar, Imbalance};

const TIME_FMT: &[FormatItem<'_>] = format_description!("[hour]:[minute]:[second]");
const HHMM_FMT: &[FormatItem<'_>] = format_description!("[hour]:[minute]");

pub fn draw(frame: &mut Frame, app: &AppState) {
    let area = frame.area();
    let now_ms = current_unix_ms();

    let lower_active = (app.show_footprint || app.show_chart) && area.height > 18;
    let constraints: Vec<Constraint> = if lower_active {
        let lower_h = ((area.height as i32 - 1) / 2).clamp(12, 30) as u16;
        vec![
            Constraint::Min(8),
            Constraint::Length(lower_h),
            Constraint::Length(1),
        ]
    } else {
        vec![Constraint::Min(0), Constraint::Length(1)]
    };
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints.clone())
        .split(area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(35),
            Constraint::Percentage(35),
            Constraint::Percentage(30),
        ])
        .split(outer[0]);

    draw_dom(frame, cols[0], app);
    draw_tape(frame, cols[1], app);
    draw_delta(frame, cols[2], app);

    if constraints.len() == 3 {
        if app.show_chart {
            crate::chart::draw(frame, outer[1], app);
        } else {
            draw_footprint(frame, outer[1], app, now_ms);
        }
        draw_status(frame, outer[2], app);
    } else {
        draw_status(frame, outer[1], app);
    }
}

fn current_unix_ms() -> i64 {
    let now = OffsetDateTime::now_utc();
    now.unix_timestamp() * 1000 + (now.nanosecond() as i64) / 1_000_000
}

fn draw_dom(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("DOM — {}", app.symbol));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if !app.has_snapshot {
        let p = Paragraph::new("syncing order book…").style(Style::default().fg(Color::Yellow));
        frame.render_widget(p, inner);
        return;
    }
    if app.book.is_stale() {
        let p = Paragraph::new(Line::from(vec![
            Span::styled(
                "STALE — RESYNCING",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        frame.render_widget(p, inner);
        return;
    }

    let height = inner.height as usize;
    if height < 3 {
        return;
    }
    // 1 row reserved for spread separator.
    let per_side = (height - 1) / 2;

    let asks: Vec<(Decimal, Decimal)> = app.book.asks.iter_asc().take(per_side).collect();
    let bids: Vec<(Decimal, Decimal)> = app.book.bids.iter_desc().take(per_side).collect();

    let max_qty = asks
        .iter()
        .chain(bids.iter())
        .map(|(_, q)| *q)
        .max()
        .unwrap_or(Decimal::ONE);

    let bar_width = inner.width.saturating_sub(20).max(4) as usize;

    let mut lines: Vec<Line> = Vec::with_capacity(height);

    // Asks: render in reverse (highest at top, lowest just above spread).
    for (i, (price, qty)) in asks.iter().rev().enumerate() {
        let is_best = i == asks.len() - 1;
        lines.push(level_line(*price, *qty, max_qty, bar_width, Color::Red, is_best));
    }

    // Spread separator.
    let spread = app.book.spread().map(|s| s.normalize().to_string()).unwrap_or_else(|| "?".into());
    lines.push(Line::from(vec![Span::styled(
        format!("─── spread {spread} ───"),
        Style::default().fg(Color::DarkGray),
    )]));

    // Bids: highest first (just below spread) → descending downward.
    for (i, (price, qty)) in bids.iter().enumerate() {
        let is_best = i == 0;
        lines.push(level_line(*price, *qty, max_qty, bar_width, Color::Green, is_best));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn level_line(
    price: Decimal,
    qty: Decimal,
    max_qty: Decimal,
    bar_width: usize,
    color: Color,
    best: bool,
) -> Line<'static> {
    let bar_len = if max_qty.is_zero() {
        0
    } else {
        let frac: f64 = (qty / max_qty).try_into().unwrap_or(0.0);
        (frac * bar_width as f64).round().max(0.0) as usize
    };
    let bar: String = "█".repeat(bar_len.min(bar_width));
    let mut style = Style::default().fg(color);
    if best {
        style = style.add_modifier(Modifier::BOLD);
    }
    Line::from(vec![
        Span::styled(format!("{:>10} ", fmt_price(price)), style),
        Span::styled(format!("{:>8} ", fmt_qty(qty)), style),
        Span::styled(bar, style),
    ])
}

fn draw_tape(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = Block::default().borders(Borders::ALL).title("Tape");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let max_rows = inner.height as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(max_rows);

    for entry in app.tape.iter().take(max_rows) {
        let (color, marker) = match entry.trade.side {
            Side::Buy => (Color::Green, "▲"),
            Side::Sell => (Color::Red, "▼"),
        };
        let dt = OffsetDateTime::from_unix_timestamp(entry.trade.time_ms / 1000)
            .unwrap_or(OffsetDateTime::UNIX_EPOCH);
        let time = dt.format(TIME_FMT).unwrap_or_else(|_| String::from("--:--:--"));
        let price = fmt_price(entry.trade.price);
        let qty = fmt_qty(entry.trade.qty);
        let mut style = Style::default().fg(color);
        if entry.large {
            style = style.add_modifier(Modifier::BOLD | Modifier::REVERSED);
        }
        let large_mark = if entry.large { "★" } else { " " };
        lines.push(Line::from(vec![Span::styled(
            format!("{time} {marker} {price:>10} {qty:>9} {large_mark}"),
            style,
        )]));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_delta(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = Block::default().borders(Borders::ALL).title("Delta");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let buy = app.delta.buy_volume();
    let sell = app.delta.sell_volume();
    let net = app.delta.delta();
    let total = app.delta.total_volume();
    let count = app.delta.trade_count();

    let net_color = if net.is_sign_negative() {
        Color::Red
    } else if net.is_zero() {
        Color::Gray
    } else {
        Color::Green
    };

    let mut lines = vec![
        Line::from(vec![
            Span::raw("Buy:   "),
            Span::styled(fmt_qty(buy), Style::default().fg(Color::Green)),
        ]),
        Line::from(vec![
            Span::raw("Sell:  "),
            Span::styled(fmt_qty(sell), Style::default().fg(Color::Red)),
        ]),
        Line::from(vec![
            Span::raw("Total: "),
            Span::styled(fmt_qty(total), Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::raw("Δ:     "),
            Span::styled(
                fmt_qty(net),
                Style::default().fg(net_color).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![Span::raw(format!("Trades: {count}"))]),
        Line::from(""),
        Line::from("Buy / Sell ratio"),
        ratio_bar(app, inner.width.saturating_sub(2) as usize),
    ];

    if let (Some((bb, _)), Some((ba, _))) = (app.book.best_bid(), app.book.best_ask()) {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("Best bid: {}", fmt_price(bb))));
        lines.push(Line::from(format!("Best ask: {}", fmt_price(ba))));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn ratio_bar(app: &AppState, width: usize) -> Line<'static> {
    let ratio = app.delta.buy_ratio().unwrap_or(0.5);
    let buy_cells = (ratio * width as f64).round() as usize;
    let buy_cells = buy_cells.min(width);
    let sell_cells = width - buy_cells;
    Line::from(vec![
        Span::styled("█".repeat(buy_cells), Style::default().fg(Color::Green)),
        Span::styled("█".repeat(sell_cells), Style::default().fg(Color::Red)),
    ])
}

fn draw_status(frame: &mut Frame, area: Rect, app: &AppState) {
    let conn = match app.conn {
        ConnState::Connected => Span::styled("●LIVE", Style::default().fg(Color::Green)),
        ConnState::Disconnected => Span::styled("○OFF ", Style::default().fg(Color::Red)),
    };
    let spread = app
        .book
        .spread()
        .map(|s| s.normalize().to_string())
        .unwrap_or_else(|| "—".into());
    let stale = if app.book.is_stale() {
        Span::styled(" STALE ", Style::default().fg(Color::Black).bg(Color::Yellow))
    } else {
        Span::raw("")
    };
    let now_ms = current_unix_ms();
    let alert_flash = match app.alert_flash_until_ms {
        Some(t) if now_ms < t => Span::styled(
            " ALERT ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightRed)
                .add_modifier(Modifier::BOLD | Modifier::SLOW_BLINK),
        ),
        _ => Span::raw(""),
    };
    let line = Line::from(vec![
        Span::styled(
            "TAPEWORM ",
            Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan),
        ),
        Span::raw(format!("{} ", app.symbol)),
        conn,
        Span::raw(format!("  spread {spread}")),
        Span::raw(format!("  evt {}", app.event_count)),
        Span::raw("  "),
        stale,
        Span::raw(" "),
        alert_flash,
        Span::raw("  [q]quit [r]reset [f]fp [c]chart [1-5]tf [t]type [v]vol [i]ind [x]cross"),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn fmt_price(p: Decimal) -> String {
    p.round_dp(2).to_string()
}

fn fmt_qty(q: Decimal) -> String {
    q.round_dp(4).to_string()
}

const FP_BAR_WIDTH: u16 = 13;
const FP_PRICE_COL: u16 = 9;

fn draw_footprint(frame: &mut Frame, area: Rect, app: &AppState, now_ms: i64) {
    let block = Block::default().borders(Borders::ALL).title("Footprint");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width <= FP_PRICE_COL + FP_BAR_WIDTH || inner.height < 6 {
        let p = Paragraph::new("terminal too small for footprint").style(Style::default().fg(Color::DarkGray));
        frame.render_widget(p, inner);
        return;
    }

    let avail_for_bars = inner.width.saturating_sub(FP_PRICE_COL);
    let max_bars = (avail_for_bars / FP_BAR_WIDTH) as usize;
    if max_bars == 0 {
        return;
    }

    // Newest bars on the right; collect chronologically and keep the last `max_bars`.
    let mut visible: Vec<&FootprintBar> = app
        .footprint
        .completed()
        .iter()
        .chain(app.footprint.forming().into_iter())
        .collect();
    if visible.len() > max_bars {
        let drop = visible.len() - max_bars;
        visible.drain(..drop);
    }

    if visible.is_empty() {
        let p = Paragraph::new("waiting for first trade…").style(Style::default().fg(Color::DarkGray));
        frame.render_widget(p, inner);
        return;
    }

    // Header rows: time, vol, delta. Then body of price levels. Reserve 3 header rows.
    let header_rows: u16 = 3;
    if inner.height <= header_rows + 1 {
        return;
    }
    let body_rows = inner.height - header_rows;

    // Aligned price set across visible bars.
    let mut price_set = std::collections::BTreeSet::new();
    for &bar in &visible {
        for price in bar.levels.keys() {
            price_set.insert(*price);
        }
    }
    let mut prices: Vec<Decimal> = price_set.into_iter().collect();
    prices.sort_by(|a: &Decimal, b: &Decimal| b.cmp(a)); // descending

    // If too many prices, center the window around the latest forming bar's close.
    if prices.len() > body_rows as usize {
        let pivot = visible.last().unwrap().close;
        let pivot_tick = round_to_tick_loose(pivot, app.footprint.tick());
        let pivot_idx = prices
            .iter()
            .position(|p| *p <= pivot_tick)
            .unwrap_or(prices.len() / 2);
        let half = body_rows as usize / 2;
        let start = pivot_idx.saturating_sub(half);
        let end = (start + body_rows as usize).min(prices.len());
        let start = end.saturating_sub(body_rows as usize);
        prices = prices[start..end].to_vec();
    }

    // Forming bar = last visible if its end > now_ms.
    let forming_start = app
        .footprint
        .forming()
        .map(|b| b.start_ms);

    let mut lines: Vec<Line> = Vec::with_capacity(inner.height as usize);

    // Row 1: time / countdown
    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::raw(format!("{:>width$}", "", width = FP_PRICE_COL as usize)));
    for &bar in &visible {
        let label = if Some(bar.start_ms) == forming_start {
            let secs = ((bar.end_ms - now_ms).max(0)) / 1000;
            format!("T-{secs:>3}s")
        } else {
            let dt = OffsetDateTime::from_unix_timestamp(bar.end_ms / 1000)
                .unwrap_or(OffsetDateTime::UNIX_EPOCH);
            dt.format(HHMM_FMT).unwrap_or_else(|_| "--:--".into())
        };
        spans.push(bar_cell_span(&format!(" {:^11} ", label), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
    }
    lines.push(Line::from(spans));

    // Row 2: total volume
    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::raw(format!("{:>width$}", "vol", width = FP_PRICE_COL as usize)));
    for &bar in &visible {
        let v = fmt_qty_compact(bar.total_volume());
        spans.push(bar_cell_span(&format!(" {v:^11} "), Style::default().fg(Color::Gray)));
    }
    lines.push(Line::from(spans));

    // Row 3: delta
    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::raw(format!("{:>width$}", "Δ", width = FP_PRICE_COL as usize)));
    for &bar in &visible {
        let d = bar.delta();
        let color = if d.is_sign_negative() {
            Color::Red
        } else if d.is_zero() {
            Color::Gray
        } else {
            Color::Green
        };
        let s = fmt_qty_compact(d);
        spans.push(bar_cell_span(
            &format!(" {s:^11} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
    lines.push(Line::from(spans));

    // Body rows: one per visible price level.
    let last_close = visible.last().map(|b| b.close);
    let last_close_tick = last_close.map(|p| round_to_tick_loose(p, app.footprint.tick()));

    for price in prices {
        let mut spans: Vec<Span> = Vec::new();
        let price_str = fmt_price(price);
        let mut price_style = Style::default().fg(Color::White);
        if Some(price) == last_close_tick {
            price_style = price_style.add_modifier(Modifier::BOLD).fg(Color::Yellow);
        }
        spans.push(Span::styled(
            format!("{price_str:>width$}", width = FP_PRICE_COL as usize),
            price_style,
        ));
        for &bar in &visible {
            let cell = bar.levels.get(&price).copied().unwrap_or_default();
            let is_poc = bar.point_of_control() == Some(price);
            spans.push(footprint_cell_span(cell, is_poc));
        }
        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn footprint_cell_span(cell: crate::footprint::Cell, is_poc: bool) -> Span<'static> {
    let imb = cell.imbalance();
    let net = cell.buy.cmp(&cell.sell);

    let base_color = if cell.total().is_zero() {
        Color::DarkGray
    } else {
        match imb {
            Imbalance::Buy => Color::Green,
            Imbalance::Sell => Color::Red,
            Imbalance::Balanced => match net {
                std::cmp::Ordering::Greater => Color::LightGreen,
                std::cmp::Ordering::Less => Color::LightRed,
                std::cmp::Ordering::Equal => Color::Gray,
            },
        }
    };

    let mut style = Style::default().fg(base_color);
    if matches!(imb, Imbalance::Buy | Imbalance::Sell) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if is_poc {
        style = style.add_modifier(Modifier::REVERSED);
    }

    let text = if cell.total().is_zero() {
        format!(" {:>4}|{:<4} ", "·", "·")
    } else {
        format!(" {:>4}|{:<4} ", fmt_cell_qty(cell.sell), fmt_cell_qty(cell.buy))
    };
    Span::styled(text, style)
}

fn bar_cell_span(text: &str, style: Style) -> Span<'static> {
    Span::styled(text.to_string(), style)
}

/// Compact qty for headers: e.g. "12.34" or "1.2k".
fn fmt_qty_compact(q: Decimal) -> String {
    let neg = q.is_sign_negative();
    let abs = if neg { -q } else { q };
    let f: f64 = abs.try_into().unwrap_or(0.0);
    let s = if f >= 1000.0 {
        format!("{:.1}k", f / 1000.0)
    } else if f >= 100.0 {
        format!("{f:.1}")
    } else {
        format!("{f:.2}")
    };
    if neg { format!("-{s}") } else { s }
}

fn fmt_cell_qty(q: Decimal) -> String {
    let f: f64 = q.try_into().unwrap_or(0.0);
    if f >= 1000.0 {
        format!("{:.1}k", f / 1000.0)
    } else if f >= 100.0 {
        format!("{f:.0}")
    } else if f >= 10.0 {
        format!("{f:.1}")
    } else {
        format!("{f:.2}")
    }
}

fn round_to_tick_loose(price: Decimal, tick: Decimal) -> Decimal {
    if tick.is_zero() {
        return price;
    }
    let n = price / tick;
    n.round() * tick
}
