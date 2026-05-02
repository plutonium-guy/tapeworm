//! Reusable terminal-graph primitives plus the comprehensive "Graphs"
//! panel that renders a 2×3 grid of mini-charts summarising every
//! data source the application maintains.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::AppState;

const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Vertical bar histogram. Negative values fall below the zero line and
/// are coloured `neg_color`; non-negative values fill upward in `pos_color`.
pub fn render_bar_histogram(values: &[f64], height: usize, pos_color: Color, neg_color: Color) -> Vec<Line<'static>> {
    if values.is_empty() || height < 2 {
        return vec![Line::from(Span::styled(
            "(no data)".to_string(),
            Style::default().fg(Color::DarkGray),
        ))];
    }
    let max_pos = values.iter().copied().fold(0.0_f64, f64::max).max(1e-9);
    let min_neg = values.iter().copied().fold(0.0_f64, f64::min).min(-1e-9);
    let abs_max = max_pos.max(-min_neg);
    let zero_row = (height / 2).max(1);
    let upper = zero_row;
    let lower = height - zero_row;
    let n = values.len();
    let mut grid: Vec<Vec<(char, Style)>> =
        (0..height).map(|_| (0..n).map(|_| (' ', Style::default())).collect()).collect();
    for (col, v) in values.iter().enumerate() {
        if v.is_nan() { continue; }
        let frac = (v.abs() / abs_max).clamp(0.0, 1.0);
        if *v >= 0.0 {
            let cells = (frac * upper as f64).round() as usize;
            for r in 0..cells {
                let row = zero_row.saturating_sub(1).saturating_sub(r);
                if row < height {
                    grid[row][col] = ('█', Style::default().fg(pos_color));
                }
            }
        } else {
            let cells = (frac * lower as f64).round() as usize;
            for r in 0..cells {
                let row = zero_row + r;
                if row < height {
                    grid[row][col] = ('█', Style::default().fg(neg_color));
                }
            }
        }
    }
    // Zero baseline.
    if zero_row < height {
        for c in 0..n {
            if grid[zero_row][c].0 == ' ' {
                grid[zero_row][c] = ('─', Style::default().fg(Color::DarkGray));
            }
        }
    }
    grid.into_iter()
        .map(|row| {
            Line::from(
                row.into_iter()
                    .map(|(c, s)| Span::styled(c.to_string(), s))
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// Smooth line chart drawn as one row of unicode block bars. Useful for
/// short sparkline-style series.
pub fn render_sparkline(values: &[f64], width: usize, color: Color) -> Line<'static> {
    if values.is_empty() {
        return Line::from(Span::styled(" ".repeat(width), Style::default().fg(color)));
    }
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = (hi - lo).max(1e-9);
    let take: Vec<f64> = values.iter().rev().take(width).rev().copied().collect();
    let mut s = String::with_capacity(take.len());
    for v in take {
        let frac = ((v - lo) / span).clamp(0.0, 1.0);
        let idx = (frac * (BLOCKS.len() as f64 - 1.0)).round() as usize;
        s.push(BLOCKS[idx.min(BLOCKS.len() - 1)]);
    }
    Line::from(Span::styled(s, Style::default().fg(color)))
}

/// Multi-row line chart using `•` markers connected vertically.
pub fn render_line_chart(values: &[f64], height: usize, width: usize, color: Color) -> Vec<Line<'static>> {
    if values.is_empty() || height < 2 || width == 0 {
        return vec![Line::from(Span::styled(
            "(no data)".to_string(),
            Style::default().fg(Color::DarkGray),
        ))];
    }
    let take: Vec<f64> = values.iter().rev().take(width).rev().copied().collect();
    let lo = take.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = take.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = (hi - lo).max(1e-9);
    let mut grid: Vec<Vec<(char, Style)>> =
        (0..height).map(|_| (0..take.len()).map(|_| (' ', Style::default())).collect()).collect();
    let mut prev: Option<usize> = None;
    for (col, v) in take.iter().enumerate() {
        let frac = (v - lo) / span;
        let r = ((1.0 - frac) * (height as f64 - 1.0)).round() as usize;
        let r = r.min(height - 1);
        grid[r][col] = ('•', Style::default().fg(color));
        if let Some(pr) = prev {
            let (a, b) = if pr < r { (pr, r) } else { (r, pr) };
            for rr in a..=b {
                if grid[rr][col].0 == ' ' {
                    grid[rr][col] = ('·', Style::default().fg(color));
                }
            }
        }
        prev = Some(r);
    }
    grid.into_iter()
        .map(|row| {
            Line::from(
                row.into_iter()
                    .map(|(c, s)| Span::styled(c.to_string(), s))
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// Horizontal gauge filling left → right proportional to `value/max`.
pub fn render_gauge(value: f64, max: f64, width: usize, fill: Color) -> Line<'static> {
    let frac = if max > 0.0 { (value / max).clamp(0.0, 1.0) } else { 0.0 };
    let cells = (frac * width as f64).round() as usize;
    let cells = cells.min(width);
    let empty = width - cells;
    Line::from(vec![
        Span::styled("█".repeat(cells), Style::default().fg(fill)),
        Span::styled("░".repeat(empty), Style::default().fg(Color::DarkGray)),
    ])
}

/// Simple horizontal bar leaderboard. Each row: `label  ▮▮▮▮  value`.
pub fn render_h_bar_leaderboard(rows: &[(String, f64)], width: usize) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return vec![Line::from(Span::styled(
            "(no data)".to_string(),
            Style::default().fg(Color::DarkGray),
        ))];
    }
    let lo = rows.iter().map(|(_, v)| *v).fold(0.0_f64, f64::min);
    let hi = rows.iter().map(|(_, v)| *v).fold(0.0_f64, f64::max);
    let abs_max = hi.abs().max(lo.abs()).max(1e-9);
    let label_w = rows.iter().map(|(l, _)| l.chars().count()).max().unwrap_or(4).max(4);
    let mid = label_w + 1;
    let bar_w = width.saturating_sub(mid + 8); // leave room for value
    let half = bar_w / 2;
    rows.iter()
        .map(|(label, v)| {
            let mut spans: Vec<Span> = vec![Span::styled(
                format!("{:<width$} ", label, width = label_w),
                Style::default().add_modifier(Modifier::BOLD),
            )];
            let frac = (v.abs() / abs_max).clamp(0.0, 1.0);
            let cells = (frac * half as f64).round() as usize;
            let color = if *v >= 0.0 { Color::Green } else { Color::Red };
            // Left filler.
            spans.push(Span::raw(" ".repeat(half - if *v < 0.0 { cells } else { 0 })));
            if *v < 0.0 {
                spans.push(Span::styled("█".repeat(cells), Style::default().fg(color)));
            }
            spans.push(Span::raw("│"));
            if *v >= 0.0 {
                spans.push(Span::styled("█".repeat(cells), Style::default().fg(color)));
            }
            spans.push(Span::raw(" ".repeat(half.saturating_sub(if *v >= 0.0 { cells } else { 0 }))));
            spans.push(Span::raw(format!(" {:>+7.2}", v)));
            Line::from(spans)
        })
        .collect()
}

pub fn draw_graphs_panel(frame: &mut Frame, area: Rect, app: &AppState) {
    let title = match app.graphs_page {
        0 => "Graphs (1/6) — [ ] cycle",
        1 => "Graphs (2/6) — [ ] cycle",
        2 => "Graphs (3/6) — [ ] cycle",
        3 => "Graphs (4/6) — [ ] cycle",
        4 => "Graphs (5/6) — [ ] cycle",
        _ => "Graphs (6/6) — [ ] cycle",
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    // Title row at area.y is clickable to cycle pages.
    crate::ui::hit(
        ratatui::layout::Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        },
        crate::app::ClickAction::CycleGraphsPage,
    );
    frame.render_widget(block, area);
    if inner.height < 8 || inner.width < 50 {
        let p = Paragraph::new("terminal too small for graphs panel")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(p, inner);
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(inner);
    let top_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(33), Constraint::Percentage(33), Constraint::Percentage(34)])
        .split(rows[0]);
    let bot_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(33), Constraint::Percentage(33), Constraint::Percentage(34)])
        .split(rows[1]);

    match app.graphs_page {
        0 => {
            draw_bar_delta_chart(frame, top_cols[0], app);
            draw_cum_delta_chart(frame, top_cols[1], app);
            draw_spread_chart(frame, top_cols[2], app);
            draw_pace_gauge(frame, bot_cols[0], app);
            draw_trade_size_histogram(frame, bot_cols[1], app);
            draw_relative_strength(frame, bot_cols[2], app);
        }
        1 => {
            draw_imbalance_chart(frame, top_cols[0], app);
            draw_signals_by_kind(frame, top_cols[1], app);
            draw_tps_chart(frame, top_cols[2], app);
            draw_per_trade_pnl(frame, bot_cols[0], app);
            draw_hold_time_histogram(frame, bot_cols[1], app);
            draw_hour_of_day_activity(frame, bot_cols[2], app);
        }
        2 => {
            draw_multi_symbol_overlay(frame, top_cols[0], app);
            draw_per_symbol_delta(frame, top_cols[1], app);
            draw_depth_snapshot(frame, top_cols[2], app);
            draw_mid_price_history(frame, bot_cols[0], app);
            draw_signal_score_histogram(frame, bot_cols[1], app);
            draw_equity_drawdown(frame, bot_cols[2], app);
        }
        3 => {
            draw_per_symbol_spread(frame, top_cols[0], app);
            draw_per_symbol_volume(frame, top_cols[1], app);
            draw_buy_sell_count_ratio(frame, top_cols[2], app);
            draw_cumulative_depth_pyramid(frame, bot_cols[0], app);
            draw_slippage_history(frame, bot_cols[1], app);
            draw_poc_migration(frame, bot_cols[2], app);
        }
        4 => {
            draw_active_rsi_chart(frame, top_cols[0], app);
            draw_bar_range_chart(frame, top_cols[1], app);
            draw_bar_buy_sell_stack(frame, top_cols[2], app);
            draw_symbol_staleness(frame, bot_cols[0], app);
            draw_inter_arrival_histogram(frame, bot_cols[1], app);
            draw_working_orders_chart(frame, bot_cols[2], app);
        }
        _ => {
            draw_price_waterfall(frame, top_cols[0], app);
            draw_book_pressure_heat(frame, top_cols[1], app);
            draw_combined_indicators(frame, top_cols[2], app);
            draw_per_symbol_grid(frame, bot_cols[0], app);
            draw_account_equity_curve(frame, bot_cols[1], app);
            draw_buy_sell_volume_race(frame, bot_cols[2], app);
        }
    }
}

