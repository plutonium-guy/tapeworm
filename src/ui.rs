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

const TIME_FMT: &[FormatItem<'_>] = format_description!("[hour]:[minute]:[second]");

pub fn draw(frame: &mut Frame, app: &AppState) {
    let area = frame.area();
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
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
    draw_status(frame, outer[1], app);
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
        Span::raw("  [q] quit  [r] reset"),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn fmt_price(p: Decimal) -> String {
    p.round_dp(2).to_string()
}

fn fmt_qty(q: Decimal) -> String {
    q.round_dp(4).to_string()
}
