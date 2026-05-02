//! Candlestick chart rendering — half-block bodies/wicks, volume histogram,
//! VWAP line, EMA overlays, indicator sub-panel, time/price axes.
//!
//! This module is purely a renderer: it reads from the [`AppState`] (the
//! candle engine, indicators, etc.) and writes lines/spans to a ratatui
//! [`Frame`]. It owns no engine state itself.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use time::OffsetDateTime;
use time::format_description::FormatItem;
use time::macros::format_description;

use crate::app::AppState;
use crate::candle::{AggregatedSeries, CandleBar, TIMEFRAMES_MS, heikin_ashi_series};
use crate::indicators::{Ema, Macd, Rsi};

const HHMM: &[FormatItem<'_>] = format_description!("[hour]:[minute]");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandleType {
    Standard,
    HeikinAshi,
    Delta,
}

impl CandleType {
    pub fn label(&self) -> &'static str {
        match self {
            CandleType::Standard => "STD",
            CandleType::HeikinAshi => "HA ",
            CandleType::Delta => "DLT",
        }
    }
    pub fn next(&self) -> Self {
        match self {
            CandleType::Standard => CandleType::HeikinAshi,
            CandleType::HeikinAshi => CandleType::Delta,
            CandleType::Delta => CandleType::Standard,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeMode {
    Standard,
    DeltaStack,
}

impl VolumeMode {
    pub fn label(&self) -> &'static str {
        match self {
            VolumeMode::Standard => "vol",
            VolumeMode::DeltaStack => "B/S",
        }
    }
    pub fn next(&self) -> Self {
        match self {
            VolumeMode::Standard => VolumeMode::DeltaStack,
            VolumeMode::DeltaStack => VolumeMode::Standard,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndicatorMode {
    Rsi,
    DeltaMomentum,
    Macd,
}

impl IndicatorMode {
    pub fn label(&self) -> &'static str {
        match self {
            IndicatorMode::Rsi => "RSI",
            IndicatorMode::DeltaMomentum => "DMO",
            IndicatorMode::Macd => "MAC",
        }
    }
    pub fn next(&self) -> Self {
        match self {
            IndicatorMode::Rsi => IndicatorMode::DeltaMomentum,
            IndicatorMode::DeltaMomentum => IndicatorMode::Macd,
            IndicatorMode::Macd => IndicatorMode::Rsi,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChartView {
    pub timeframe_ms: i64,
    pub candle_type: CandleType,
    pub volume_mode: VolumeMode,
    pub indicator: IndicatorMode,
    pub show_vwap: bool,
    pub show_emas: [bool; 3],
    pub show_volume_profile: bool,
    pub show_cum_delta: bool,
    pub show_indicator_panel: bool,
    /// Number of bars scrolled back from the live edge (0 = at live edge).
    pub scroll_back: usize,
    /// User-defined horizontal alert price levels.
    pub alerts: Vec<f64>,
    /// Crosshair active flag.
    pub crosshair: bool,
    /// Index from the right edge of visible bars (0 = newest visible).
    pub cross_offset: usize,
}

impl Default for ChartView {
    fn default() -> Self {
        Self {
            timeframe_ms: 60_000,
            candle_type: CandleType::Standard,
            volume_mode: VolumeMode::Standard,
            indicator: IndicatorMode::Rsi,
            show_vwap: true,
            show_emas: [true, true, true],
            show_volume_profile: true,
            show_cum_delta: true,
            show_indicator_panel: true,
            scroll_back: 0,
            alerts: Vec::new(),
            crosshair: false,
            cross_offset: 0,
        }
    }
}

impl ChartView {
    pub fn cycle_timeframe_up(&mut self) {
        let i = TIMEFRAMES_MS.iter().position(|&t| t == self.timeframe_ms).unwrap_or(0);
        self.timeframe_ms = TIMEFRAMES_MS[(i + 1).min(TIMEFRAMES_MS.len() - 1)];
        self.scroll_back = 0;
    }
    pub fn cycle_timeframe_down(&mut self) {
        let i = TIMEFRAMES_MS.iter().position(|&t| t == self.timeframe_ms).unwrap_or(0);
        self.timeframe_ms = TIMEFRAMES_MS[i.saturating_sub(1)];
        self.scroll_back = 0;
    }
    pub fn set_timeframe_index(&mut self, idx: usize) {
        if idx < TIMEFRAMES_MS.len() {
            self.timeframe_ms = TIMEFRAMES_MS[idx];
            self.scroll_back = 0;
        }
    }
    pub fn timeframe_label(&self) -> &'static str {
        match self.timeframe_ms {
            60_000 => "1m",
            180_000 => "3m",
            300_000 => "5m",
            900_000 => "15m",
            3_600_000 => "1h",
            _ => "?",
        }
    }
    pub fn cycle_candle_type(&mut self) {
        self.candle_type = self.candle_type.next();
    }
    pub fn cycle_volume_mode(&mut self) {
        self.volume_mode = self.volume_mode.next();
    }
    pub fn cycle_indicator(&mut self) {
        self.indicator = self.indicator.next();
    }
    pub fn toggle_ema(&mut self, idx: usize) {
        if idx < 3 {
            self.show_emas[idx] = !self.show_emas[idx];
        }
    }
    pub fn toggle_vwap(&mut self) { self.show_vwap = !self.show_vwap; }
    pub fn toggle_volume_profile(&mut self) { self.show_volume_profile = !self.show_volume_profile; }
    pub fn toggle_cum_delta(&mut self) { self.show_cum_delta = !self.show_cum_delta; }
    pub fn toggle_indicator_panel(&mut self) { self.show_indicator_panel = !self.show_indicator_panel; }
    pub fn add_alert(&mut self, price: f64) {
        if price.is_finite() && !self.alerts.contains(&price) {
            self.alerts.push(price);
        }
    }
    pub fn remove_nearest_alert(&mut self, price: f64) {
        if let Some((idx, _)) = self
            .alerts
            .iter()
            .enumerate()
            .min_by(|a, b| (a.1 - price).abs().partial_cmp(&(b.1 - price).abs()).unwrap())
        {
            self.alerts.remove(idx);
        }
    }
    pub fn scroll_left(&mut self, by: usize) {
        if self.crosshair {
            self.cross_offset = self.cross_offset.saturating_add(by);
        } else {
            self.scroll_back = self.scroll_back.saturating_add(by);
        }
    }
    pub fn scroll_right(&mut self, by: usize) {
        if self.crosshair {
            self.cross_offset = self.cross_offset.saturating_sub(by);
        } else {
            self.scroll_back = self.scroll_back.saturating_sub(by);
        }
    }
    pub fn scroll_home(&mut self) {
        self.scroll_back = 0;
        self.cross_offset = 0;
    }
    pub fn toggle_crosshair(&mut self) {
        self.crosshair = !self.crosshair;
        if !self.crosshair {
            self.cross_offset = 0;
        }
    }
}

const PRICE_AXIS_W: u16 = 9;
const PROFILE_W: u16 = 12;
const VOLUME_H: u16 = 4;
const INDICATOR_H: u16 = 5;

pub fn draw(frame: &mut Frame, area: Rect, app: &AppState) {
    let view = &app.chart;
    let title = format!(
        "Chart — {tf} {ct} {vm} [{ind}]{vwap}{vp}{cd}",
        tf = view.timeframe_label(),
        ct = view.candle_type.label(),
        vm = view.volume_mode.label(),
        ind = view.indicator.label(),
        vwap = if view.show_vwap { " VWAP" } else { "" },
        vp = if view.show_volume_profile { " VP" } else { "" },
        cd = if view.show_cum_delta { " CΔ" } else { "" },
    );
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width <= PRICE_AXIS_W + 4 || inner.height < 6 {
        let p = Paragraph::new("terminal too small for chart").style(Style::default().fg(Color::DarkGray));
        frame.render_widget(p, inner);
        return;
    }

    let series = app.active.candles.aggregate(view.timeframe_ms);
    if series.is_empty() {
        let p = Paragraph::new("waiting for first trade…").style(Style::default().fg(Color::DarkGray));
        frame.render_widget(p, inner);
        return;
    }

    // Vertical layout: candle area / volume / [indicator] / time-axis row.
    let mut rest_h = inner.height;
    let time_axis_h: u16 = 1;
    rest_h = rest_h.saturating_sub(time_axis_h);
    let ind_h = if view.show_indicator_panel { INDICATOR_H.min(rest_h.saturating_sub(VOLUME_H + 4)) } else { 0 };
    rest_h = rest_h.saturating_sub(ind_h);
    let vol_h = VOLUME_H.min(rest_h.saturating_sub(4));
    rest_h = rest_h.saturating_sub(vol_h);
    let candle_h = rest_h;

    // Horizontal split: price-axis right, optional volume profile right of axis.
    let profile_w = if view.show_volume_profile { PROFILE_W } else { 0 };

    let plot_w = inner.width.saturating_sub(PRICE_AXIS_W + profile_w);
    let plot_x = inner.x;
    let axis_x = plot_x + plot_w;
    let profile_x = axis_x + PRICE_AXIS_W;

    let candle_area = Rect { x: plot_x, y: inner.y, width: plot_w, height: candle_h };
    let axis_area = Rect { x: axis_x, y: inner.y, width: PRICE_AXIS_W, height: candle_h };
    let profile_area = Rect { x: profile_x, y: inner.y, width: profile_w, height: candle_h };
    let volume_area = Rect { x: plot_x, y: inner.y + candle_h, width: plot_w, height: vol_h };
    let indicator_area = Rect { x: plot_x, y: inner.y + candle_h + vol_h, width: plot_w, height: ind_h };
    let time_axis_area = Rect {
        x: plot_x,
        y: inner.y + candle_h + vol_h + ind_h,
        width: plot_w,
        height: time_axis_h,
    };

    // Pick the visible bars window.
    let total = series.len();
    let max_bars = plot_w as usize;
    if max_bars == 0 || total == 0 {
        return;
    }
    // Apply scroll_back, clamped to available history.
    let scroll = view.scroll_back.min(total.saturating_sub(1));
    let end_idx = total - scroll;
    let start_idx = end_idx.saturating_sub(max_bars);
    let bars: Vec<&CandleBar> = series.iter().skip(start_idx).take(end_idx - start_idx).collect();
    if bars.is_empty() {
        return;
    }

    // Compute indicator series over the FULL aggregated history so that
    // displayed values are stable as the user scrolls. Slicing happens in
    // the draw_* functions below.
    let full_indicators = compute_full_indicators(&series);

    // Candle rendering colors are computed against a transformed series:
    // Heikin Ashi or Delta retain the same OHLC view but recolor differently.
    let display_bars: DisplayBars = make_display_bars(&bars, view.candle_type);

    // Vertical price range across visible bars + volume max.
    let (mut p_lo, mut p_hi) = price_range(&display_bars);
    if let Some(vwap) = app.active.candles.session_vwap() {
        if view.show_vwap {
            p_hi = p_hi.max(vwap);
            p_lo = p_lo.min(vwap);
        }
    }
    if p_hi <= p_lo {
        p_hi = p_lo + 1.0;
    }
    // Pad ranges 5%.
    let pad = (p_hi - p_lo) * 0.05;
    let p_hi = p_hi + pad;
    let p_lo = (p_lo - pad).max(0.0);

    draw_candle_area(
        frame,
        candle_area,
        &display_bars,
        view,
        app,
        p_lo,
        p_hi,
        &full_indicators,
        start_idx,
    );
    draw_price_axis(frame, axis_area, p_lo, p_hi, app.active.candles.session_vwap(), view);
    if view.show_volume_profile {
        draw_volume_profile(frame, profile_area, &bars, p_lo, p_hi);
    }
    draw_volume(frame, volume_area, &bars, view, &full_indicators, start_idx);
    if view.show_indicator_panel {
        draw_indicator(frame, indicator_area, &bars, view, &full_indicators, start_idx);
    }
    draw_time_axis(frame, time_axis_area, &bars);
}

/// Indicator series computed over the entire aggregated history. Slicing
/// happens at the draw site; this avoids the bug where each draw call
/// re-seeded EMA/RSI/MACD on the visible window only and produced
/// scroll-dependent values.
struct FullIndicators {
    ema9: Vec<Option<f64>>,
    ema21: Vec<Option<f64>>,
    ema50: Vec<Option<f64>>,
    rsi14: Vec<Option<f64>>,
    macd_hist: Vec<Option<f64>>,
    /// 20-bar SMA of total volume — computed once over the full series so
    /// the left edge of the volume panel doesn't show distorted (short-window)
    /// averages when the user scrolls.
    vol_sma20: Vec<f64>,
}

fn compute_full_indicators(series: &AggregatedSeries) -> FullIndicators {
    let mut e9 = Ema::new(9);
    let mut e21 = Ema::new(21);
    let mut e50 = Ema::new(50);
    let mut rsi = Rsi::new(14);
    let mut macd = Macd::new(12, 26, 9);
    let mut ema9 = Vec::with_capacity(series.len());
    let mut ema21 = Vec::with_capacity(series.len());
    let mut ema50 = Vec::with_capacity(series.len());
    let mut rsi14 = Vec::with_capacity(series.len());
    let mut macd_hist = Vec::with_capacity(series.len());
    let mut vols: Vec<f64> = Vec::with_capacity(series.len());
    for bar in series.iter() {
        let c = bar.close;
        ema9.push(e9.update(c));
        ema21.push(e21.update(c));
        ema50.push(e50.update(c));
        rsi14.push(rsi.update(c));
        macd_hist.push(macd.update(c).map(|v| v.histogram));
        vols.push(bar.total_volume());
    }
    // 20-bar trailing SMA of volume — true rolling window; the left edge
    // of the visible window inherits the correct historical average.
    let vol_sma20: Vec<f64> = (0..vols.len())
        .map(|i| {
            let lo = i.saturating_sub(19);
            let slice = &vols[lo..=i];
            slice.iter().sum::<f64>() / slice.len() as f64
        })
        .collect();
    FullIndicators { ema9, ema21, ema50, rsi14, macd_hist, vol_sma20 }
}

#[derive(Debug, Clone)]
struct DisplayBar {
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    /// True OHLC for axis/labels (used with Heikin-Ashi to remember real prices).
    real_close: f64,
    real_high: f64,
    real_low: f64,
    /// Coloring rule depends on candle type; precomputed.
    bullish: bool,
    doji: bool,
}

#[derive(Debug, Clone)]
struct DisplayBars {
    bars: Vec<DisplayBar>,
}

impl DisplayBars {
    fn iter(&self) -> std::slice::Iter<'_, DisplayBar> {
        self.bars.iter()
    }
}

fn make_display_bars(bars: &[&CandleBar], ctype: CandleType) -> DisplayBars {
    let owned: Vec<CandleBar> = bars.iter().map(|b| **b).collect();
    let ha: Vec<crate::candle::HeikinAshi> = if matches!(ctype, CandleType::HeikinAshi) {
        heikin_ashi_series(&owned)
    } else {
        Vec::new()
    };

    let out: Vec<DisplayBar> = bars
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let (o, h, l, c) = match ctype {
                CandleType::HeikinAshi => {
                    let h = ha[i];
                    (h.open, h.high, h.low, h.close)
                }
                CandleType::Standard | CandleType::Delta => (b.open, b.high, b.low, b.close),
            };
            let delta = b.delta();
            let bullish = match ctype {
                CandleType::Standard | CandleType::HeikinAshi => c > o,
                CandleType::Delta => delta > 0.0,
            };
            let doji = (c - o).abs() <= 1e-9 && delta.abs() <= 1e-9;
            let _ = delta;
            DisplayBar {
                open: o,
                high: h,
                low: l,
                close: c,
                real_close: b.close,
                real_high: b.high,
                real_low: b.low,
                bullish,
                doji,
            }
        })
        .collect();
    DisplayBars { bars: out }
}

fn price_range(disp: &DisplayBars) -> (f64, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for b in disp.iter() {
        if b.low < lo { lo = b.low; }
        if b.high > hi { hi = b.high; }
        if b.real_low < lo { lo = b.real_low; }
        if b.real_high > hi { hi = b.real_high; }
    }
    if !lo.is_finite() || !hi.is_finite() {
        (0.0, 1.0)
    } else {
        (lo, hi)
    }
}

fn draw_candle_area(
    frame: &mut Frame,
    area: Rect,
    bars: &DisplayBars,
    view: &ChartView,
    app: &AppState,
    p_lo: f64,
    p_hi: f64,
    indicators: &FullIndicators,
    start_idx: usize,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let h = area.height as usize;
    let w = area.width as usize;
    let visible_bars = bars.bars.len().min(w);

    // Build a 2D character grid pre-filled with background grid characters.
    let mut grid: Vec<Vec<(char, Style)>> = (0..h)
        .map(|_| (0..w).map(|_| (' ', Style::default())).collect())
        .collect();

    // Subtle horizontal grid every ~quarter of the height.
    let grid_color = Color::Rgb(30, 30, 30);
    for r in (0..h).step_by((h / 4).max(1)) {
        for c in 0..w {
            grid[r][c] = ('·', Style::default().fg(grid_color));
        }
    }

    // Cell price units (each cell row covers cell_h price; half = cell_h/2).
    let range = p_hi - p_lo;
    let cell_h = range / h as f64;
    let half = cell_h / 2.0;

    let price_to_row = |p: f64| -> Option<usize> {
        if p < p_lo || p > p_hi || cell_h <= 0.0 { return None; }
        let from_top = (p_hi - p) / cell_h;
        let r = from_top as usize;
        Some(r.min(h - 1))
    };

    // Render candles.
    for (col_off, b) in bars.iter().enumerate().take(visible_bars) {
        let col = col_off; // bar column = grid x position
        if col >= w { break; }

        let body_hi = b.open.max(b.close);
        let body_lo = b.open.min(b.close);
        let color = candle_color(b);

        for r in 0..h {
            let row_top = p_hi - r as f64 * cell_h;
            let row_bot = row_top - cell_h;
            // Upper half mid-price and lower half mid-price.
            let upper_mid = row_top - half / 2.0;
            let lower_mid = row_bot + half / 2.0;

            let upper_in_wick = upper_mid >= b.low && upper_mid <= b.high;
            let lower_in_wick = lower_mid >= b.low && lower_mid <= b.high;
            let upper_in_body = upper_in_wick && upper_mid >= body_lo && upper_mid <= body_hi;
            let lower_in_body = lower_in_wick && lower_mid >= body_lo && lower_mid <= body_hi;

            let style = Style::default().fg(color);
            let glyph = match (upper_in_body, lower_in_body, upper_in_wick, lower_in_wick) {
                (true, true, _, _) => '█',
                (true, false, _, false) => '▀',
                (false, true, false, _) => '▄',
                (true, false, _, true) => '▀',
                (false, true, true, _) => '▄',
                (false, false, true, true) => '│',
                (false, false, true, false) => '╵',
                (false, false, false, true) => '╷',
                _ => continue,
            };
            grid[r][col] = (glyph, style);
        }

        // Doji: ensure at least one '+' marker.
        if b.doji {
            if let Some(r) = price_to_row(b.close) {
                grid[r][col] = ('+', Style::default().fg(Color::Yellow));
            }
        }
    }

    // VWAP overlay.
    if view.show_vwap {
        if let Some(vwap) = app.active.candles.session_vwap() {
            if let Some(r) = price_to_row(vwap) {
                for c in 0..w {
                    grid[r][c] = ('─', Style::default().fg(Color::Magenta));
                }
            }
            // Std-dev bands.
            if let Some(sd) = app.active.candles.session_vwap_stddev() {
                for (k, ch) in [(1.0_f64, '╌'), (2.0_f64, '╍')] {
                    for sign in [1.0_f64, -1.0] {
                        if let Some(r) = price_to_row(vwap + sign * k * sd) {
                            for c in 0..w {
                                if grid[r][c].0 == ' ' || grid[r][c].0 == '·' {
                                    grid[r][c] = (ch, Style::default().fg(Color::DarkGray));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // EMA overlays — values were precomputed over the FULL aggregated
    // series; we just slice the visible window. This makes the displayed
    // values stable regardless of how the user scrolls the chart.
    let ema_sources: [(&Vec<Option<f64>>, Color, usize); 3] = [
        (&indicators.ema9, Color::LightCyan, 0),
        (&indicators.ema21, Color::LightYellow, 1),
        (&indicators.ema50, Color::LightMagenta, 2),
    ];
    for (ema_series, color, idx) in ema_sources {
        if !view.show_emas[idx] { continue; }
        let mut prev_row: Option<usize> = None;
        for col in 0..visible_bars {
            let global_idx = start_idx + col;
            let v = match ema_series.get(global_idx).and_then(|x| *x) {
                Some(v) => v,
                None => continue,
            };
            if let Some(r) = price_to_row(v) {
                grid[r][col] = ('·', Style::default().fg(color));
                if let Some(pr) = prev_row {
                    let (lo, hi) = if pr < r { (pr, r) } else { (r, pr) };
                    for rr in lo..=hi {
                        if grid[rr][col].0 == ' ' || grid[rr][col].0 == '·' {
                            grid[rr][col] = ('·', Style::default().fg(color));
                        }
                    }
                }
                prev_row = Some(r);
            }
        }
    }

    // Cumulative delta overlay (secondary axis): scale between min/max of cum_delta_trail.
    if view.show_cum_delta {
        let trail = app.active.candles.cum_delta_trail();
        if !trail.is_empty() {
            let total_completed = app.active.candles.completed().len();
            // Map global trail indices to local visible indices.
            let global_start = total_completed.saturating_sub(visible_bars);
            let local: Vec<f64> = trail
                .iter()
                .skip(global_start)
                .copied()
                .take(visible_bars)
                .collect();
            if !local.is_empty() {
                let mut lo = f64::INFINITY;
                let mut hi = f64::NEG_INFINITY;
                for v in &local {
                    if *v < lo { lo = *v; }
                    if *v > hi { hi = *v; }
                }
                if lo.is_finite() && hi.is_finite() && (hi - lo).abs() > 0.0 {
                    let span = (hi - lo).max(1e-9);
                    for (col, v) in local.iter().enumerate() {
                        let frac = (v - lo) / span;
                        let r = ((1.0 - frac) * (h as f64 - 1.0)).round() as usize;
                        if r < h {
                            if grid[r][col].0 == ' ' || grid[r][col].0 == '·' {
                                grid[r][col] = ('•', Style::default().fg(Color::Cyan));
                            }
                        }
                    }
                    // Divergence detection (simple): price new high but cumDelta not.
                    detect_and_mark_divergences(&mut grid, &bars.bars[..visible_bars], &local);
                }
            }
        }
    }

    // Last trade dotted horizontal line.
    if let Some(last) = bars.bars.last() {
        if let Some(r) = price_to_row(last.real_close) {
            for c in 0..w {
                if grid[r][c].0 == ' ' || grid[r][c].0 == '·' {
                    grid[r][c] = ('·', Style::default().fg(Color::Yellow));
                }
            }
        }
    }

    // Alert level overlays.
    for &alert_price in &view.alerts {
        if let Some(r) = price_to_row(alert_price) {
            for c in 0..w {
                if c % 2 == 0 {
                    grid[r][c] = ('─', Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD));
                }
            }
        }
    }

    // Crosshair vertical+horizontal lines + data panel.
    if view.crosshair && !bars.bars.is_empty() {
        let last_idx = visible_bars.saturating_sub(1);
        let cross_col = last_idx.saturating_sub(view.cross_offset.min(last_idx));
        let cross_bar = &bars.bars[cross_col];
        let cross_row = price_to_row(cross_bar.real_close).unwrap_or(h / 2);

        for r in 0..h {
            let prev = grid[r][cross_col];
            grid[r][cross_col] = (
                if prev.0 == ' ' { '│' } else { prev.0 },
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            );
        }
        for c in 0..w {
            let prev = grid[cross_row][c];
            grid[cross_row][c] = (
                if prev.0 == ' ' || prev.0 == '·' { '─' } else { prev.0 },
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            );
        }
        grid[cross_row][cross_col] = ('┼', Style::default().fg(Color::White).add_modifier(Modifier::BOLD));

        // Data panel: position based on which half the crosshair is in.
        let panel_lines = data_panel_lines(cross_bar, app, view, &bars.bars[..visible_bars], cross_col);
        let panel_w: usize = panel_lines.iter().map(|s| s.chars().count()).max().unwrap_or(0).max(20);
        let panel_h = panel_lines.len();
        let on_left = cross_col >= w / 2;
        let panel_x = if on_left { 1 } else { w.saturating_sub(panel_w + 1) };
        let panel_y = 0;
        for (i, line) in panel_lines.iter().enumerate() {
            let row = panel_y + i;
            if row >= h || row >= panel_y + panel_h { break; }
            for (j, ch) in line.chars().enumerate() {
                let c = panel_x + j;
                if c >= w { break; }
                grid[row][c] = (ch, Style::default().fg(Color::Black).bg(Color::Gray));
            }
        }
    }

    // Render grid into Lines.
    let lines: Vec<Line> = grid
        .into_iter()
        .map(|row| {
            let spans: Vec<Span> = row
                .into_iter()
                .map(|(c, s)| Span::styled(c.to_string(), s))
                .collect();
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn candle_color(b: &DisplayBar) -> Color {
    if b.doji {
        Color::Gray
    } else if b.bullish {
        Color::Green
    } else {
        Color::Red
    }
}

fn data_panel_lines(
    bar: &DisplayBar,
    app: &AppState,
    _view: &ChartView,
    visible: &[DisplayBar],
    cross_col: usize,
) -> Vec<String> {
    // Compute EMAs at this cross point from prefix of closes.
    let prefix: Vec<f64> = visible.iter().take(cross_col + 1).map(|b| b.real_close).collect();
    let mut e9 = Ema::new(9);
    let mut e21 = Ema::new(21);
    let mut e50 = Ema::new(50);
    for c in &prefix {
        e9.update(*c);
        e21.update(*c);
        e50.update(*c);
    }
    let mut rsi = Rsi::new(14);
    for c in &prefix {
        rsi.update(*c);
    }

    let vwap = app.active.candles.session_vwap().unwrap_or(0.0);
    let mut lines: Vec<String> = Vec::new();
    lines.push(" Selected ".into());
    lines.push(format!(" O: {:>10.2} ", bar.open));
    lines.push(format!(" H: {:>10.2} ", bar.real_high));
    lines.push(format!(" L: {:>10.2} ", bar.real_low));
    lines.push(format!(" C: {:>10.2} ", bar.real_close));
    let total_vol: f64 = app
        .active
        .candles
        .completed()
        .iter()
        .chain(app.active.candles.forming().into_iter())
        .map(|b| b.total_volume())
        .sum();
    let _ = total_vol;
    lines.push(format!(" Δ: {:>+10.4} ", bar.real_close - bar.open));
    lines.push(format!(" VWAP: {:>7.2} ", vwap));
    if let Some(v) = e9.value() { lines.push(format!(" EMA9 : {:>6.2} ", v)); }
    if let Some(v) = e21.value() { lines.push(format!(" EMA21: {:>6.2} ", v)); }
    if let Some(v) = e50.value() { lines.push(format!(" EMA50: {:>6.2} ", v)); }
    if let Some(v) = rsi.value() { lines.push(format!(" RSI  : {:>6.2} ", v)); }
    lines
}

fn detect_and_mark_divergences(
    grid: &mut Vec<Vec<(char, Style)>>,
    bars: &[DisplayBar],
    cum_delta_local: &[f64],
) {
    // Use the prefix common to both inputs — `bars` typically includes the
    // forming bar while `cum_delta_local` only carries values for sealed
    // bars, so an exact-length match almost never holds in the live path.
    let n = bars.len().min(cum_delta_local.len());
    if n < 5 {
        return;
    }
    let bars = &bars[..n];
    let cum_delta_local = &cum_delta_local[..n];
    // Simple swing-high / swing-low detection over a short lookback.
    let look = 3.min(n / 2);
    let h = grid.len();
    if h == 0 { return; }
    for i in look..n.saturating_sub(look) {
        let c = bars[i].real_close;
        let d = cum_delta_local[i];
        let mut higher_price = true;
        let mut higher_delta = true;
        for j in (i.saturating_sub(look))..i {
            if bars[j].real_close >= c { higher_price = false; }
            if cum_delta_local[j] >= d { higher_delta = false; }
        }
        // Bearish divergence: new price high, no new delta high.
        if higher_price && !higher_delta {
            let row = h.saturating_sub(1);
            grid[row][i] = ('⚠', Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD));
        }
        let mut lower_price = true;
        let mut lower_delta = true;
        for j in (i.saturating_sub(look))..i {
            if bars[j].real_close <= c { lower_price = false; }
            if cum_delta_local[j] <= d { lower_delta = false; }
        }
        if lower_price && !lower_delta {
            let row = h.saturating_sub(1);
            grid[row][i] = ('⚠', Style::default().fg(Color::LightGreen).add_modifier(Modifier::BOLD));
        }
    }
}

fn draw_price_axis(frame: &mut Frame, area: Rect, p_lo: f64, p_hi: f64, vwap: Option<f64>, view: &ChartView) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let h = area.height as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(h);
    for r in 0..h {
        let frac = r as f64 / (h - 1).max(1) as f64;
        let price = p_hi - frac * (p_hi - p_lo);
        let label = format!("{price:>8.2}");
        let mut style = Style::default().fg(Color::Gray);
        if let Some(v) = vwap {
            if (price - v).abs() <= (p_hi - p_lo) / h as f64 {
                style = Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD);
            }
        }
        for &alert in &view.alerts {
            if (price - alert).abs() <= (p_hi - p_lo) / h as f64 {
                style = Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD);
            }
        }
        lines.push(Line::from(Span::styled(label, style)));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_volume_profile(frame: &mut Frame, area: Rect, bars: &[&CandleBar], p_lo: f64, p_hi: f64) {
    if area.width <= 1 || area.height == 0 {
        return;
    }
    let h = area.height as usize;
    let w = area.width as usize;
    let cell_h = (p_hi - p_lo) / h as f64;
    if cell_h <= 0.0 { return; }
    let mut buckets: Vec<f64> = vec![0.0; h];
    for b in bars {
        let p = b.close;
        let r = ((p_hi - p) / cell_h) as usize;
        if r < h {
            buckets[r] += b.total_volume();
        }
    }
    let max = buckets.iter().copied().fold(0.0_f64, f64::max);
    // POC and Value Area (70%).
    let poc_idx = buckets
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(i, _)| i);
    let total_vol: f64 = buckets.iter().sum();
    let mut va_set = std::collections::HashSet::new();
    if let Some(poc) = poc_idx {
        let target = total_vol * 0.7;
        let mut acc = buckets[poc];
        va_set.insert(poc);
        let (mut up, mut down) = (poc as i64 - 1, poc as i64 + 1);
        while acc < target {
            let up_v = if up >= 0 { buckets[up as usize] } else { -1.0 };
            let dn_v = if (down as usize) < h { buckets[down as usize] } else { -1.0 };
            if up_v < 0.0 && dn_v < 0.0 { break; }
            if up_v >= dn_v {
                if up >= 0 { va_set.insert(up as usize); acc += up_v; up -= 1; }
            } else {
                if (down as usize) < h { va_set.insert(down as usize); acc += dn_v; down += 1; }
            }
        }
    }
    let mut lines: Vec<Line> = Vec::with_capacity(h);
    for r in 0..h {
        let v = buckets[r];
        let bar_w = if max > 0.0 { ((v / max) * (w as f64 - 2.0)).round() as usize } else { 0 };
        let mut style = Style::default().fg(Color::Blue);
        if Some(r) == poc_idx {
            style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
        } else if va_set.contains(&r) {
            style = Style::default().fg(Color::LightBlue);
        }
        let mut s = String::new();
        s.push(' ');
        for _ in 0..bar_w { s.push('▌'); }
        lines.push(Line::from(Span::styled(s, style)));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_volume(
    frame: &mut Frame,
    area: Rect,
    bars: &[&CandleBar],
    view: &ChartView,
    indicators: &FullIndicators,
    start_idx: usize,
) {
    if area.width == 0 || area.height == 0 { return; }
    let h = area.height as usize;
    let w = area.width as usize;
    let visible = bars.len().min(w);

    // Max for scaling
    let max_v = bars.iter().map(|b| b.total_volume()).fold(0.0_f64, f64::max).max(1e-9);

    // 20-bar moving average — precomputed over full history; slice here.
    let sma: Vec<f64> = indicators
        .vol_sma20
        .iter()
        .skip(start_idx)
        .take(visible)
        .copied()
        .collect();
    let mut grid: Vec<Vec<(char, Style)>> = (0..h).map(|_| (0..w).map(|_| (' ', Style::default())).collect()).collect();
    for (col, b) in bars.iter().enumerate().take(visible) {
        let total = b.total_volume();
        let frac = total / max_v;
        let bar_h = (frac * h as f64).round() as usize;
        match view.volume_mode {
            VolumeMode::Standard => {
                let color = if b.delta() >= 0.0 { Color::Green } else { Color::Red };
                for r in 0..bar_h {
                    let row = h - 1 - r;
                    grid[row][col] = ('█', Style::default().fg(color));
                }
            }
            VolumeMode::DeltaStack => {
                // Stack: buy fills from the bottom, sell stacks above it.
                // CRITICAL: clip the sell range to the rows still empty
                // above buy. Without the clip, when buy_h fills the whole
                // bar the saturating_sub bottoms out at row 0 and the
                // sell loop overwrites the bottom buy cells.
                let buy_h = if total > 0.0 { ((b.buy_volume / max_v) * h as f64).round() as usize } else { 0 };
                let sell_h = if total > 0.0 { ((b.sell_volume / max_v) * h as f64).round() as usize } else { 0 };
                let buy_h = buy_h.min(h);
                let sell_room = h.saturating_sub(buy_h);
                let sell_h = sell_h.min(sell_room);
                for r in 0..buy_h {
                    let row = h - 1 - r;
                    grid[row][col] = ('█', Style::default().fg(Color::Green));
                }
                for r in 0..sell_h {
                    let row = sell_room.saturating_sub(1 + r);
                    grid[row][col] = ('█', Style::default().fg(Color::Red));
                }
            }
        }
        // Highlight bars > 1.5x the SMA
        if total > sma[col] * 1.5 {
            let r = h.saturating_sub(bar_h.max(1));
            if r < h {
                let (_, mut s) = grid[r][col];
                s = s.add_modifier(Modifier::BOLD | Modifier::REVERSED);
                grid[r][col].1 = s;
            }
        }
    }
    // Volume MA line overlay
    for (col, &avg) in sma.iter().enumerate().take(visible) {
        let frac = avg / max_v;
        let r = h.saturating_sub((frac * h as f64).round() as usize).saturating_sub(1).min(h - 1);
        if grid[r][col].0 == ' ' {
            grid[r][col] = ('─', Style::default().fg(Color::White));
        }
    }
    let lines: Vec<Line> = grid.into_iter().map(|row| Line::from(row.into_iter().map(|(c, s)| Span::styled(c.to_string(), s)).collect::<Vec<_>>())).collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_indicator(
    frame: &mut Frame,
    area: Rect,
    bars: &[&CandleBar],
    view: &ChartView,
    indicators: &FullIndicators,
    start_idx: usize,
) {
    if area.width == 0 || area.height == 0 { return; }
    let h = area.height as usize;
    let w = area.width as usize;
    let visible = bars.len().min(w);

    // Slice the relevant portion of the precomputed full indicator series.
    let slice_full = |full: &[Option<f64>]| -> Vec<Option<f64>> {
        full.iter().skip(start_idx).take(visible).copied().collect()
    };

    let (values, lo, hi, color_fn): (Vec<Option<f64>>, f64, f64, Box<dyn Fn(f64) -> Color>) = match view.indicator {
        IndicatorMode::Rsi => {
            let vals = slice_full(&indicators.rsi14);
            let color: Box<dyn Fn(f64) -> Color> = Box::new(|v: f64| {
                if v >= 70.0 { Color::Red } else if v <= 30.0 { Color::Green } else { Color::Cyan }
            });
            (vals, 0.0, 100.0, color)
        }
        IndicatorMode::Macd => {
            let vals = slice_full(&indicators.macd_hist);
            let mut min_h = 0.0_f64;
            let mut max_h = 0.0_f64;
            for v in vals.iter().flatten() {
                if *v < min_h { min_h = *v; }
                if *v > max_h { max_h = *v; }
            }
            let span = (max_h - min_h).max(1e-9);
            let color: Box<dyn Fn(f64) -> Color> = Box::new(|v: f64| if v >= 0.0 { Color::Green } else { Color::Red });
            (vals, min_h - span * 0.1, max_h + span * 0.1, color)
        }
        IndicatorMode::DeltaMomentum => {
            // Delta momentum across visible bars: derive cum delta from bar deltas.
            let mut cum = 0.0;
            let mut series: Vec<f64> = Vec::with_capacity(visible);
            for b in bars.iter().take(visible) {
                cum += b.delta();
                series.push(cum);
            }
            let mut dmo = crate::indicators::DeltaMomentum::new(5);
            let vals: Vec<Option<f64>> = series.iter().map(|s| dmo.update(*s)).collect();
            let mut min_v = 0.0_f64;
            let mut max_v = 0.0_f64;
            for v in vals.iter().flatten() {
                if *v < min_v { min_v = *v; }
                if *v > max_v { max_v = *v; }
            }
            let span = (max_v - min_v).max(1e-9);
            let color: Box<dyn Fn(f64) -> Color> = Box::new(|v: f64| if v >= 0.0 { Color::Green } else { Color::Red });
            (vals, min_v - span * 0.1, max_v + span * 0.1, color)
        }
    };

    let mut grid: Vec<Vec<(char, Style)>> = (0..h).map(|_| (0..w).map(|_| (' ', Style::default())).collect()).collect();
    let span = (hi - lo).max(1e-9);
    // Reference levels for RSI
    if matches!(view.indicator, IndicatorMode::Rsi) {
        for level in [30.0, 70.0] {
            let r = (((hi - level) / span) * (h as f64 - 1.0)).round() as usize;
            if r < h {
                for c in 0..w {
                    if grid[r][c].0 == ' ' {
                        grid[r][c] = ('-', Style::default().fg(Color::DarkGray));
                    }
                }
            }
        }
    }
    let mut prev: Option<usize> = None;
    for (col, v) in values.iter().enumerate().take(visible) {
        if let Some(val) = v {
            let frac = (hi - val) / span;
            let r = (frac * (h as f64 - 1.0)).clamp(0.0, h as f64 - 1.0) as usize;
            grid[r][col] = ('●', Style::default().fg(color_fn(*val)));
            if let Some(pr) = prev {
                let (lo_r, hi_r) = if pr < r { (pr, r) } else { (r, pr) };
                for rr in lo_r..=hi_r {
                    if grid[rr][col].0 == ' ' {
                        grid[rr][col] = ('·', Style::default().fg(color_fn(*val)));
                    }
                }
            }
            prev = Some(r);
        }
    }
    let lines: Vec<Line> = grid.into_iter().map(|row| Line::from(row.into_iter().map(|(c, s)| Span::styled(c.to_string(), s)).collect::<Vec<_>>())).collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_time_axis(frame: &mut Frame, area: Rect, bars: &[&CandleBar]) {
    if area.width == 0 || area.height == 0 { return; }
    let w = area.width as usize;
    let visible = bars.len().min(w);
    let label_every = (visible / 6).max(6);
    let mut text = String::with_capacity(w);
    let mut i = 0;
    while i < visible {
        if i % label_every == 0 {
            let dt = OffsetDateTime::from_unix_timestamp(bars[i].start_ms / 1000).unwrap_or(OffsetDateTime::UNIX_EPOCH);
            let lbl = dt.format(HHMM).unwrap_or_else(|_| "--:--".into());
            for ch in lbl.chars() {
                if text.len() < w { text.push(ch); }
            }
            i += lbl.chars().count();
            while i % label_every != 0 && text.len() < w {
                text.push(' ');
                i += 1;
            }
        } else {
            text.push(' ');
            i += 1;
        }
    }
    while text.chars().count() < w { text.push(' '); }
    frame.render_widget(Paragraph::new(Line::from(Span::styled(text, Style::default().fg(Color::DarkGray)))), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeframe_label_known() {
        let mut v = ChartView::default();
        assert_eq!(v.timeframe_label(), "1m");
        v.timeframe_ms = 900_000;
        assert_eq!(v.timeframe_label(), "15m");
        v.timeframe_ms = 3_600_000;
        assert_eq!(v.timeframe_label(), "1h");
    }

    #[test]
    fn timeframe_cycle_clamps() {
        let mut v = ChartView::default();
        for _ in 0..10 { v.cycle_timeframe_up(); }
        assert_eq!(v.timeframe_ms, 3_600_000);
        for _ in 0..10 { v.cycle_timeframe_down(); }
        assert_eq!(v.timeframe_ms, 60_000);
    }

    #[test]
    fn alerts_round_trip() {
        let mut v = ChartView::default();
        v.add_alert(100.0);
        v.add_alert(105.0);
        v.add_alert(100.0); // duplicate
        assert_eq!(v.alerts.len(), 2);
        v.remove_nearest_alert(101.0);
        assert!(v.alerts.iter().any(|x| (*x - 105.0).abs() < 1e-9));
        assert!(!v.alerts.iter().any(|x| (*x - 100.0).abs() < 1e-9));
    }

    #[test]
    fn cycle_candle_type_round_trips() {
        let mut v = ChartView::default();
        let original = v.candle_type;
        v.cycle_candle_type();
        v.cycle_candle_type();
        v.cycle_candle_type();
        assert_eq!(v.candle_type, original);
    }
}
