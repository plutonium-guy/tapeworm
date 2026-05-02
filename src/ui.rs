use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use rust_decimal::Decimal;
use time::OffsetDateTime;
use time::format_description::FormatItem;
use time::macros::format_description;

use std::cell::RefCell;

use crate::app::{AppState, ClickAction, Hotspot};
use crate::engines::ConnState;
use crate::delta::Side;
use crate::footprint::{FootprintBar, Imbalance};
use crate::paper::OrderStatus;

thread_local! {
    static HITS: RefCell<Vec<Hotspot>> = RefCell::new(Vec::new());
}

pub fn take_hits() -> Vec<Hotspot> {
    HITS.with(|h| std::mem::take(&mut *h.borrow_mut()))
}

pub(crate) fn hit(area: Rect, action: ClickAction) {
    HITS.with(|h| {
        h.borrow_mut().push(Hotspot {
            x: area.x,
            y: area.y,
            w: area.width,
            h: area.height,
            action,
        });
    });
}

const TIME_FMT: &[FormatItem<'_>] = format_description!("[hour]:[minute]:[second]");
const HHMM_FMT: &[FormatItem<'_>] = format_description!("[hour]:[minute]");

pub fn draw(frame: &mut Frame, app: &AppState) {
    let area = frame.area();
    let now_ms = current_unix_ms();

    let lower_active = (app.show_footprint || app.show_chart || app.show_analytics || app.show_watchlist || app.show_graphs) && area.height > 18;
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

    let cols = if app.show_paper {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(28),
                Constraint::Percentage(30),
                Constraint::Percentage(22),
                Constraint::Percentage(20),
            ])
            .split(outer[0])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(35),
                Constraint::Percentage(35),
                Constraint::Percentage(30),
            ])
            .split(outer[0])
    };

    draw_dom(frame, cols[0], app);
    draw_tape(frame, cols[1], app);
    draw_delta(frame, cols[2], app);
    if app.show_paper && cols.len() == 4 {
        draw_paper(frame, cols[3], app);
    }

    if constraints.len() == 3 {
        if app.show_signals_log {
            let split = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(30), Constraint::Length(40)])
                .split(outer[1]);
            if app.show_graphs {
                crate::graphs::draw_graphs_panel(frame, split[0], app);
            } else if app.show_watchlist {
                draw_watchlist(frame, split[0], app);
            } else if app.show_analytics {
                draw_analytics(frame, split[0], app);
            } else if app.show_chart {
                crate::chart::draw(frame, split[0], app);
            } else {
                draw_footprint(frame, split[0], app, now_ms);
            }
            draw_signals_log(frame, split[1], app);
        } else if app.show_graphs {
            crate::graphs::draw_graphs_panel(frame, outer[1], app);
        } else if app.show_watchlist {
            draw_watchlist(frame, outer[1], app);
        } else if app.show_analytics {
            draw_analytics(frame, outer[1], app);
        } else if app.show_chart {
            crate::chart::draw(frame, outer[1], app);
        } else {
            draw_footprint(frame, outer[1], app, now_ms);
        }
        draw_status(frame, outer[2], app);
    } else if app.show_signals_log {
        // Signals log alone in lower area when no other lower panel.
        let lower_h: u16 = (area.height / 3).max(8);
        if area.height > lower_h + 5 {
            let outer2 = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(6),
                    Constraint::Length(lower_h),
                    Constraint::Length(1),
                ])
                .split(area);
            // Re-render top columns (with paper if visible).
            let cols = if app.show_paper {
                Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(28),
                        Constraint::Percentage(30),
                        Constraint::Percentage(22),
                        Constraint::Percentage(20),
                    ])
                    .split(outer2[0])
            } else {
                Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(35),
                        Constraint::Percentage(35),
                        Constraint::Percentage(30),
                    ])
                    .split(outer2[0])
            };
            draw_dom(frame, cols[0], app);
            draw_tape(frame, cols[1], app);
            draw_delta(frame, cols[2], app);
            if app.show_paper && cols.len() == 4 {
                draw_paper(frame, cols[3], app);
            }
            draw_signals_log(frame, outer2[1], app);
            draw_status(frame, outer2[2], app);
            return;
        }
        draw_status(frame, outer[1], app);
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
        .title(format!("DOM — {}", app.active.symbol));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if !app.active.has_snapshot {
        let p = Paragraph::new("syncing order book…").style(Style::default().fg(Color::Yellow));
        frame.render_widget(p, inner);
        return;
    }
    if app.active.book.is_stale() {
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

    let asks: Vec<(Decimal, Decimal)> = app.active.book.asks.iter_asc().take(per_side).collect();
    let bids: Vec<(Decimal, Decimal)> = app.active.book.bids.iter_desc().take(per_side).collect();

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
    let spread = app.active.book.spread().map(|s| s.normalize().to_string()).unwrap_or_else(|| "?".into());
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

    for entry in app.active.tape.iter().take(max_rows) {
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

    let buy = app.active.delta.buy_volume();
    let sell = app.active.delta.sell_volume();
    let net = app.active.delta.delta();
    let total = app.active.delta.total_volume();
    let count = app.active.delta.trade_count();

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

    if let (Some((bb, _)), Some((ba, _))) = (app.active.book.best_bid(), app.active.book.best_ask()) {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("Best bid: {}", fmt_price(bb))));
        lines.push(Line::from(format!("Best ask: {}", fmt_price(ba))));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn ratio_bar(app: &AppState, width: usize) -> Line<'static> {
    let ratio = app.active.delta.buy_ratio().unwrap_or(0.5);
    let buy_cells = (ratio * width as f64).round() as usize;
    let buy_cells = buy_cells.min(width);
    let sell_cells = width - buy_cells;
    Line::from(vec![
        Span::styled("█".repeat(buy_cells), Style::default().fg(Color::Green)),
        Span::styled("█".repeat(sell_cells), Style::default().fg(Color::Red)),
    ])
}