fn sub_block(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            title.to_string(),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))
}

fn draw_bar_delta_chart(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Bar Δ (last 30)");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }

    let total = app.active.candles.completed().len() + if app.active.candles.forming().is_some() { 1 } else { 0 };
    let take = (inner.width as usize).min(30);
    let start = total.saturating_sub(take);
    let values: Vec<f64> = app
        .active
        .candles
        .completed()
        .iter()
        .chain(app.active.candles.forming().into_iter())
        .skip(start)
        .map(|b| b.delta())
        .collect();
    let lines = render_bar_histogram(&values, inner.height as usize, Color::Green, Color::Red);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_cum_delta_chart(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Cumulative Δ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let trail: Vec<f64> = app.active.candles.cum_delta_trail().iter().copied().collect();
    let lines = render_line_chart(&trail, inner.height as usize, inner.width as usize, Color::Cyan);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_spread_chart(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Spread");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let history: Vec<f64> = app.spread_history.iter().copied().collect();
    let lines = if history.len() >= 2 {
        render_line_chart(&history, inner.height as usize, inner.width as usize, Color::Yellow)
    } else {
        vec![Line::from(Span::styled(
            "warming up…".to_string(),
            Style::default().fg(Color::DarkGray),
        ))]
    };
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_pace_gauge(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Pace (TPS)");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 || inner.width < 10 { return; }
    let cur = app.active.signals.pace.current_tps();
    let buy = app.active.signals.pace.buy_tps();
    let sell = app.active.signals.pace.sell_tps();
    let avg = app
        .active
        .signals
        .pace
        .session_avg_tps(app.active.event_count as i64 + 1)
        .unwrap_or(0.0);
    let max = (cur.max(avg) * 1.2).max(1.0);
    let lines = vec![
        Line::from(Span::raw(format!("cur {:>6.2}", cur))),
        render_gauge(cur, max, inner.width.saturating_sub(2) as usize, Color::Cyan),
        Line::from(Span::raw(format!("avg {:>6.2}", avg))),
        render_gauge(avg, max, inner.width.saturating_sub(2) as usize, Color::Gray),
        Line::from(Span::raw(format!("buy {:>6.2}", buy))),
        render_gauge(buy, max, inner.width.saturating_sub(2) as usize, Color::Green),
        Line::from(Span::raw(format!("sel {:>6.2}", sell))),
        render_gauge(sell, max, inner.width.saturating_sub(2) as usize, Color::Red),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_trade_size_histogram(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Trade size dist");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 || inner.width < 6 { return; }
    // Bucket the last N tape entries on a log10 scale.
    let qtys: Vec<f64> = app
        .active
        .tape
        .iter()
        .rev()
        .take(500)
        .map(|e| {
            let q: f64 = e.trade.qty.try_into().unwrap_or(0.0);
            q.max(1e-9)
        })
        .collect();
    if qtys.is_empty() {
        let p = Paragraph::new(Line::from(Span::styled(
            "no trades yet".to_string(),
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(p, inner);
        return;
    }
    let buckets_n = (inner.width as usize).min(20).max(4);
    let log_min = qtys.iter().copied().fold(f64::INFINITY, f64::min).max(1e-9).ln();
    let log_max = qtys.iter().copied().fold(f64::NEG_INFINITY, f64::max).max(1e-9).ln();
    let span = (log_max - log_min).max(1e-9);
    let mut counts = vec![0u32; buckets_n];
    for q in &qtys {
        let frac = ((q.ln() - log_min) / span).clamp(0.0, 1.0);
        let idx = ((frac * (buckets_n - 1) as f64).round() as usize).min(buckets_n - 1);
        counts[idx] += 1;
    }
    let max_c = counts.iter().copied().max().unwrap_or(1).max(1);
    let h = inner.height as usize;
    let mut grid: Vec<Vec<(char, Style)>> = (0..h).map(|_| (0..buckets_n).map(|_| (' ', Style::default())).collect()).collect();
    for (col, c) in counts.iter().enumerate() {
        let bar = ((*c as f64 / max_c as f64) * h as f64).round() as usize;
        for r in 0..bar {
            grid[h - 1 - r][col] = ('█', Style::default().fg(Color::Magenta));
        }
    }
    let lines: Vec<Line> = grid
        .into_iter()
        .map(|row| Line::from(row.into_iter().map(|(c, s)| Span::styled(c.to_string(), s)).collect::<Vec<_>>()))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_relative_strength(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Relative strength");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    // Each watchlist symbol: pct change from session open.
    let mut rows: Vec<(String, f64)> = app
        .watchlist
        .iter()
        .filter_map(|s| s.change_from_open_pct().map(|p| (trim_sym(&s.symbol), p)))
        .collect();
    if rows.is_empty() {
        let p = Paragraph::new(Line::from(Span::styled(
            "warming up…".to_string(),
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(p, inner);
        return;
    }
    rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let lines = render_h_bar_leaderboard(&rows, inner.width as usize);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_imbalance_chart(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Book imbalance (top10)");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let v: Vec<f64> = app.imbalance_history.iter().copied().collect();
    if v.len() < 2 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "warming up…".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    // Imbalance is in [-1, +1]; render as bar histogram around zero.
    let lines = render_bar_histogram(&v, inner.height as usize, Color::Green, Color::Red);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_signals_by_kind(frame: &mut Frame, area: Rect, app: &AppState) {
    use crate::signals::SignalKind;
    let block = sub_block("Signals by kind");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let kinds = [
        (SignalKind::Iceberg, "ICE"),
        (SignalKind::Absorption, "ABS"),
        (SignalKind::PaceSpike, "PCE"),
        (SignalKind::StopRun, "STP"),
        (SignalKind::Exhaustion, "EXH"),
    ];
    let counts: Vec<(String, f64)> = kinds
        .iter()
        .map(|(k, label)| {
            let n = app.active.signals.log.iter().filter(|s| s.kind == *k).count();
            (label.to_string(), n as f64)
        })
        .collect();
    let total: f64 = counts.iter().map(|(_, c)| *c).sum();
    if total <= 0.0 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no signals yet".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let lines = render_h_bar_leaderboard(&counts, inner.width as usize);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_tps_chart(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("TPS history");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let v: Vec<f64> = app.tps_history.iter().copied().collect();
    if v.len() < 2 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "warming up…".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let lines = render_line_chart(&v, inner.height as usize, inner.width as usize, Color::Cyan);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_per_trade_pnl(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Per-trade PnL");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let pnls: Vec<f64> = app
        .analytics
        .trades
        .iter()
        .filter_map(|t| t.pnl.try_into().ok())
        .collect();
    if pnls.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no trades yet".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let take: Vec<f64> = pnls.iter().rev().take(inner.width as usize).rev().copied().collect();
    let lines = render_bar_histogram(&take, inner.height as usize, Color::Green, Color::Red);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_hold_time_histogram(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Hold-time dist (sec)");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let times: Vec<f64> = app
        .analytics
        .trades
        .iter()
        .map(|t| (t.hold_ms as f64 / 1000.0).max(0.0))
        .collect();
    if times.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no closed trades yet".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let buckets_n = (inner.width as usize).min(20).max(4);
    let lo = times.iter().copied().fold(f64::INFINITY, f64::min).max(0.0);
    let hi = times.iter().copied().fold(f64::NEG_INFINITY, f64::max).max(lo + 1e-9);
    let span = (hi - lo).max(1e-9);
    let mut counts = vec![0u32; buckets_n];
    for v in &times {
        let frac = ((v - lo) / span).clamp(0.0, 1.0);
        let idx = ((frac * (buckets_n - 1) as f64).round() as usize).min(buckets_n - 1);
        counts[idx] += 1;
    }
    let max_c = counts.iter().copied().max().unwrap_or(1).max(1);
    let h = inner.height as usize;
    let mut grid: Vec<Vec<(char, Style)>> =
        (0..h).map(|_| (0..buckets_n).map(|_| (' ', Style::default())).collect()).collect();
    for (col, c) in counts.iter().enumerate() {
        let bar = ((*c as f64 / max_c as f64) * h as f64).round() as usize;
        for r in 0..bar {
            grid[h - 1 - r][col] = ('█', Style::default().fg(Color::Yellow));
        }
    }
    let lines: Vec<Line> = grid
        .into_iter()
        .map(|row| Line::from(row.into_iter().map(|(c, s)| Span::styled(c.to_string(), s)).collect::<Vec<_>>()))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_hour_of_day_activity(frame: &mut Frame, area: Rect, app: &AppState) {
    use time::OffsetDateTime;
    let block = sub_block("Trades / hour (UTC)");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let mut buckets = [0u32; 24];
    for entry in app.active.tape.iter() {
        let h = OffsetDateTime::from_unix_timestamp(entry.trade.time_ms / 1000)
            .map(|d| d.hour() as usize)
            .unwrap_or(0);
        if h < 24 {
            buckets[h] = buckets[h].saturating_add(1);
        }
    }
    let max_c = buckets.iter().copied().max().unwrap_or(1).max(1);
    let h_rows = inner.height as usize;
    // Width: 24 buckets need at least 24 cols. If less, downsample.
    let cols = (inner.width as usize).min(24);
    let step = 24 / cols.max(1);
    let mut grid: Vec<Vec<(char, Style)>> =
        (0..h_rows).map(|_| (0..cols).map(|_| (' ', Style::default())).collect()).collect();
    for c in 0..cols {
        let start = c * step;
        let end = ((c + 1) * step).min(24);
        let sum: u32 = buckets[start..end].iter().copied().sum();
        let bar = ((sum as f64 / max_c as f64) * h_rows as f64).round() as usize;
        for r in 0..bar {
            grid[h_rows - 1 - r][c] = ('█', Style::default().fg(Color::Magenta));
        }
    }
    let lines: Vec<Line> = grid
        .into_iter()
        .map(|row| Line::from(row.into_iter().map(|(c, s)| Span::styled(c.to_string(), s)).collect::<Vec<_>>()))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_multi_symbol_overlay(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Symbols (% from open)");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 || inner.width < 6 {
        return;
    }
    // Use the watchlist's per-minute sparkline samples; normalise each
    // series to its first sample so they render together on one axis.
    let series: Vec<(String, Vec<f64>, Color)> = app
        .watchlist
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let samples: Vec<f64> = s.sparkline.iter().copied().collect();
            if samples.len() < 2 { return None; }
            let base = samples[0];
            if base.abs() < 1e-12 { return None; }
            let pct: Vec<f64> = samples.iter().map(|v| (v - base) / base * 100.0).collect();
            const PALETTE: [Color; 8] = [
                Color::Cyan,
                Color::LightGreen,
                Color::LightMagenta,
                Color::LightYellow,
                Color::LightBlue,
                Color::Red,
                Color::Green,
                Color::Magenta,
            ];
            Some((trim_sym(&s.symbol), pct, PALETTE[i % PALETTE.len()]))
        })
        .collect();

    if series.is_empty() {
        let p = Paragraph::new(Line::from(Span::styled(
            "warming up…".to_string(),
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(p, inner);
        return;
    }

    // Use the longest series to set the chart x range.
    let max_len = series.iter().map(|(_, v, _)| v.len()).max().unwrap_or(0);
    let cols = (inner.width as usize).min(max_len);
    let h = inner.height as usize;

    // Global y-range across all series (last `cols` samples each).
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for (_, v, _) in &series {
        for x in v.iter().rev().take(cols) {
            if *x < lo { lo = *x; }
            if *x > hi { hi = *x; }
        }
    }
    if !lo.is_finite() || !hi.is_finite() {
        return;
    }
    let span = (hi - lo).max(1e-9);
    let mut grid: Vec<Vec<(char, Style)>> =
        (0..h).map(|_| (0..cols).map(|_| (' ', Style::default())).collect()).collect();

    // Zero baseline.
    let zero_frac = (0.0 - lo) / span;
    let zero_row = ((1.0 - zero_frac) * (h as f64 - 1.0)).round() as usize;
    if zero_row < h {
        for c in 0..cols {
            grid[zero_row][c] = ('·', Style::default().fg(Color::DarkGray));
        }
    }

    for (_, v, color) in &series {
        let take: Vec<f64> = v.iter().rev().take(cols).rev().copied().collect();
        let mut prev: Option<usize> = None;
        for (col, x) in take.iter().enumerate() {
            let frac = (x - lo) / span;
            let r = ((1.0 - frac) * (h as f64 - 1.0)).round() as usize;
            let r = r.min(h - 1);
            grid[r][col] = ('●', Style::default().fg(*color));
            if let Some(pr) = prev {
                let (a, b) = if pr < r { (pr, r) } else { (r, pr) };
                for rr in a..=b {
                    if grid[rr][col].0 == ' ' || grid[rr][col].0 == '·' {
                        grid[rr][col] = ('·', Style::default().fg(*color));
                    }
                }
            }
            prev = Some(r);
        }
    }

    // Right-edge legend.
    let mut lines: Vec<Line> = Vec::with_capacity(h);
    for (r, row) in grid.into_iter().enumerate() {
        let mut spans: Vec<Span> = row
            .into_iter()
            .map(|(c, s)| Span::styled(c.to_string(), s))
            .collect();
        if let Some((label, samples, color)) = series.get(r) {
            let cur = *samples.last().unwrap_or(&0.0);
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("{label} {:>+5.2}%", cur),
                Style::default().fg(*color).add_modifier(Modifier::BOLD),
            ));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_per_symbol_delta(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Symbol session Δ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let rows: Vec<(String, f64)> = app
        .watchlist
        .iter()
        .map(|s| (trim_sym(&s.symbol), s.delta()))
        .filter(|(_, d)| *d != 0.0)
        .collect();
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "warming up…".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let lines = render_h_bar_leaderboard(&rows, inner.width as usize);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_depth_snapshot(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Depth snapshot");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 6 || inner.width < 12 {
        return;
    }
    let h = inner.height as usize;
    let per_side = h / 2;
    let asks: Vec<(rust_decimal::Decimal, rust_decimal::Decimal)> =
        app.active.book.asks.iter_asc().take(per_side).collect();
    let bids: Vec<(rust_decimal::Decimal, rust_decimal::Decimal)> =
        app.active.book.bids.iter_desc().take(per_side).collect();
    let max_q: f64 = asks
        .iter()
        .chain(bids.iter())
        .map(|(_, q)| {
            let v: f64 = (*q).try_into().unwrap_or(0.0);
            v
        })
        .fold(0.0_f64, f64::max)
        .max(1e-9);
    let bar_w = inner.width.saturating_sub(8) as usize;

    let mut lines: Vec<Line> = Vec::with_capacity(h);
    // Asks descending: highest at top.
    for (price, qty) in asks.iter().rev() {
        let q: f64 = (*qty).try_into().unwrap_or(0.0);
        let cells = ((q / max_q) * bar_w as f64).round() as usize;
        let p: f64 = (*price).try_into().unwrap_or(0.0);
        lines.push(Line::from(vec![
            Span::raw(format!("{p:>8.2} ")),
            Span::styled("█".repeat(cells), Style::default().fg(Color::Red)),
        ]));
    }
    // Bids: highest first.
    for (price, qty) in &bids {
        let q: f64 = (*qty).try_into().unwrap_or(0.0);
        let cells = ((q / max_q) * bar_w as f64).round() as usize;
        let p: f64 = (*price).try_into().unwrap_or(0.0);
        lines.push(Line::from(vec![
            Span::raw(format!("{p:>8.2} ")),
            Span::styled("█".repeat(cells), Style::default().fg(Color::Green)),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "no book yet".to_string(),
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_mid_price_history(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Mid-price history");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let v: Vec<f64> = app.mid_history.iter().copied().collect();
    if v.len() < 2 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "warming up…".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let lines = render_line_chart(&v, inner.height as usize, inner.width as usize, Color::Yellow);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_signal_score_histogram(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Signal score dist");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let mut counts = [0u32; 5];
    for s in &app.active.signals.log {
        let i = (s.score.saturating_sub(1) as usize).min(4);
        counts[i] += 1;
    }
    let total: u32 = counts.iter().sum();
    if total == 0 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no signals".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let max_c = counts.iter().copied().max().unwrap_or(1).max(1);
    let h = inner.height as usize;
    let cols = 5;
    let bar_w = (inner.width as usize / cols).max(1);
    let mut grid: Vec<Vec<(char, Style)>> =
        (0..h).map(|_| (0..bar_w * cols).map(|_| (' ', Style::default())).collect()).collect();
    for (i, c) in counts.iter().enumerate() {
        let bar = ((*c as f64 / max_c as f64) * h as f64).round() as usize;
        for r in 0..bar {
            for off in 0..bar_w.saturating_sub(1) {
                let col = i * bar_w + off;
                if col < grid[0].len() {
                    grid[h - 1 - r][col] = ('█', Style::default().fg(Color::Yellow));
                }
            }
        }
    }
    let lines: Vec<Line> = grid
        .into_iter()
        .map(|row| Line::from(row.into_iter().map(|(c, s)| Span::styled(c.to_string(), s)).collect::<Vec<_>>()))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_equity_drawdown(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Equity drawdown");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let cum: Vec<f64> = app
        .analytics
        .cumulative_pnl()
        .into_iter()
        .filter_map(|d| d.try_into().ok())
        .collect();
    if cum.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no closed trades yet".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    // Drawdown at each point: peak_so_far - cum.
    let mut peak = f64::NEG_INFINITY;
    let dd: Vec<f64> = cum
        .iter()
        .map(|v| {
            if *v > peak {
                peak = *v;
            }
            -(peak - v)
        })
        .collect();
    let lines = render_bar_histogram(&dd, inner.height as usize, Color::Green, Color::Red);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_per_symbol_spread(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Spread (price)");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let rows: Vec<(String, f64)> = app
        .watchlist
        .iter()
        .filter_map(|s| s.spread.map(|sp| (trim_sym(&s.symbol), sp)))
        .collect();
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "warming up…".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let lines = render_h_bar_leaderboard(&rows, inner.width as usize);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_per_symbol_volume(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Session vol");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let rows: Vec<(String, f64)> = app
        .watchlist
        .iter()
        .map(|s| (trim_sym(&s.symbol), s.total_volume()))
        .filter(|(_, v)| *v > 0.0)
        .collect();
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "warming up…".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let lines = render_h_bar_leaderboard(&rows, inner.width as usize);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_buy_sell_count_ratio(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Trade count B/S");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 || inner.width < 8 { return; }
    let mut buys = 0u32;
    let mut sells = 0u32;
    for entry in app.active.tape.iter() {
        match entry.trade.side {
            crate::delta::Side::Buy => buys += 1,
            crate::delta::Side::Sell => sells += 1,
        }
    }
    let total = buys + sells;
    if total == 0 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no trades yet".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let buy_pct = buys as f64 / total as f64 * 100.0;
    let sell_pct = sells as f64 / total as f64 * 100.0;
    let lines = vec![
        Line::from(format!("buys:  {} ({:>5.1}%)", buys, buy_pct)),
        render_gauge(buys as f64, total as f64, inner.width.saturating_sub(2) as usize, Color::Green),
        Line::from(format!("sells: {} ({:>5.1}%)", sells, sell_pct)),
        render_gauge(sells as f64, total as f64, inner.width.saturating_sub(2) as usize, Color::Red),
        Line::from(""),
        Line::from(Span::styled(
            format!("ratio: {:.2}", buys as f64 / sells.max(1) as f64),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_cumulative_depth_pyramid(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Cumulative depth");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 6 || inner.width < 12 { return; }
    let h = inner.height as usize;
    let per_side = h / 2;
    // Cumulative running quantity outward from the spread.
    let mut cum_a = 0.0_f64;
    let mut cum_b = 0.0_f64;
    let asks_cum: Vec<f64> = app
        .active
        .book
        .asks
        .iter_asc()
        .take(per_side)
        .map(|(_, q)| {
            let qq: f64 = q.try_into().unwrap_or(0.0);
            cum_a += qq;
            cum_a
        })
        .collect();
    let bids_cum: Vec<f64> = app
        .active
        .book
        .bids
        .iter_desc()
        .take(per_side)
        .map(|(_, q)| {
            let qq: f64 = q.try_into().unwrap_or(0.0);
            cum_b += qq;
            cum_b
        })
        .collect();
    let max = asks_cum.iter().chain(bids_cum.iter()).copied().fold(0.0_f64, f64::max).max(1e-9);
    let bar_w = inner.width.saturating_sub(8) as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(h);
    // Asks: outer (deepest) at top.
    for v in asks_cum.iter().rev() {
        let cells = ((v / max) * bar_w as f64).round() as usize;
        lines.push(Line::from(vec![
            Span::raw(format!("{v:>8.2} ")),
            Span::styled("█".repeat(cells), Style::default().fg(Color::Red)),
        ]));
    }
    for v in &bids_cum {
        let cells = ((v / max) * bar_w as f64).round() as usize;
        lines.push(Line::from(vec![
            Span::raw(format!("{v:>8.2} ")),
            Span::styled("█".repeat(cells), Style::default().fg(Color::Green)),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "no book yet".to_string(),
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_slippage_history(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Per-order slippage");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let v: Vec<f64> = app
        .paper
        .orders
        .iter()
        .filter_map(|o| {
            if matches!(
                o.status,
                crate::paper::OrderStatus::Filled | crate::paper::OrderStatus::PartialFill
            ) {
                let f: f64 = o.slippage.try_into().unwrap_or(0.0);
                Some(f)
            } else {
                None
            }
        })
        .collect();
    if v.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no fills yet".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let take: Vec<f64> = v.iter().rev().take(inner.width as usize).rev().copied().collect();
    let lines = render_bar_histogram(&take, inner.height as usize, Color::Green, Color::Red);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_poc_migration(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("POC migration");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let pocs: Vec<f64> = app
        .active
        .footprint
        .completed()
        .iter()
        .chain(app.active.footprint.forming().into_iter())
        .filter_map(|b| b.point_of_control().and_then(|p| p.try_into().ok()))
        .collect();
    if pocs.len() < 2 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "warming up…".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let lines = render_line_chart(&pocs, inner.height as usize, inner.width as usize, Color::LightCyan);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_active_rsi_chart(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("RSI(14) on closes");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 4 { return; }
    let closes: Vec<f64> = app
        .active
        .candles
        .completed()
        .iter()
        .chain(app.active.candles.forming().into_iter())
        .map(|b| b.close)
        .collect();
    if closes.len() < 16 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "warming up…".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let mut rsi = crate::indicators::Rsi::new(14);
    let series: Vec<f64> = closes
        .iter()
        .filter_map(|c| rsi.update(*c))
        .collect();
    if series.is_empty() {
        return;
    }
    // Force chart bounds to [0, 100].
    let take: Vec<f64> = series.iter().rev().take(inner.width as usize).rev().copied().collect();
    let h = inner.height as usize;
    let mut grid: Vec<Vec<(char, Style)>> =
        (0..h).map(|_| (0..take.len()).map(|_| (' ', Style::default())).collect()).collect();
    // Reference rows at 30 and 70.
    for level in [30.0, 70.0] {
        let r = (((100.0 - level) / 100.0) * (h as f64 - 1.0)).round() as usize;
        if r < h {
            for c in 0..take.len() {
                grid[r][c] = ('-', Style::default().fg(Color::DarkGray));
            }
        }
    }
    let mut prev: Option<usize> = None;
    for (col, v) in take.iter().enumerate() {
        let r = (((100.0 - v) / 100.0) * (h as f64 - 1.0)).clamp(0.0, h as f64 - 1.0) as usize;
        let color = if *v >= 70.0 { Color::Red } else if *v <= 30.0 { Color::Green } else { Color::Cyan };
        grid[r][col] = ('●', Style::default().fg(color));
        if let Some(pr) = prev {
            let (a, b) = if pr < r { (pr, r) } else { (r, pr) };
            for rr in a..=b {
                if grid[rr][col].0 == ' ' || grid[rr][col].0 == '-' {
                    grid[rr][col] = ('·', Style::default().fg(color));
                }
            }
        }
        prev = Some(r);
    }
    let lines: Vec<Line> = grid
        .into_iter()
        .map(|row| Line::from(row.into_iter().map(|(c, s)| Span::styled(c.to_string(), s)).collect::<Vec<_>>()))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_bar_range_chart(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Bar HL range");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let v: Vec<f64> = app
        .active
        .candles
        .completed()
        .iter()
        .chain(app.active.candles.forming().into_iter())
        .map(|b| b.high - b.low)
        .collect();
    if v.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "warming up…".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let take: Vec<f64> = v.iter().rev().take(inner.width as usize).rev().copied().collect();
    let h = inner.height as usize;
    let max = take.iter().copied().fold(0.0_f64, f64::max).max(1e-9);
    let mut grid: Vec<Vec<(char, Style)>> =
        (0..h).map(|_| (0..take.len()).map(|_| (' ', Style::default())).collect()).collect();
    for (col, v) in take.iter().enumerate() {
        let cells = ((v / max) * h as f64).round() as usize;
        for r in 0..cells {
            grid[h - 1 - r][col] = ('█', Style::default().fg(Color::LightYellow));
        }
    }
    let lines: Vec<Line> = grid
        .into_iter()
        .map(|row| Line::from(row.into_iter().map(|(c, s)| Span::styled(c.to_string(), s)).collect::<Vec<_>>()))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_bar_buy_sell_stack(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Bar buy/sell vol");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let bars: Vec<(f64, f64)> = app
        .active
        .candles
        .completed()
        .iter()
        .chain(app.active.candles.forming().into_iter())
        .map(|b| (b.buy_volume, b.sell_volume))
        .collect();
    if bars.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "warming up…".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let take: Vec<(f64, f64)> = bars.iter().rev().take(inner.width as usize).rev().copied().collect();
    let max = take
        .iter()
        .map(|(b, s)| b + s)
        .fold(0.0_f64, f64::max)
        .max(1e-9);
    let h = inner.height as usize;
    let mut grid: Vec<Vec<(char, Style)>> =
        (0..h).map(|_| (0..take.len()).map(|_| (' ', Style::default())).collect()).collect();
    for (col, (buy, sell)) in take.iter().enumerate() {
        let buy_cells = ((buy / max) * h as f64).round() as usize;
        let sell_cells = ((sell / max) * h as f64).round() as usize;
        for r in 0..buy_cells.min(h) {
            grid[h - 1 - r][col] = ('█', Style::default().fg(Color::Green));
        }
        for r in 0..sell_cells.min(h.saturating_sub(buy_cells)) {
            let row = h.saturating_sub(buy_cells).saturating_sub(1 + r);
            if row < h {
                grid[row][col] = ('█', Style::default().fg(Color::Red));
            }
        }
    }
    let lines: Vec<Line> = grid
        .into_iter()
        .map(|row| Line::from(row.into_iter().map(|(c, s)| Span::styled(c.to_string(), s)).collect::<Vec<_>>()))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_symbol_staleness(frame: &mut Frame, area: Rect, app: &AppState) {
    use time::OffsetDateTime;
    let block = sub_block("Symbol staleness (s)");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let now_ms = OffsetDateTime::now_utc().unix_timestamp() * 1000;
    let rows: Vec<(String, f64)> = app
        .watchlist
        .iter()
        .filter(|s| s.last_trade_ms > 0)
        .map(|s| {
            let age = ((now_ms - s.last_trade_ms).max(0) as f64) / 1000.0;
            (trim_sym(&s.symbol), age)
        })
        .collect();
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "warming up…".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let lines = render_h_bar_leaderboard(&rows, inner.width as usize);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_inter_arrival_histogram(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Trade inter-arrival (ms)");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 || inner.width < 6 { return; }
    let mut diffs: Vec<f64> = Vec::new();
    let mut prev: Option<i64> = None;
    // Walk tape oldest-first to compute deltas.
    for entry in app.active.tape.iter().rev() {
        let t = entry.trade.time_ms;
        if let Some(p) = prev {
            let d = (t - p).max(0) as f64;
            diffs.push(d);
        }
        prev = Some(t);
    }
    if diffs.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "warming up…".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let buckets_n = (inner.width as usize).min(20).max(4);
    let lo = 0.0;
    let hi = diffs.iter().copied().fold(0.0_f64, f64::max).max(1.0);
    let span = (hi - lo).max(1e-9);
    let mut counts = vec![0u32; buckets_n];
    for d in &diffs {
        let frac = ((d - lo) / span).clamp(0.0, 1.0);
        let idx = ((frac * (buckets_n - 1) as f64).round() as usize).min(buckets_n - 1);
        counts[idx] += 1;
    }
    let max_c = counts.iter().copied().max().unwrap_or(1).max(1);
    let h = inner.height as usize;
    let mut grid: Vec<Vec<(char, Style)>> =
        (0..h).map(|_| (0..buckets_n).map(|_| (' ', Style::default())).collect()).collect();
    for (col, c) in counts.iter().enumerate() {
        let bar = ((*c as f64 / max_c as f64) * h as f64).round() as usize;
        for r in 0..bar {
            grid[h - 1 - r][col] = ('█', Style::default().fg(Color::LightCyan));
        }
    }
    let lines: Vec<Line> = grid
        .into_iter()
        .map(|row| Line::from(row.into_iter().map(|(c, s)| Span::styled(c.to_string(), s)).collect::<Vec<_>>()))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_working_orders_chart(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Working orders");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let v: Vec<f64> = app.working_orders_history.iter().copied().collect();
    if v.iter().sum::<f64>() == 0.0 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no working orders".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let lines = render_line_chart(&v, inner.height as usize, inner.width as usize, Color::Magenta);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_price_waterfall(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Tape waterfall");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 || inner.width < 12 { return; }
    let h = inner.height as usize;
    // Map last `h` trades onto rows. Width column shows direction shading +
    // qty bar.
    let entries: Vec<&crate::engines::TapeEntry> = app.active.tape.iter().take(h).collect();
    if entries.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no trades yet".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let max_q: f64 = entries
        .iter()
        .map(|e| {
            let q: f64 = e.trade.qty.try_into().unwrap_or(0.0);
            q
        })
        .fold(0.0_f64, f64::max)
        .max(1e-9);
    let bar_w = inner.width.saturating_sub(13) as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(entries.len());
    for e in entries {
        let q: f64 = e.trade.qty.try_into().unwrap_or(0.0);
        let p: f64 = e.trade.price.try_into().unwrap_or(0.0);
        let cells = ((q / max_q) * bar_w as f64).round() as usize;
        let (color, marker) = match e.trade.side {
            crate::delta::Side::Buy => (Color::Green, "▲"),
            crate::delta::Side::Sell => (Color::Red, "▼"),
        };
        let style = if e.large {
            Style::default().fg(color).add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default().fg(color)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker} "), style),
            Span::raw(format!("{p:>9.2} ")),
            Span::styled("█".repeat(cells), style),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_book_pressure_heat(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Book pressure heat");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 6 || inner.width < 12 { return; }
    let h = inner.height as usize;
    let per_side = h / 2;
    let asks: Vec<(rust_decimal::Decimal, rust_decimal::Decimal)> =
        app.active.book.asks.iter_asc().take(per_side).collect();
    let bids: Vec<(rust_decimal::Decimal, rust_decimal::Decimal)> =
        app.active.book.bids.iter_desc().take(per_side).collect();
    let max_q: f64 = asks
        .iter()
        .chain(bids.iter())
        .map(|(_, q)| {
            let v: f64 = (*q).try_into().unwrap_or(0.0);
            v
        })
        .fold(0.0_f64, f64::max)
        .max(1e-9);
    let bar_w = inner.width.saturating_sub(8) as usize;
    let mut lines: Vec<Line> = Vec::new();
    for (price, qty) in asks.iter().rev() {
        let q: f64 = (*qty).try_into().unwrap_or(0.0);
        let frac = (q / max_q).clamp(0.0, 1.0);
        let cells = (frac * bar_w as f64).round() as usize;
        let intensity = (frac * 4.0).clamp(0.0, 4.0) as u8;
        let color = match intensity {
            0 => Color::Rgb(60, 0, 0),
            1 => Color::Rgb(120, 0, 0),
            2 => Color::Rgb(180, 30, 30),
            3 => Color::Rgb(220, 60, 60),
            _ => Color::Rgb(255, 90, 90),
        };
        let p: f64 = (*price).try_into().unwrap_or(0.0);
        lines.push(Line::from(vec![
            Span::raw(format!("{p:>8.2} ")),
            Span::styled("█".repeat(cells), Style::default().fg(color)),
        ]));
    }
    for (price, qty) in &bids {
        let q: f64 = (*qty).try_into().unwrap_or(0.0);
        let frac = (q / max_q).clamp(0.0, 1.0);
        let cells = (frac * bar_w as f64).round() as usize;
        let intensity = (frac * 4.0).clamp(0.0, 4.0) as u8;
        let color = match intensity {
            0 => Color::Rgb(0, 60, 0),
            1 => Color::Rgb(0, 120, 0),
            2 => Color::Rgb(30, 180, 30),
            3 => Color::Rgb(60, 220, 60),
            _ => Color::Rgb(90, 255, 90),
        };
        let p: f64 = (*price).try_into().unwrap_or(0.0);
        lines.push(Line::from(vec![
            Span::raw(format!("{p:>8.2} ")),
            Span::styled("█".repeat(cells), Style::default().fg(color)),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "no book yet".to_string(),
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_combined_indicators(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Combined indicators");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 4 || inner.width < 8 { return; }
    let closes: Vec<f64> = app
        .active
        .candles
        .completed()
        .iter()
        .chain(app.active.candles.forming().into_iter())
        .map(|b| b.close)
        .collect();
    if closes.len() < 30 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "warming up…".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let mut rsi_eng = crate::indicators::Rsi::new(14);
    let rsi: Vec<f64> = closes.iter().filter_map(|c| rsi_eng.update(*c)).collect();
    let mut macd_eng = crate::indicators::Macd::new(12, 26, 9);
    let hist: Vec<f64> = closes
        .iter()
        .filter_map(|c| macd_eng.update(*c).map(|m| m.histogram))
        .collect();
    // Normalise both into [0, 1].
    let take = inner.width as usize;
    let rsi_n: Vec<f64> = rsi
        .iter()
        .rev()
        .take(take)
        .rev()
        .map(|v| v / 100.0)
        .collect();
    let h_lo = hist.iter().copied().fold(0.0_f64, f64::min);
    let h_hi = hist.iter().copied().fold(0.0_f64, f64::max);
    let span = (h_hi - h_lo).max(1e-9);
    let hist_n: Vec<f64> = hist
        .iter()
        .rev()
        .take(take)
        .rev()
        .map(|v| (v - h_lo) / span)
        .collect();
    let h = inner.height as usize;
    let mut grid: Vec<Vec<(char, Style)>> =
        (0..h).map(|_| (0..take).map(|_| (' ', Style::default())).collect()).collect();
    let plot = |grid: &mut Vec<Vec<(char, Style)>>, v: &[f64], color: Color, ch: char| {
        let mut prev: Option<usize> = None;
        for (col, x) in v.iter().enumerate() {
            let r = ((1.0 - x) * (h as f64 - 1.0)).round().clamp(0.0, (h - 1) as f64) as usize;
            grid[r][col] = (ch, Style::default().fg(color));
            if let Some(pr) = prev {
                let (a, b) = if pr < r { (pr, r) } else { (r, pr) };
                for rr in a..=b {
                    if grid[rr][col].0 == ' ' {
                        grid[rr][col] = ('·', Style::default().fg(color));
                    }
                }
            }
            prev = Some(r);
        }
    };
    plot(&mut grid, &rsi_n, Color::Cyan, '●');
    plot(&mut grid, &hist_n, Color::Magenta, '◆');
    let lines: Vec<Line> = grid
        .into_iter()
        .map(|row| Line::from(row.into_iter().map(|(c, s)| Span::styled(c.to_string(), s)).collect::<Vec<_>>()))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_per_symbol_grid(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Symbol grid");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let mut lines: Vec<Line> = Vec::new();
    let spark_w = inner.width.saturating_sub(28) as usize;
    for s in &app.watchlist {
        let chg = s.change_from_open_pct().unwrap_or(0.0);
        let color = if chg < 0.0 { Color::Red } else if chg > 0.0 { Color::Green } else { Color::Gray };
        let conn = if s.connected { "●" } else { "○" };
        let conn_color = if s.connected { Color::Green } else { Color::Red };
        let spark = render_sparkline_string(&s.sparkline, spark_w);
        lines.push(Line::from(vec![
            Span::styled(conn.to_string(), Style::default().fg(conn_color)),
            Span::raw(" "),
            Span::styled(format!("{:<8}", trim_sym(&s.symbol)), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(" {:>9.2} ", s.last_price)),
            Span::styled(format!("{:>+5.2}% ", chg), Style::default().fg(color)),
            Span::styled(spark, Style::default().fg(color)),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "no symbols watched".to_string(),
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_sparkline_string(data: &std::collections::VecDeque<f64>, width: usize) -> String {
    if data.is_empty() {
        return " ".repeat(width);
    }
    let chars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let lo = data.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = data.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = (hi - lo).max(1e-9);
    let take = data.iter().rev().take(width).rev();
    let mut out = String::with_capacity(width);
    for v in take {
        let frac = ((v - lo) / span).clamp(0.0, 1.0);
        let idx = (frac * (chars.len() as f64 - 1.0)).round() as usize;
        out.push(chars[idx.min(chars.len() - 1)]);
    }
    out
}

fn draw_account_equity_curve(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Equity curve");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 { return; }
    let realized: Vec<f64> = app
        .analytics
        .cumulative_pnl()
        .into_iter()
        .filter_map(|d| d.try_into().ok())
        .collect();
    if realized.is_empty() {
        let unr = app.paper.unrealized_pnl();
        let unr_f: f64 = unr.try_into().unwrap_or(0.0);
        let bal: f64 = app.paper.balance().try_into().unwrap_or(0.0);
        let txt = format!(
            "balance {:>10.2}\nunrealized {:>+10.2}",
            bal, unr_f
        );
        frame.render_widget(
            Paragraph::new(txt).style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    }
    let lines = render_line_chart(&realized, inner.height as usize, inner.width as usize, Color::Green);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_buy_sell_volume_race(frame: &mut Frame, area: Rect, app: &AppState) {
    let block = sub_block("Buy vs sell vol race");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 4 || inner.width < 6 { return; }
    let buy: f64 = app.active.delta.buy_volume().try_into().unwrap_or(0.0);
    let sell: f64 = app.active.delta.sell_volume().try_into().unwrap_or(0.0);
    let total = buy + sell;
    if total <= 0.0 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no flow yet".to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    }
    let bar_w = inner.width.saturating_sub(2) as usize;
    let buy_cells = ((buy / total) * bar_w as f64).round() as usize;
    let sell_cells = bar_w.saturating_sub(buy_cells);
    let lines = vec![
        Line::from(format!("buy  {:>10.2}", buy)),
        Line::from(vec![
            Span::styled("█".repeat(buy_cells), Style::default().fg(Color::Green)),
            Span::styled("█".repeat(sell_cells), Style::default().fg(Color::Red)),
        ]),
        Line::from(format!("sell {:>10.2}", sell)),
        Line::from(format!("Δ    {:>+10.2}", buy - sell)),
        Line::from(format!("ratio {:>9.2}", buy / sell.max(1e-9))),
    ];
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparkline_renders_correct_width() {
        let line = render_sparkline(&[1.0, 2.0, 3.0, 4.0], 8, Color::Cyan);
        let total: usize = line
            .spans
            .iter()
            .map(|s| s.content.chars().count())
            .sum();
        assert_eq!(total, 4); // sparkline equals data length, capped at width
    }

    #[test]
    fn line_chart_height_matches() {
        let v: Vec<f64> = (0..30).map(|i| (i as f64).sin()).collect();
        let lines = render_line_chart(&v, 5, 30, Color::Cyan);
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn bar_histogram_handles_mixed_signs() {
        let v = vec![1.0, -2.0, 3.0, -1.0];
        let lines = render_bar_histogram(&v, 4, Color::Green, Color::Red);
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn gauge_clamps_overflow() {
        let line = render_gauge(150.0, 100.0, 10, Color::Cyan);
        let total: usize = line
            .spans
            .iter()
            .map(|s| s.content.chars().count())
            .sum();
        assert_eq!(total, 10);
    }

    #[test]
    fn leaderboard_renders_one_row_per_input() {
        let rows = vec![("BTC".to_string(), 1.5), ("ETH".to_string(), -0.3)];
        let lines = render_h_bar_leaderboard(&rows, 40);
        assert_eq!(lines.len(), 2);
    }
}