fn draw_status(frame: &mut Frame, area: Rect, app: &AppState) {
    let conn = match app.active.conn {
        ConnState::Connected => Span::styled("●LIVE", Style::default().fg(Color::Green)),
        ConnState::Disconnected => Span::styled("○OFF ", Style::default().fg(Color::Red)),
    };
    let spread = app
        .active
        .book
        .spread()
        .map(|s| s.normalize().to_string())
        .unwrap_or_else(|| "—".into());
    let stale = if app.active.book.is_stale() {
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
    let prefix = vec![
        Span::styled(
            "TAPEWORM ",
            Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan),
        ),
        Span::raw(format!("{} ", app.active.symbol)),
        conn,
        Span::raw(format!("  spread {spread}")),
        Span::raw(format!("  evt {}", app.active.event_count)),
        Span::raw("  "),
        stale,
        Span::raw(" "),
        alert_flash,
        Span::raw("  "),
    ];
    let prefix_w: u16 = prefix
        .iter()
        .map(|s| s.content.chars().count() as u16)
        .sum();
    // Build clickable [key]label chips and register hotspots.
    let chips: [(&str, ClickAction); 9] = [
        ("[q]quit", ClickAction::Quit),
        ("[r]reset", ClickAction::Reset),
        ("[f]fp", ClickAction::ToggleFootprint),
        ("[c]chart", ClickAction::ToggleChart),
        ("[G]graphs", ClickAction::ToggleGraphs),
        ("[w]watch", ClickAction::ToggleWatchlist),
        ("[s]sig", ClickAction::ToggleSignalsLog),
        ("[p]pap", ClickAction::TogglePaper),
        ("[z]anly", ClickAction::ToggleAnalytics),
    ];
    let mut spans: Vec<Span> = prefix;
    let mut col = area.x + prefix_w.min(area.width.saturating_sub(1));
    for (label, action) in chips {
        let w = label.chars().count() as u16;
        let rect = Rect { x: col, y: area.y, width: w, height: 1 };
        hit(rect, action.clone());
        spans.push(Span::styled(label.to_string(), Style::default().fg(Color::Cyan)));
        spans.push(Span::raw(" "));
        col = col.saturating_add(w + 1);
        if col >= area.x + area.width {
            break;
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_watchlist(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = Block::default().borders(Borders::ALL).title("Watchlist");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 2 || inner.width < 30 { return; }

    let mut lines: Vec<Line> = Vec::with_capacity(app.watchlist.len() + 1);
    lines.push(Line::from(Span::styled(
        " sym       last     %chg     Δ        vol      spark",
        Style::default().fg(Color::DarkGray),
    )));
    for (i, s) in app.watchlist.iter().enumerate() {
        let chg = s.change_from_open_pct().unwrap_or(0.0);
        let chg_color = if chg < 0.0 { Color::Red } else if chg > 0.0 { Color::Green } else { Color::Gray };
        let delta_color = if s.delta() < 0.0 { Color::Red } else if s.delta() > 0.0 { Color::Green } else { Color::Gray };
        let conn_marker = if s.connected { "●" } else { "○" };
        let conn_color = if s.connected { Color::Green } else { Color::Red };
        let spark = render_sparkline(&s.sparkline, 24);
        let row_y = inner.y + 1 + i as u16; // +1 for header row
        if row_y < inner.y + inner.height {
            hit(
                Rect { x: inner.x, y: row_y, width: inner.width, height: 1 },
                ClickAction::SwitchActive(s.symbol.clone()),
            );
        }
        let line = Line::from(vec![
            Span::styled(conn_marker.to_string(), Style::default().fg(conn_color)),
            Span::raw(" "),
            Span::styled(format!("{:<8}", s.symbol), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(" {:>9.2}", s.last_price)),
            Span::styled(format!(" {:>+6.2}%", chg), Style::default().fg(chg_color)),
            Span::styled(format!(" {:>+10.4}", s.delta()), Style::default().fg(delta_color)),
            Span::raw(format!(" {:>9.2}", s.total_volume())),
            Span::raw(" "),
            Span::styled(spark, Style::default().fg(chg_color)),
        ]);
        lines.push(line);
    }
    if app.watchlist.is_empty() {
        lines.push(Line::from(Span::styled("(no symbols watched)", Style::default().fg(Color::DarkGray))));
    }

    // Correlation matrix (pairwise Pearson over sparkline samples).
    if app.watchlist.len() >= 2 {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Correlation (last 30 1-min closes)",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )));
        let mut header = String::from("        ");
        for s in &app.watchlist {
            header.push_str(&format!("{:>8}", trim_sym(&s.symbol)));
        }
        lines.push(Line::from(Span::styled(header, Style::default().fg(Color::DarkGray))));
        for (i, a) in app.watchlist.iter().enumerate() {
            let mut spans: Vec<Span> = Vec::new();
            spans.push(Span::styled(
                format!("{:<8}", trim_sym(&a.symbol)),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            for (j, b) in app.watchlist.iter().enumerate() {
                if i == j {
                    spans.push(Span::styled("    1.00".to_string(), Style::default().fg(Color::DarkGray)));
                    continue;
                }
                let av: Vec<f64> = a.sparkline.iter().copied().collect();
                let bv: Vec<f64> = b.sparkline.iter().copied().collect();
                let n = av.len().min(bv.len());
                if n < 4 {
                    spans.push(Span::raw("       —"));
                    continue;
                }
                let av = &av[av.len() - n..];
                let bv = &bv[bv.len() - n..];
                let c = crate::multi::correlation(av, bv).unwrap_or(0.0);
                let color = if c > 0.6 {
                    Color::Green
                } else if c < -0.6 {
                    Color::Red
                } else {
                    Color::Gray
                };
                spans.push(Span::styled(
                    format!("  {:>+5.2}", c),
                    Style::default().fg(color),
                ));
            }
            lines.push(Line::from(spans));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn trim_sym(s: &str) -> String {
    let s = s.to_uppercase();
    if s.ends_with("USDT") {
        s[..s.len() - 4].to_string()
    } else {
        s
    }
}

fn render_sparkline(data: &std::collections::VecDeque<f64>, width: usize) -> String {
    if data.is_empty() {
        return " ".repeat(width);
    }
    let chars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let lo = data.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = data.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = (hi - lo).max(1e-9);
    let mut out = String::with_capacity(width);
    let take = data.iter().rev().take(width).rev();
    for v in take {
        let frac = ((v - lo) / span).clamp(0.0, 1.0);
        let idx = (frac * (chars.len() as f64 - 1.0)).round() as usize;
        out.push(chars[idx.min(chars.len() - 1)]);
    }
    out
}

fn draw_analytics(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = Block::default().borders(Borders::ALL).title("Analytics");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 4 || inner.width < 30 { return; }

    let a = &app.analytics;
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![Span::styled(
        format!("Trades: {}    Winners: {}    Losers: {}", a.count(), a.winners(), a.losers()),
        Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan),
    )]));
    let pnl = a.total_pnl();
    let pnl_color = if pnl.is_sign_negative() { Color::Red } else if pnl.is_zero() { Color::Gray } else { Color::Green };
    lines.push(Line::from(vec![
        Span::raw("Total PnL:    "),
        Span::styled(format!("{:>+10.2}", pnl), Style::default().fg(pnl_color).add_modifier(Modifier::BOLD)),
    ]));
    if let Some(wr) = a.win_rate() {
        lines.push(Line::from(format!("Win rate:     {:>10.1}%", wr * 100.0)));
    }
    if let Some(pf) = a.profit_factor() {
        lines.push(Line::from(format!("Profit factor:{:>10.2}", pf)));
    }
    if let Some(exp) = a.expectancy() {
        lines.push(Line::from(format!("Expectancy:   {:>+10.4}", exp)));
    }
    if let Some(w) = a.avg_winner() {
        lines.push(Line::from(format!("Avg winner:   {:>+10.2}", w)));
    }
    if let Some(l) = a.avg_loser() {
        lines.push(Line::from(format!("Avg loser:    {:>+10.2}", l)));
    }
    lines.push(Line::from(format!("Max drawdown: {:>+10.2}", a.max_drawdown())));
    lines.push(Line::from(format!("Best streak:  {} W / {} L", a.longest_streak(true), a.longest_streak(false))));
    lines.push(Line::from(""));

    // PnL spark.
    let cum: Vec<f64> = a
        .cumulative_pnl()
        .into_iter()
        .filter_map(|d| d.try_into().ok())
        .collect();
    if cum.len() >= 2 {
        let w = inner.width.saturating_sub(2) as usize;
        let n = cum.len().min(w);
        let lo = cum.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = cum.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let span = (hi - lo).max(1e-9);
        let h = inner.height.saturating_sub(lines.len() as u16 + 1).max(3) as usize;
        let mut grid: Vec<Vec<(char, Style)>> = (0..h).map(|_| (0..n).map(|_| (' ', Style::default())).collect()).collect();
        for (col, v) in cum.iter().rev().take(n).enumerate() {
            let r = ((1.0 - (v - lo) / span) * (h as f64 - 1.0)).round().clamp(0.0, (h - 1) as f64) as usize;
            let color = if *v >= 0.0 { Color::Green } else { Color::Red };
            grid[r][n - 1 - col] = ('•', Style::default().fg(color));
        }
        for row in grid {
            lines.push(Line::from(row.into_iter().map(|(c, s)| Span::styled(c.to_string(), s)).collect::<Vec<_>>()));
        }
    } else {
        lines.push(Line::from(Span::styled("not enough trades for cumulative PnL chart", Style::default().fg(Color::DarkGray))));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_paper(frame: &mut Frame, area: Rect, app: &AppState) {
    let title = if app.paper.trading_mode { "Paper [LIVE]" } else { "Paper [LOCKED]" };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 { return; }

    let pos = &app.paper.position;
    let unr = app.paper.unrealized_pnl();
    let realized = pos.realized_pnl;
    let bal = app.paper.balance();
    let buf = app.paper.daily_loss_buffer();
    let pos_color = if pos.qty.is_zero() {
        Color::Gray
    } else if pos.qty.is_sign_positive() {
        Color::Green
    } else {
        Color::Red
    };

    let working = app
        .paper
        .orders
        .iter()
        .filter(|o| matches!(o.status, OrderStatus::Working | OrderStatus::PartialFill))
        .count();

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::raw("Pos: "),
        Span::styled(
            format!("{:>+10.4}", pos.qty),
            Style::default().fg(pos_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" @ {}", fmt_price(pos.avg_price))),
    ]));
    let unr_color = if unr.is_sign_negative() { Color::Red } else if unr.is_zero() { Color::Gray } else { Color::Green };
    lines.push(Line::from(vec![
        Span::raw("uPnL: "),
        Span::styled(
            format!("{:>+10.2}", unr),
            Style::default().fg(unr_color).add_modifier(Modifier::BOLD),
        ),
    ]));
    let real_color = if realized.is_sign_negative() { Color::Red } else if realized.is_zero() { Color::Gray } else { Color::Green };
    lines.push(Line::from(vec![
        Span::raw("rPnL: "),
        Span::styled(
            format!("{:>+10.2}", realized),
            Style::default().fg(real_color),
        ),
    ]));
    lines.push(Line::from(format!("Bal:  {:>10.2}", bal)));
    let buf_color = if buf <= rust_decimal::Decimal::ZERO {
        Color::Red
    } else {
        Color::Yellow
    };
    lines.push(Line::from(vec![
        Span::raw("Buf:  "),
        Span::styled(format!("{:>10.2}", buf), Style::default().fg(buf_color)),
    ]));
    lines.push(Line::from(format!("Slip: {:>10.4}", app.paper.session_slippage)));
    lines.push(Line::from(format!("Qty:  {:>10}", app.paper_qty)));
    lines.push(Line::from(format!("Wkg:  {working}")));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "[b]buy [B]sell [F]flat [X]cancel [+/-]qty [T]trade-mode",
        Style::default().fg(Color::DarkGray),
    )));

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_signals_log(frame: &mut Frame, area: Rect, app: &AppState) {
    use crate::signals::SignalKind;
    let filter_label = match app.signals_filter {
        None => "all".to_string(),
        Some(k) => k.label().to_string(),
    };
    let title = format!("Signals [{filter_label}]");
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let max_rows = inner.height as usize;

    // Header row.
    let mut lines: Vec<Line> = Vec::with_capacity(max_rows);
    lines.push(Line::from(Span::styled(
        format!(" time  type  {:>10}  s  note", "price"),
        Style::default().fg(Color::DarkGray),
    )));

    let pace_tps = app.active.signals.pace.current_tps();
    let pace_avg = app
        .active
        .signals
        .pace
        .session_avg_tps(app.active.event_count as i64 + 1)
        .unwrap_or(0.0);
    lines.push(Line::from(Span::styled(
        format!(
            " pace cur={:.2}/s  avg={:.2}/s  buy={:.2}  sell={:.2}",
            pace_tps,
            pace_avg,
            app.active.signals.pace.buy_tps(),
            app.active.signals.pace.sell_tps(),
        ),
        Style::default().fg(Color::Cyan),
    )));
    lines.push(Line::from(""));

    let filtered: Vec<&crate::signals::Signal> = app
        .active
        .signals
        .log
        .iter()
        .filter(|s| match app.signals_filter {
            None => true,
            Some(k) => s.kind == k,
        })
        .take(max_rows.saturating_sub(3))
        .collect();

    for (idx, sig) in filtered.iter().enumerate() {
        // Hotspot for the row (account for the 3 header lines).
        let row_y = inner.y + 3 + idx as u16;
        if row_y < inner.y + inner.height {
            hit(
                Rect { x: inner.x, y: row_y, width: inner.width, height: 1 },
                ClickAction::SelectSignalAt(idx),
            );
        }
        let _ = sig;
        let sig = filtered[idx];
        let dt = OffsetDateTime::from_unix_timestamp(sig.time_ms / 1000)
            .unwrap_or(OffsetDateTime::UNIX_EPOCH);
        let time = dt.format(TIME_FMT).unwrap_or_else(|_| "--:--:--".into());
        let kind_color = match sig.kind {
            SignalKind::Iceberg => Color::LightCyan,
            SignalKind::Absorption => Color::LightMagenta,
            SignalKind::PaceSpike => Color::LightYellow,
            SignalKind::StopRun => Color::LightBlue,
            SignalKind::Exhaustion => Color::LightRed,
        };
        let stars: String = "★".repeat(sig.score as usize);
        let selected = app.show_signals_log && idx == app.signals_sel_idx;
        let prefix = if selected { "▶" } else { " " };
        let mut spans = vec![
            Span::styled(
                prefix.to_string(),
                Style::default().fg(if selected { Color::Yellow } else { Color::DarkGray }),
            ),
            Span::raw(time.clone()),
            Span::raw(" "),
            Span::styled(
                format!("{:<3}", sig.kind.label()),
                Style::default().fg(kind_color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::raw(format!("{:>10}", fmt_price(sig.price))),
            Span::raw(" "),
            Span::styled(stars, Style::default().fg(Color::Yellow)),
            Span::raw(" "),
            Span::styled(sig.note.clone(), Style::default().fg(Color::Gray)),
        ];
        if selected {
            for span in &mut spans {
                span.style = span.style.add_modifier(Modifier::REVERSED);
            }
        }
        lines.push(Line::from(spans));
    }
    if app.active.signals.log.is_empty() {
        lines.push(Line::from(Span::styled(
            "no signals yet",
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
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
        .active
        .footprint
        .completed()
        .iter()
        .chain(app.active.footprint.forming().into_iter())
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
        let pivot_tick = round_to_tick_loose(pivot, app.active.footprint.tick());
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
        .active
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
    let last_close_tick = last_close.map(|p| round_to_tick_loose(p, app.active.footprint.tick()));

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
